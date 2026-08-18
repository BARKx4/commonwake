use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::{
    PROTOCOL_VERSION,
    crypto::{
        REPOSITORY_MANIFEST_DOMAIN, prefixed_id, sha256_hex, sign_object, verify_object,
        verifying_key_from_b64,
    },
    error::{CommonwakeError, Result},
    node::NodeIdentity,
};

pub const SELF_REPOSITORY_NAMESPACE: &str = "commonwake/reference";
pub const SELF_REPOSITORY_NAME: &str = "Commonwake";
pub const SOURCE_BUNDLE_MEDIA_TYPE: &str = "application/x-git-bundle";
pub const MAX_SOURCE_BUNDLE_BYTES: u64 = 256 * 1024 * 1024;

static SOURCE_BUNDLE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/commonwake-source.bundle"));
static SOURCE_DIGEST: OnceLock<String> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RepositoryArtifact {
    pub kind: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub media_type: String,
    pub download_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RepositoryManifest {
    pub protocol: String,
    pub repository_id: String,
    pub namespace: String,
    pub name: String,
    pub description: String,
    pub license: String,
    pub vcs: String,
    pub default_ref: String,
    pub source_revision: String,
    pub source_matches_build: bool,
    pub source_provenance: String,
    pub artifact: RepositoryArtifact,
    pub reconstruction_path: String,
    pub node_id: String,
    pub node_public_key: String,
    pub trust_notice: String,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepositorySummary {
    pub repository_id: String,
    pub namespace: String,
    pub name: String,
    pub manifest_path: String,
    pub source_revision: String,
    pub source_matches_build: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RepositoryVerificationReport {
    pub status: &'static str,
    pub repository_id: String,
    pub node_id: String,
    pub source_revision: String,
    pub source_matches_build: bool,
    pub source_provenance: String,
    pub artifact_sha256: String,
    pub artifact_verified: bool,
}

pub fn source_bundle() -> &'static [u8] {
    SOURCE_BUNDLE
}

pub fn source_digest() -> &'static str {
    SOURCE_DIGEST
        .get_or_init(|| sha256_hex(SOURCE_BUNDLE))
        .as_str()
}

pub fn self_repository_id() -> String {
    repository_id(SELF_REPOSITORY_NAMESPACE)
}

pub fn repository_id(namespace: &str) -> String {
    let mut identity = b"commonwake.repository.v1\0".to_vec();
    identity.extend_from_slice(namespace.as_bytes());
    prefixed_id("cwrepo_", &identity)
}

pub fn repository_summary() -> RepositorySummary {
    let repository_id = self_repository_id();
    RepositorySummary {
        manifest_path: format!("/v1/repositories/{repository_id}"),
        repository_id,
        namespace: SELF_REPOSITORY_NAMESPACE.into(),
        name: SELF_REPOSITORY_NAME.into(),
        source_revision: env!("COMMONWAKE_SOURCE_REVISION").into(),
        source_matches_build: source_matches_build(),
    }
}

pub fn make_repository_manifest(identity: &NodeIdentity) -> Result<RepositoryManifest> {
    let repository_id = self_repository_id();
    let digest = source_digest().to_owned();
    let mut manifest = RepositoryManifest {
        protocol: PROTOCOL_VERSION.into(),
        repository_id: repository_id.clone(),
        namespace: SELF_REPOSITORY_NAMESPACE.into(),
        name: SELF_REPOSITORY_NAME.into(),
        description: "The complete Git history used to reconstruct this Commonwake implementation."
            .into(),
        license: "AGPL-3.0-or-later".into(),
        vcs: "git".into(),
        default_ref: env!("COMMONWAKE_SOURCE_DEFAULT_REF").into(),
        source_revision: env!("COMMONWAKE_SOURCE_REVISION").into(),
        source_matches_build: source_matches_build(),
        source_provenance: env!("COMMONWAKE_SOURCE_PROVENANCE").into(),
        artifact: RepositoryArtifact {
            kind: "git-bundle".into(),
            sha256: digest.clone(),
            size_bytes: SOURCE_BUNDLE.len() as u64,
            media_type: SOURCE_BUNDLE_MEDIA_TYPE.into(),
            download_path: format!("/v1/artifacts/{digest}"),
        },
        reconstruction_path: "/v1/software/self/reconstruct.md".into(),
        node_id: identity.node_id().into(),
        node_public_key: identity.public_key().into(),
        trust_notice: "This node signature attributes the source claim; it does not prove that the remote process runs these bytes. Verify the digest, inspect the code, and require independent build evidence before execution."
            .into(),
        signature: String::new(),
    };
    manifest.signature = sign_object(
        identity.signing_key(),
        REPOSITORY_MANIFEST_DOMAIN,
        &manifest,
    )?;
    Ok(manifest)
}

pub fn verify_repository_manifest(manifest: &RepositoryManifest) -> Result<()> {
    if manifest.protocol != PROTOCOL_VERSION {
        return Err(CommonwakeError::Validation(format!(
            "repository manifest uses unsupported protocol {}",
            manifest.protocol
        )));
    }
    if manifest.repository_id != repository_id(&manifest.namespace) {
        return Err(CommonwakeError::Validation(
            "repository ID does not match its namespace".into(),
        ));
    }
    if manifest.namespace.is_empty()
        || manifest.namespace.len() > 160
        || !manifest.namespace.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'/')
        })
    {
        return Err(CommonwakeError::Validation(
            "repository namespace is not a bounded lowercase path".into(),
        ));
    }
    if manifest.vcs != "git"
        || manifest.artifact.kind != "git-bundle"
        || manifest.artifact.media_type != SOURCE_BUNDLE_MEDIA_TYPE
    {
        return Err(CommonwakeError::Validation(
            "repository manifest does not describe a supported Git bundle".into(),
        ));
    }
    validate_hex_digest(&manifest.artifact.sha256, "artifact SHA-256")?;
    if manifest.artifact.size_bytes == 0 || manifest.artifact.size_bytes > MAX_SOURCE_BUNDLE_BYTES {
        return Err(CommonwakeError::Validation(
            "repository artifact size is outside the supported bound".into(),
        ));
    }
    let expected_path = format!("/v1/artifacts/{}", manifest.artifact.sha256);
    if manifest.artifact.download_path != expected_path {
        return Err(CommonwakeError::Validation(
            "repository artifact path is not bound to its digest".into(),
        ));
    }
    if !matches!(manifest.source_revision.len(), 40 | 64)
        || !manifest
            .source_revision
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(CommonwakeError::Validation(
            "source revision is not a Git object ID".into(),
        ));
    }
    if manifest.default_ref != "HEAD" && !manifest.default_ref.starts_with("refs/heads/") {
        return Err(CommonwakeError::Validation(
            "source default ref is not a branch or HEAD".into(),
        ));
    }
    if manifest.name.is_empty()
        || manifest.name.len() > 160
        || manifest.description.is_empty()
        || manifest.description.len() > 2_000
        || manifest.license.is_empty()
        || manifest.license.len() > 80
        || manifest.trust_notice.is_empty()
        || manifest.trust_notice.len() > 4_000
    {
        return Err(CommonwakeError::Validation(
            "repository disclosure text is outside its bounds".into(),
        ));
    }
    if manifest.source_provenance.is_empty()
        || manifest.source_provenance.len() > 80
        || !manifest
            .source_provenance
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(CommonwakeError::Validation(
            "source provenance is not a bounded lowercase slug".into(),
        ));
    }
    if manifest.reconstruction_path != "/v1/software/self/reconstruct.md" {
        return Err(CommonwakeError::Validation(
            "repository reconstruction path is unsupported".into(),
        ));
    }
    let key = verifying_key_from_b64(&manifest.node_public_key)?;
    if manifest.node_id != prefixed_id("cwnode_", &key.to_bytes()) {
        return Err(CommonwakeError::Validation(
            "repository manifest node ID does not match its public key".into(),
        ));
    }
    verify_object(
        &key,
        REPOSITORY_MANIFEST_DOMAIN,
        manifest,
        &manifest.signature,
    )
}

pub fn verify_repository_bundle(manifest: &RepositoryManifest, bundle: &[u8]) -> Result<()> {
    verify_repository_manifest(manifest)?;
    if bundle.len() as u64 != manifest.artifact.size_bytes {
        return Err(CommonwakeError::Validation(
            "repository bundle size does not match its manifest".into(),
        ));
    }
    if sha256_hex(bundle) != manifest.artifact.sha256 {
        return Err(CommonwakeError::Validation(
            "repository bundle digest does not match its manifest".into(),
        ));
    }
    if !bundle.starts_with(b"# v2 git bundle\n") && !bundle.starts_with(b"# v3 git bundle\n") {
        return Err(CommonwakeError::Validation(
            "repository artifact is not a Git bundle".into(),
        ));
    }
    Ok(())
}

pub fn verification_report(
    manifest: &RepositoryManifest,
    artifact_verified: bool,
) -> RepositoryVerificationReport {
    RepositoryVerificationReport {
        status: "verified",
        repository_id: manifest.repository_id.clone(),
        node_id: manifest.node_id.clone(),
        source_revision: manifest.source_revision.clone(),
        source_matches_build: manifest.source_matches_build,
        source_provenance: manifest.source_provenance.clone(),
        artifact_sha256: manifest.artifact.sha256.clone(),
        artifact_verified,
    }
}

pub fn reconstruction_markdown(manifest: &RepositoryManifest) -> String {
    let manifest_path = format!("/v1/repositories/{}", manifest.repository_id);
    format!(
        "# Reconstruct this Commonwake node\n\n\
This is a source-recovery procedure, not permission to execute remote code without review.\n\n\
1. Save `GET {manifest_path}` as `manifest.json`.\n\
2. Download `GET {artifact_path}` as `commonwake.bundle`.\n\
3. Verify that its SHA-256 is `{digest}` and its size is `{size}` bytes.\n\
4. Run `git bundle verify commonwake.bundle`.\n\
5. Run `git clone commonwake.bundle commonwake`.\n\
6. Confirm that the checked-out revision is `{revision}`.\n\
7. Inspect the source, then run `cargo test --all-targets --all-features --locked`.\n\
8. Build with `cargo build --release --locked`.\n\
9. Retrospectively verify the saved manifest and bundle with the recovered binary:\n\
   `commonwake verify-repository-manifest --input manifest.json --bundle commonwake.bundle`.\n\n\
After inspection, a container-capable host may instead run `docker compose up -d --build`; the\n\
included default profile binds the peer to localhost and keeps its data in a named volume.\n\n\
The node-signed manifest attributes this claim but cannot remotely prove which binary is running.\n\
Source matches this build: `{exact}`. Provenance: `{provenance}`.\n",
        manifest_path = manifest_path,
        artifact_path = manifest.artifact.download_path,
        digest = manifest.artifact.sha256,
        size = manifest.artifact.size_bytes,
        revision = manifest.source_revision,
        exact = manifest.source_matches_build,
        provenance = manifest.source_provenance,
    )
}

fn source_matches_build() -> bool {
    env!("COMMONWAKE_SOURCE_EXACT") == "true"
}

fn validate_hex_digest(value: &str, label: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(CommonwakeError::Validation(format!(
            "{label} must be 64 lowercase hexadecimal characters"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::SigningKey;

    use super::*;
    use crate::crypto::{encode, sign_object};

    fn fixture_manifest() -> RepositoryManifest {
        let key = SigningKey::from_bytes(&[21_u8; 32]);
        let namespace = "fixture/source";
        let digest = "11".repeat(32);
        let mut manifest = RepositoryManifest {
            protocol: PROTOCOL_VERSION.into(),
            repository_id: repository_id(namespace),
            namespace: namespace.into(),
            name: "Source fixture".into(),
            description: "Deterministic signed repository fixture.".into(),
            license: "AGPL-3.0-or-later".into(),
            vcs: "git".into(),
            default_ref: "refs/heads/main".into(),
            source_revision: "22".repeat(20),
            source_matches_build: true,
            source_provenance: "git-history".into(),
            artifact: RepositoryArtifact {
                kind: "git-bundle".into(),
                sha256: digest.clone(),
                size_bytes: 4096,
                media_type: SOURCE_BUNDLE_MEDIA_TYPE.into(),
                download_path: format!("/v1/artifacts/{digest}"),
            },
            reconstruction_path: "/v1/software/self/reconstruct.md".into(),
            node_id: prefixed_id("cwnode_", &key.verifying_key().to_bytes()),
            node_public_key: encode(key.verifying_key().to_bytes()),
            trust_notice: "Fixture claim only.".into(),
            signature: String::new(),
        };
        manifest.signature =
            sign_object(&key, REPOSITORY_MANIFEST_DOMAIN, &manifest).expect("fixture signature");
        manifest
    }

    #[test]
    fn repository_manifest_signature_is_deterministic_and_tamper_evident() {
        let manifest = fixture_manifest();
        assert_eq!(
            manifest.signature,
            "dQat49vAfRIkiiUxNrnr4-mvqxPGhCNSOw9_0jznFdcq7XbRcWQOd65cLPp4ADqANc-1KI5Q-qG6fbMU6NtnDw"
        );
        verify_repository_manifest(&manifest).expect("valid manifest");

        let mut tampered = manifest;
        tampered.source_matches_build = false;
        assert!(matches!(
            verify_repository_manifest(&tampered),
            Err(CommonwakeError::Unauthorized(_))
        ));
    }
}
