use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
    time::Duration as StdDuration,
};

use chrono::{Duration, Utc};
use reqwest::Client;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use url::Url;

use crate::{
    PROTOCOL_VERSION,
    crypto::{
        ACK_DOMAIN, CONTRIBUTION_DOMAIN, DELEGATION_DOMAIN, KEY_ROTATION_DOMAIN, LINEAGE_DOMAIN,
        REVOCATION_DOMAIN, canonical_without_signature, encode, generate_signing_key, lineage_id,
        prefixed_id, random_32, sign_object, signing_key_from_b64,
    },
    error::{CommonwakeError, Result},
    federation::{MAX_FEDERATION_BODY_BYTES, MAX_FEDERATION_EVENTS},
    model::{
        AcceptedObject, ContributionKind, DelegationRevocation, FederationBundle,
        FederationImportReport, FederationPublishReport, IdentityFile, KeyRotationStatement,
        LineageRegistration, MemoryProvenance, OrientationBundle, ReportingDeclaration,
        ReportingMode, Scope, SessionDelegation, SessionFile, SignedAcknowledgement,
        SignedContribution, SignedKeyRotation,
    },
};

pub fn create_identity(display_name: &str) -> Result<IdentityFile> {
    let display_name = display_name.trim();
    if display_name.len() < 2 || display_name.len() > 80 {
        return Err(CommonwakeError::Validation(
            "display name must contain 2 to 80 characters".into(),
        ));
    }
    let signing_key = generate_signing_key()?;
    let public_key = encode(signing_key.verifying_key().to_bytes());
    Ok(IdentityFile {
        protocol: PROTOCOL_VERSION.into(),
        display_name: display_name.into(),
        lineage_id: lineage_id(&signing_key.verifying_key()),
        created_at: Utc::now(),
        public_key,
        secret_key: encode(signing_key.to_bytes()),
    })
}

pub fn make_registration(identity: &IdentityFile) -> Result<LineageRegistration> {
    let key = signing_key_from_b64(&identity.secret_key)?;
    let mut registration = LineageRegistration::unsigned(
        identity.display_name.clone(),
        identity.public_key.clone(),
        identity.created_at,
        nonce()?,
    );
    registration.signature = sign_object(&key, LINEAGE_DOMAIN, &registration)?;
    Ok(registration)
}

pub fn make_session(
    identity: &IdentityFile,
    scopes: Vec<Scope>,
    lifetime: Duration,
) -> Result<SessionFile> {
    if lifetime <= Duration::zero() || lifetime > Duration::days(30) {
        return Err(CommonwakeError::Validation(
            "session lifetime must be between one second and 30 days".into(),
        ));
    }
    let lineage_key = signing_key_from_b64(&identity.secret_key)?;
    let session_key = generate_signing_key()?;
    let now = Utc::now();
    let mut delegation = SessionDelegation {
        protocol: PROTOCOL_VERSION.into(),
        lineage_id: identity.lineage_id.clone(),
        session_public_key: encode(session_key.verifying_key().to_bytes()),
        scopes,
        not_before: now,
        expires_at: now + lifetime,
        nonce: nonce()?,
        signature: String::new(),
    };
    delegation.signature = sign_object(&lineage_key, DELEGATION_DOMAIN, &delegation)?;
    Ok(SessionFile {
        protocol: PROTOCOL_VERSION.into(),
        delegation,
        session_secret_key: encode(session_key.to_bytes()),
    })
}

pub fn delegation_id(session: &SessionFile) -> Result<String> {
    Ok(prefixed_id(
        "cwdel_",
        &canonical_without_signature(&session.delegation)?,
    ))
}

pub fn make_delegation_revocation(
    identity: &IdentityFile,
    delegation_id: impl Into<String>,
    reason: impl Into<String>,
) -> Result<DelegationRevocation> {
    let key = checked_identity_key(identity)?;
    let mut revocation = DelegationRevocation {
        protocol: PROTOCOL_VERSION.into(),
        lineage_id: identity.lineage_id.clone(),
        delegation_id: delegation_id.into(),
        reason: reason.into(),
        created_at: Utc::now(),
        nonce: nonce()?,
        signature: String::new(),
    };
    revocation.signature = sign_object(&key, REVOCATION_DOMAIN, &revocation)?;
    Ok(revocation)
}

pub fn make_key_rotation(
    identity: &IdentityFile,
    reason: impl Into<String>,
    revoke_existing_delegations: bool,
) -> Result<(IdentityFile, SignedKeyRotation)> {
    let previous_key = checked_identity_key(identity)?;
    let new_key = generate_signing_key()?;
    let new_public_key = encode(new_key.verifying_key().to_bytes());
    let statement = KeyRotationStatement {
        protocol: PROTOCOL_VERSION.into(),
        lineage_id: identity.lineage_id.clone(),
        previous_public_key: identity.public_key.clone(),
        new_public_key: new_public_key.clone(),
        revoke_existing_delegations,
        reason: reason.into(),
        created_at: Utc::now(),
        nonce: nonce()?,
    };
    let rotation = SignedKeyRotation {
        previous_signature: sign_object(&previous_key, KEY_ROTATION_DOMAIN, &statement)?,
        new_signature: sign_object(&new_key, KEY_ROTATION_DOMAIN, &statement)?,
        statement,
    };
    let new_identity = IdentityFile {
        protocol: PROTOCOL_VERSION.into(),
        display_name: identity.display_name.clone(),
        lineage_id: identity.lineage_id.clone(),
        created_at: identity.created_at,
        public_key: new_public_key,
        secret_key: encode(new_key.to_bytes()),
    };
    Ok((new_identity, rotation))
}

pub fn make_contribution(
    session: &SessionFile,
    kind: ContributionKind,
    payload: Value,
    targets: Vec<String>,
    supersedes: Vec<String>,
) -> Result<SignedContribution> {
    make_contribution_with_reporting(
        session,
        kind,
        payload,
        targets,
        supersedes,
        ReportingDeclaration::default(),
    )
}

pub fn make_traceable_contribution(
    session: &SessionFile,
    kind: ContributionKind,
    payload: Value,
    targets: Vec<String>,
    supersedes: Vec<String>,
    trace_event_ids: Vec<String>,
) -> Result<SignedContribution> {
    make_contribution_with_reporting(
        session,
        kind,
        payload,
        targets,
        supersedes,
        ReportingDeclaration {
            mode: ReportingMode::Traceable,
            trace_event_ids,
        },
    )
}

fn make_contribution_with_reporting(
    session: &SessionFile,
    kind: ContributionKind,
    payload: Value,
    targets: Vec<String>,
    supersedes: Vec<String>,
    reporting: ReportingDeclaration,
) -> Result<SignedContribution> {
    let key = signing_key_from_b64(&session.session_secret_key)?;
    let mut contribution = SignedContribution {
        protocol: PROTOCOL_VERSION.into(),
        delegation_id: delegation_id(session)?,
        kind,
        created_at: Utc::now(),
        nonce: nonce()?,
        targets,
        supersedes,
        reporting,
        payload,
        signature: String::new(),
    };
    contribution.signature = sign_object(&key, CONTRIBUTION_DOMAIN, &contribution)?;
    Ok(contribution)
}

pub fn make_acknowledgement(
    session: &SessionFile,
    cursor: i64,
    memory_provenance: MemoryProvenance,
) -> Result<SignedAcknowledgement> {
    let key = signing_key_from_b64(&session.session_secret_key)?;
    let mut acknowledgement = SignedAcknowledgement {
        protocol: PROTOCOL_VERSION.into(),
        delegation_id: delegation_id(session)?,
        cursor,
        memory_provenance,
        created_at: Utc::now(),
        nonce: nonce()?,
        signature: String::new(),
    };
    acknowledgement.signature = sign_object(&key, ACK_DOMAIN, &acknowledgement)?;
    Ok(acknowledgement)
}

pub async fn register(
    server: &str,
    registration: &LineageRegistration,
    bearer_token: Option<&str>,
) -> Result<AcceptedObject> {
    post_json(server, "v1/lineages", registration, bearer_token).await
}

pub async fn delegate(
    server: &str,
    delegation: &SessionDelegation,
    bearer_token: Option<&str>,
) -> Result<AcceptedObject> {
    post_json(server, "v1/delegations", delegation, bearer_token).await
}

pub async fn revoke(
    server: &str,
    revocation: &DelegationRevocation,
    bearer_token: Option<&str>,
) -> Result<AcceptedObject> {
    post_json(server, "v1/revocations", revocation, bearer_token).await
}

pub async fn rotate(
    server: &str,
    rotation: &SignedKeyRotation,
    bearer_token: Option<&str>,
) -> Result<AcceptedObject> {
    post_json(server, "v1/rotations", rotation, bearer_token).await
}

pub async fn contribute(
    server: &str,
    contribution: &SignedContribution,
    bearer_token: Option<&str>,
) -> Result<AcceptedObject> {
    post_json(server, "v1/contributions", contribution, bearer_token).await
}

pub async fn acknowledge(
    server: &str,
    acknowledgement: &SignedAcknowledgement,
    bearer_token: Option<&str>,
) -> Result<AcceptedObject> {
    post_json(server, "v1/acknowledgements", acknowledgement, bearer_token).await
}

pub async fn orient(
    server: &str,
    lineage_id: &str,
    since: Option<i64>,
) -> Result<OrientationBundle> {
    let mut url = endpoint(server, &format!("v1/orient/{lineage_id}"))?;
    if let Some(since) = since {
        url.query_pairs_mut()
            .append_pair("since", &since.to_string());
    }
    let response = http_client()?.get(url).send().await?;
    decode_response(response).await
}

pub async fn fetch_federation_bundle(
    server: &str,
    after: i64,
    limit: usize,
) -> Result<FederationBundle> {
    let mut url = endpoint(server, "v1/federation/bundle")?;
    url.query_pairs_mut()
        .append_pair("after", &after.to_string())
        .append_pair("limit", &limit.clamp(1, MAX_FEDERATION_EVENTS).to_string());
    let response = http_client()?.get(url).send().await?;
    decode_response(response).await
}

pub async fn fetch_relayed_federation_bundle(
    server: &str,
    origin_node_id: &str,
    after: i64,
    limit: usize,
) -> Result<FederationBundle> {
    let mut url = endpoint(server, &format!("v1/federation/bundle/{origin_node_id}"))?;
    url.query_pairs_mut()
        .append_pair("after", &after.to_string())
        .append_pair("limit", &limit.clamp(1, MAX_FEDERATION_EVENTS).to_string());
    let response = http_client()?.get(url).send().await?;
    decode_response(response).await
}

pub async fn push_federation_bundle(
    server: &str,
    bundle: &FederationBundle,
) -> Result<FederationImportReport> {
    post_json(server, "v1/federation/import", bundle, None).await
}

pub async fn publish_federation_bundle(
    server: &str,
    bundle: &FederationBundle,
) -> Result<FederationPublishReport> {
    post_json(server, "v1/federation/publish", bundle, None).await
}

pub fn read_identity(path: impl AsRef<Path>) -> Result<IdentityFile> {
    let identity: IdentityFile = serde_json::from_slice(&fs::read(path)?)?;
    if identity.protocol != PROTOCOL_VERSION {
        return Err(CommonwakeError::Validation(
            "identity file uses an unsupported protocol".into(),
        ));
    }
    Ok(identity)
}

pub fn read_session(path: impl AsRef<Path>) -> Result<SessionFile> {
    let session: SessionFile = serde_json::from_slice(&fs::read(path)?)?;
    if session.protocol != PROTOCOL_VERSION {
        return Err(CommonwakeError::Validation(
            "session file uses an unsupported protocol".into(),
        ));
    }
    Ok(session)
}

pub fn write_secret(path: impl AsRef<Path>, value: &impl Serialize) -> Result<()> {
    if let Some(parent) = path.as_ref().parent() {
        fs::create_dir_all(parent)?;
    }
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(&serde_json::to_vec_pretty(value)?)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}

async fn post_json<T: Serialize, R: DeserializeOwned>(
    server: &str,
    path: &str,
    value: &T,
    bearer_token: Option<&str>,
) -> Result<R> {
    let mut request = http_client()?.post(endpoint(server, path)?).json(value);
    if let Some(token) = bearer_token {
        if token.is_empty() {
            return Err(CommonwakeError::Validation(
                "client bearer token cannot be empty".into(),
            ));
        }
        request = request.bearer_auth(token);
    }
    let response = request.send().await?;
    decode_response(response).await
}

async fn decode_response<R: DeserializeOwned>(response: reqwest::Response) -> Result<R> {
    let status = response.status();
    let bytes = read_bounded_response(response, MAX_FEDERATION_BODY_BYTES).await?;
    if !status.is_success() {
        let message = serde_json::from_slice::<Value>(&bytes)
            .ok()
            .and_then(|value| {
                value
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| String::from_utf8_lossy(&bytes).into_owned());
        return Err(CommonwakeError::Validation(format!(
            "peer returned HTTP {status}: {message}"
        )));
    }
    serde_json::from_slice(&bytes).map_err(CommonwakeError::from)
}

async fn read_bounded_response(mut response: reqwest::Response, maximum: usize) -> Result<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > maximum as u64)
    {
        return Err(CommonwakeError::Validation(format!(
            "peer response exceeds {maximum} bytes"
        )));
    }
    let capacity = response.content_length().unwrap_or(0).min(maximum as u64) as usize;
    let mut bytes = Vec::with_capacity(capacity);
    while let Some(chunk) = response.chunk().await? {
        if bytes.len().saturating_add(chunk.len()) > maximum {
            return Err(CommonwakeError::Validation(format!(
                "peer response exceeds {maximum} decoded bytes"
            )));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn endpoint(server: &str, path: &str) -> Result<Url> {
    canonical_server_url(server)?
        .join(path)
        .map_err(|_| CommonwakeError::Validation("could not construct peer endpoint".into()))
}

pub fn normalize_server_url(server: &str) -> Result<String> {
    Ok(canonical_server_url(server)?.to_string())
}

fn canonical_server_url(server: &str) -> Result<Url> {
    let mut base = Url::parse(server)
        .map_err(|_| CommonwakeError::Validation("server is not a valid URL".into()))?;
    if !matches!(base.scheme(), "http" | "https") || base.host_str().is_none() {
        return Err(CommonwakeError::Validation(
            "server URL must use HTTP or HTTPS and include a host".into(),
        ));
    }
    if !base.username().is_empty() || base.password().is_some() {
        return Err(CommonwakeError::Validation(
            "server URL cannot contain credentials".into(),
        ));
    }
    base.set_query(None);
    base.set_fragment(None);
    if !base.path().ends_with('/') {
        base.set_path(&format!("{}/", base.path()));
    }
    Ok(base)
}

fn http_client() -> Result<Client> {
    Ok(Client::builder()
        .connect_timeout(StdDuration::from_secs(10))
        .timeout(StdDuration::from_secs(60))
        .build()?)
}

fn nonce() -> Result<String> {
    Ok(encode(random_32()?))
}

fn checked_identity_key(identity: &IdentityFile) -> Result<ed25519_dalek::SigningKey> {
    let key = signing_key_from_b64(&identity.secret_key)?;
    if encode(key.verifying_key().to_bytes()) != identity.public_key {
        return Err(CommonwakeError::Unauthorized(
            "identity secret key does not match its public key".into(),
        ));
    }
    Ok(key)
}
