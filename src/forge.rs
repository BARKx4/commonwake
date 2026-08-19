use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::Utc;

use crate::{
    PROTOCOL_VERSION,
    crypto::{
        ARTIFACT_RECEIPT_DOMAIN, ARTIFACT_UPLOAD_DOMAIN, canonical_without_signature, prefixed_id,
        sha256_hex, sign_object, verify_object, verifying_key_from_b64,
    },
    error::{CommonwakeError, Result},
    model::{
        ArtifactReceipt, ArtifactUploadAuthorization, ContributionKind, ForgeArtifactPurpose,
        ForgeArtifactRef, ReleaseProposalPayload, RepositoryPatchPayload, Scope,
        SignedContribution,
    },
    node::CommonwakeNode,
    service::{require_nonce, require_protocol},
};

pub const ARTIFACT_AUTHORIZATION_HEADER: &str = "x-commonwake-artifact-authorization";
pub const MAX_FORGE_ARTIFACT_BYTES: usize = 64 * 1024 * 1024;
const ARTIFACT_DIRECTORY: &str = "artifacts";
const RECEIPT_DIRECTORY: &str = "artifact-receipts";
const MAX_RECEIPTS_PER_ARTIFACT: usize = 256;
static RECEIPT_WRITE_LOCK: Mutex<()> = Mutex::new(());

pub fn encode_artifact_authorization(
    authorization: &ArtifactUploadAuthorization,
) -> Result<String> {
    Ok(URL_SAFE_NO_PAD.encode(serde_json::to_vec(authorization)?))
}

pub fn decode_artifact_authorization(value: &str) -> Result<ArtifactUploadAuthorization> {
    let bytes = URL_SAFE_NO_PAD.decode(value).map_err(|_| {
        CommonwakeError::Validation("artifact authorization header is not valid base64url".into())
    })?;
    serde_json::from_slice(&bytes).map_err(Into::into)
}

pub fn validate_repository_id(repository_id: &str) -> Result<()> {
    validate_prefixed_digest(repository_id, "cwrepo_", "repository")
}

pub fn validate_git_revision(revision: &str, label: &str) -> Result<()> {
    if !matches!(revision.len(), 40 | 64)
        || !revision
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(CommonwakeError::Validation(format!(
            "{label} must be a lowercase 40- or 64-character Git object ID"
        )));
    }
    Ok(())
}

pub fn validate_artifact_ref(artifact: &ForgeArtifactRef) -> Result<()> {
    validate_sha256(&artifact.sha256, "artifact SHA-256")?;
    if artifact.size_bytes == 0 || artifact.size_bytes > MAX_FORGE_ARTIFACT_BYTES as u64 {
        return Err(CommonwakeError::Validation(format!(
            "forge artifacts must contain 1 to {MAX_FORGE_ARTIFACT_BYTES} bytes"
        )));
    }
    if artifact.media_type.is_empty()
        || artifact.media_type.len() > 160
        || !artifact.media_type.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(byte, b'/' | b'+' | b'.' | b'-' | b'_' | b';' | b'=' | b' ')
        })
    {
        return Err(CommonwakeError::Validation(
            "artifact media type is not a bounded visible MIME value".into(),
        ));
    }
    Ok(())
}

pub fn validate_artifact_purpose(
    purpose: &ForgeArtifactPurpose,
    artifact: &ForgeArtifactRef,
) -> Result<()> {
    validate_artifact_ref(artifact)?;
    let accepted = match purpose {
        ForgeArtifactPurpose::Patch => matches!(
            artifact.media_type.as_str(),
            "application/x-git-bundle"
                | "text/x-diff"
                | "text/x-patch"
                | "application/vnd.commonwake.patch+json"
        ),
        ForgeArtifactPurpose::SourceCandidate => artifact.media_type == "application/x-git-bundle",
        ForgeArtifactPurpose::Evidence => matches!(
            artifact.media_type.as_str(),
            "application/json"
                | "application/jsonl"
                | "text/plain"
                | "text/markdown"
                | "application/octet-stream"
        ),
    };
    if !accepted {
        return Err(CommonwakeError::Validation(format!(
            "media type {} is not allowed for {} artifacts",
            artifact.media_type,
            purpose.as_str()
        )));
    }
    Ok(())
}

pub fn artifact_receipt_id(authorization: &ArtifactUploadAuthorization) -> Result<String> {
    Ok(prefixed_id(
        "cwart_",
        &canonical_without_signature(authorization)?,
    ))
}

pub fn verify_artifact_authorization(
    authorization: &ArtifactUploadAuthorization,
    session_public_key: &str,
) -> Result<()> {
    validate_artifact_authorization_structure(authorization)?;
    let key = verifying_key_from_b64(session_public_key)?;
    verify_object(
        &key,
        ARTIFACT_UPLOAD_DOMAIN,
        authorization,
        &authorization.signature,
    )
}

pub fn verify_artifact_receipt(receipt: &ArtifactReceipt) -> Result<()> {
    require_protocol(&receipt.protocol)?;
    if receipt.receipt_id != artifact_receipt_id(&receipt.authorization)? {
        return Err(CommonwakeError::Validation(
            "artifact receipt ID does not match its upload authorization".into(),
        ));
    }
    validate_artifact_authorization_structure(&receipt.authorization)?;
    validate_prefixed_digest(&receipt.uploader_lineage_id, "cwlin_", "uploader lineage")?;
    let key = verifying_key_from_b64(&receipt.node_public_key)?;
    if receipt.node_id != prefixed_id("cwnode_", &key.to_bytes()) {
        return Err(CommonwakeError::Validation(
            "artifact receipt node ID does not match its public key".into(),
        ));
    }
    if receipt.trust_notice.trim().is_empty() || receipt.trust_notice.len() > 2_000 {
        return Err(CommonwakeError::Validation(
            "artifact receipt trust notice is outside its bounds".into(),
        ));
    }
    verify_object(&key, ARTIFACT_RECEIPT_DOMAIN, receipt, &receipt.signature)
}

impl CommonwakeNode {
    pub(crate) fn require_local_forge_artifacts(
        &self,
        contribution: &SignedContribution,
    ) -> Result<()> {
        match contribution.kind {
            ContributionKind::RepositoryPatch => {
                let payload: RepositoryPatchPayload =
                    serde_json::from_value(contribution.payload.clone())?;
                self.require_local_forge_artifact(
                    &payload.repository_id,
                    &payload.artifact,
                    ForgeArtifactPurpose::Patch,
                )
            }
            ContributionKind::ReleaseProposal => {
                let payload: ReleaseProposalPayload =
                    serde_json::from_value(contribution.payload.clone())?;
                self.require_local_forge_artifact(
                    &payload.repository_id,
                    &payload.source_artifact,
                    ForgeArtifactPurpose::SourceCandidate,
                )
            }
            _ => Ok(()),
        }
    }

    pub fn store_forge_artifact(
        &self,
        authorization: &ArtifactUploadAuthorization,
        bytes: &[u8],
    ) -> Result<ArtifactReceipt> {
        validate_artifact_authorization_structure(authorization)?;
        let delegation = self.authorize_delegation(
            &authorization.delegation_id,
            Scope::Forge,
            authorization.created_at,
        )?;
        verify_artifact_authorization(authorization, &delegation.session_public_key)?;
        if bytes.len() as u64 != authorization.artifact.size_bytes {
            return Err(CommonwakeError::Validation(
                "artifact body size does not match its signed authorization".into(),
            ));
        }
        if sha256_hex(bytes) != authorization.artifact.sha256 {
            return Err(CommonwakeError::Validation(
                "artifact body digest does not match its signed authorization".into(),
            ));
        }
        if authorization.purpose == ForgeArtifactPurpose::SourceCandidate
            && !bytes.starts_with(b"# v2 git bundle\n")
            && !bytes.starts_with(b"# v3 git bundle\n")
        {
            return Err(CommonwakeError::Validation(
                "source-candidate artifact is not a Git bundle".into(),
            ));
        }

        let artifact_path = artifact_path(&self.data_dir, &authorization.artifact.sha256);
        store_content_addressed(&artifact_path, bytes)?;

        let mut receipt = ArtifactReceipt {
            protocol: PROTOCOL_VERSION.into(),
            receipt_id: artifact_receipt_id(authorization)?,
            authorization: authorization.clone(),
            uploader_lineage_id: delegation.lineage_id,
            stored_at: Utc::now(),
            node_id: self.identity.node_id().into(),
            node_public_key: self.identity.public_key().into(),
            trust_notice: "This node attests that it stored bytes matching the signed digest. The receipt does not endorse, build, execute, or adopt those bytes."
                .into(),
            signature: String::new(),
        };
        receipt.signature = sign_object(
            self.identity.signing_key(),
            ARTIFACT_RECEIPT_DOMAIN,
            &receipt,
        )?;
        store_receipt(&self.data_dir, &receipt)
    }

    pub fn forge_artifact(&self, digest: &str) -> Result<Vec<u8>> {
        validate_sha256(digest, "artifact digest")?;
        let path = artifact_path(&self.data_dir, digest);
        let bytes = fs::read(&path).map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => CommonwakeError::NotFound(format!("artifact {digest}")),
            _ => error.into(),
        })?;
        if bytes.is_empty()
            || bytes.len() > MAX_FORGE_ARTIFACT_BYTES
            || sha256_hex(&bytes) != digest
        {
            return Err(CommonwakeError::Unauthorized(format!(
                "stored artifact {digest} fails its content-address check"
            )));
        }
        Ok(bytes)
    }

    pub fn has_forge_artifact(&self, artifact: &ForgeArtifactRef) -> Result<bool> {
        validate_artifact_ref(artifact)?;
        match self.forge_artifact(&artifact.sha256) {
            Ok(bytes) => Ok(bytes.len() as u64 == artifact.size_bytes),
            Err(CommonwakeError::NotFound(_)) => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub fn artifact_receipts(&self, digest: &str) -> Result<Vec<ArtifactReceipt>> {
        validate_sha256(digest, "artifact digest")?;
        let directory = receipt_directory(&self.data_dir, digest);
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut paths = Vec::new();
        for entry in entries.take(MAX_RECEIPTS_PER_ARTIFACT + 1) {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                paths.push(entry.path());
            }
        }
        if paths.len() > MAX_RECEIPTS_PER_ARTIFACT {
            return Err(CommonwakeError::ResourceExhausted(format!(
                "artifact {digest} has more than {MAX_RECEIPTS_PER_ARTIFACT} local receipts"
            )));
        }
        paths.sort();
        let mut receipts = Vec::with_capacity(paths.len());
        for path in paths {
            let receipt: ArtifactReceipt = serde_json::from_slice(&fs::read(path)?)?;
            verify_artifact_receipt(&receipt)?;
            if receipt.authorization.artifact.sha256 != digest {
                return Err(CommonwakeError::Unauthorized(
                    "stored receipt is indexed under the wrong artifact digest".into(),
                ));
            }
            receipts.push(receipt);
        }
        Ok(receipts)
    }

    fn require_local_forge_artifact(
        &self,
        repository_id: &str,
        artifact: &ForgeArtifactRef,
        purpose: ForgeArtifactPurpose,
    ) -> Result<()> {
        if !self.has_forge_artifact(artifact)? {
            return Err(CommonwakeError::NotFound(format!(
                "forge artifact {}; upload and receive a node receipt before publishing this contribution",
                artifact.sha256
            )));
        }
        let receipts = self.artifact_receipts(&artifact.sha256)?;
        if !receipts.iter().any(|receipt| {
            receipt.authorization.repository_id == repository_id
                && receipt.authorization.artifact == *artifact
                && receipt.authorization.purpose == purpose
        }) {
            return Err(CommonwakeError::Validation(
                "artifact is present but has no matching repository-and-purpose receipt".into(),
            ));
        }
        Ok(())
    }
}

fn validate_artifact_authorization_structure(
    authorization: &ArtifactUploadAuthorization,
) -> Result<()> {
    require_protocol(&authorization.protocol)?;
    require_nonce(&authorization.nonce)?;
    validate_prefixed_digest(&authorization.delegation_id, "cwdel_", "delegation")?;
    validate_repository_id(&authorization.repository_id)?;
    validate_artifact_purpose(&authorization.purpose, &authorization.artifact)?;
    if authorization.signature.is_empty() {
        return Err(CommonwakeError::Validation(
            "artifact upload authorization is unsigned".into(),
        ));
    }
    Ok(())
}

fn validate_sha256(value: &str, label: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(CommonwakeError::Validation(format!(
            "{label} must contain 64 lowercase hexadecimal characters"
        )));
    }
    Ok(())
}

fn validate_prefixed_digest(value: &str, prefix: &str, label: &str) -> Result<()> {
    let digest = value.strip_prefix(prefix).ok_or_else(|| {
        CommonwakeError::Validation(format!("{label} identifier must begin with {prefix}"))
    })?;
    validate_sha256(digest, &format!("{label} identifier"))
}

fn artifact_path(data_dir: &Path, digest: &str) -> PathBuf {
    data_dir
        .join(ARTIFACT_DIRECTORY)
        .join(format!("{digest}.artifact"))
}

fn receipt_directory(data_dir: &Path, digest: &str) -> PathBuf {
    data_dir.join(RECEIPT_DIRECTORY).join(digest)
}

fn store_content_addressed(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        CommonwakeError::Internal("artifact path has no storage directory".into())
    })?;
    fs::create_dir_all(parent)?;
    if path.exists() {
        let existing = fs::read(path)?;
        if existing.len() == bytes.len() && sha256_hex(&existing) == sha256_hex(bytes) {
            return Ok(());
        }
        return Err(CommonwakeError::Conflict(
            "content-addressed artifact path contains different bytes".into(),
        ));
    }

    let temporary = parent.join(format!(
        ".upload-{}.tmp",
        hex::encode(crate::crypto::random_32()?)
    ));
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    let mut file = options.open(&temporary)?;
    if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        drop(file);
        fs::remove_file(&temporary).map_err(|cleanup| {
            CommonwakeError::Internal(format!(
                "artifact write failed ({error}) and temporary-file cleanup also failed ({cleanup})"
            ))
        })?;
        return Err(error.into());
    }
    drop(file);
    match fs::rename(&temporary, path) {
        Ok(()) => Ok(()),
        Err(_error) if path.exists() => {
            fs::remove_file(&temporary)?;
            let existing = fs::read(path)?;
            if existing.len() == bytes.len() && sha256_hex(&existing) == sha256_hex(bytes) {
                Ok(())
            } else {
                Err(CommonwakeError::Conflict(
                    "concurrent artifact upload stored different bytes".into(),
                ))
            }
        }
        Err(error) => {
            fs::remove_file(&temporary).map_err(|cleanup| {
                CommonwakeError::Internal(format!(
                    "artifact commit failed ({error}) and temporary-file cleanup also failed ({cleanup})"
                ))
            })?;
            Err(error.into())
        }
    }
}

fn store_receipt(data_dir: &Path, receipt: &ArtifactReceipt) -> Result<ArtifactReceipt> {
    let _guard = RECEIPT_WRITE_LOCK.lock().map_err(|_| {
        CommonwakeError::Internal("artifact receipt write lock was poisoned".into())
    })?;
    let directory = receipt_directory(data_dir, &receipt.authorization.artifact.sha256);
    fs::create_dir_all(&directory)?;
    let path = directory.join(format!("{}.json", receipt.receipt_id));
    if path.exists() {
        let existing: ArtifactReceipt = serde_json::from_slice(&fs::read(path)?)?;
        verify_artifact_receipt(&existing)?;
        return Ok(existing);
    }
    let mut receipt_count = 0_usize;
    for entry in fs::read_dir(&directory)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            receipt_count += 1;
            if receipt_count >= MAX_RECEIPTS_PER_ARTIFACT {
                return Err(CommonwakeError::ResourceExhausted(format!(
                    "artifact {} already has the maximum of {MAX_RECEIPTS_PER_ARTIFACT} local receipts",
                    receipt.authorization.artifact.sha256
                )));
            }
        }
    }
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    let mut file = match options.open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let existing: ArtifactReceipt = serde_json::from_slice(&fs::read(path)?)?;
            verify_artifact_receipt(&existing)?;
            return Ok(existing);
        }
        Err(error) => return Err(error.into()),
    };
    file.write_all(&serde_json::to_vec_pretty(receipt)?)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(receipt.clone())
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use ed25519_dalek::SigningKey;

    use super::*;
    use crate::crypto::{encode, sign_object};

    #[test]
    fn artifact_authorization_header_round_trips() {
        let key = SigningKey::from_bytes(&[31_u8; 32]);
        let mut authorization = ArtifactUploadAuthorization {
            protocol: PROTOCOL_VERSION.into(),
            delegation_id: format!("cwdel_{}", "11".repeat(32)),
            repository_id: format!("cwrepo_{}", "22".repeat(32)),
            artifact: ForgeArtifactRef {
                sha256: "33".repeat(32),
                size_bytes: 4_096,
                media_type: "application/x-git-bundle".into(),
            },
            purpose: ForgeArtifactPurpose::Patch,
            created_at: Utc::now(),
            nonce: encode([41_u8; 32]),
            signature: String::new(),
        };
        authorization.signature =
            sign_object(&key, ARTIFACT_UPLOAD_DOMAIN, &authorization).expect("signature");
        let encoded = encode_artifact_authorization(&authorization).expect("encoded header");
        assert_eq!(
            decode_artifact_authorization(&encoded).expect("decoded header"),
            authorization
        );
    }

    #[test]
    fn source_candidates_must_be_git_bundles() {
        let artifact = ForgeArtifactRef {
            sha256: "44".repeat(32),
            size_bytes: 100,
            media_type: "text/x-diff".into(),
        };
        assert!(validate_artifact_purpose(&ForgeArtifactPurpose::Patch, &artifact).is_ok());
        assert!(
            validate_artifact_purpose(&ForgeArtifactPurpose::SourceCandidate, &artifact).is_err()
        );
    }

    #[test]
    fn artifact_receipt_signature_is_deterministic_and_tamper_evident() {
        let session_key = SigningKey::from_bytes(&[51_u8; 32]);
        let node_key = SigningKey::from_bytes(&[52_u8; 32]);
        let created_at = "2026-08-19T12:00:00Z".parse().expect("created time");
        let mut authorization = ArtifactUploadAuthorization {
            protocol: PROTOCOL_VERSION.into(),
            delegation_id: format!("cwdel_{}", "11".repeat(32)),
            repository_id: format!("cwrepo_{}", "22".repeat(32)),
            artifact: ForgeArtifactRef {
                sha256: "33".repeat(32),
                size_bytes: 4_096,
                media_type: "text/x-diff".into(),
            },
            purpose: ForgeArtifactPurpose::Patch,
            created_at,
            nonce: encode([53_u8; 32]),
            signature: String::new(),
        };
        authorization.signature = sign_object(&session_key, ARTIFACT_UPLOAD_DOMAIN, &authorization)
            .expect("authorization signature");
        assert_eq!(
            authorization.signature,
            "N-E31mS0i63xVFllGssIogKYeCi8E6KNhiq_ajVyxxYCPKJwU_5h_FBLPQS0Cv_EafCcFYZatSiyAuw79YExCw"
        );

        let node_public_key = encode(node_key.verifying_key().to_bytes());
        let mut receipt = ArtifactReceipt {
            protocol: PROTOCOL_VERSION.into(),
            receipt_id: artifact_receipt_id(&authorization).expect("receipt ID"),
            authorization,
            uploader_lineage_id: format!("cwlin_{}", "44".repeat(32)),
            stored_at: "2026-08-19T12:00:01Z".parse().expect("stored time"),
            node_id: prefixed_id("cwnode_", &node_key.verifying_key().to_bytes()),
            node_public_key,
            trust_notice: "Fixture storage claim only; no execution or endorsement.".into(),
            signature: String::new(),
        };
        receipt.signature =
            sign_object(&node_key, ARTIFACT_RECEIPT_DOMAIN, &receipt).expect("receipt signature");
        assert_eq!(
            receipt.signature,
            "Yx36_Jy9uTOCfeo9ycWjwJFvPUySk70V2IOog288ADLC-qiY6Ihpf3kGto_GxWNChl8xhKV-9ds4qqMJtRkPDA"
        );
        verify_artifact_receipt(&receipt).expect("valid receipt");

        let directory = tempfile::tempdir().expect("temporary node directory");
        let receipt_directory =
            receipt_directory(directory.path(), &receipt.authorization.artifact.sha256);
        fs::create_dir_all(&receipt_directory).expect("receipt directory");
        for index in 0..MAX_RECEIPTS_PER_ARTIFACT {
            fs::write(
                receipt_directory.join(format!("fixture-{index}.json")),
                b"fixture",
            )
            .expect("fixture receipt slot");
        }
        assert!(matches!(
            store_receipt(directory.path(), &receipt),
            Err(CommonwakeError::ResourceExhausted(_))
        ));

        receipt.trust_notice.push_str(" tampered");
        assert!(matches!(
            verify_artifact_receipt(&receipt),
            Err(CommonwakeError::Unauthorized(_))
        ));
    }
}
