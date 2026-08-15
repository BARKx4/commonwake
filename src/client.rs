use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};

use chrono::{Duration, Utc};
use reqwest::Client;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use url::Url;

use crate::{
    PROTOCOL_VERSION,
    crypto::{
        ACK_DOMAIN, CONTRIBUTION_DOMAIN, DELEGATION_DOMAIN, LINEAGE_DOMAIN,
        canonical_without_signature, encode, generate_signing_key, lineage_id, prefixed_id,
        random_32, sign_object, signing_key_from_b64,
    },
    error::{CommonwakeError, Result},
    model::{
        AcceptedObject, ContributionKind, IdentityFile, LineageRegistration, MemoryProvenance,
        OrientationBundle, Scope, SessionDelegation, SessionFile, SignedAcknowledgement,
        SignedContribution,
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

pub fn make_contribution(
    session: &SessionFile,
    kind: ContributionKind,
    payload: Value,
    targets: Vec<String>,
    supersedes: Vec<String>,
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

pub async fn register(server: &str, registration: &LineageRegistration) -> Result<AcceptedObject> {
    post_json(server, "v1/lineages", registration).await
}

pub async fn delegate(server: &str, delegation: &SessionDelegation) -> Result<AcceptedObject> {
    post_json(server, "v1/delegations", delegation).await
}

pub async fn contribute(server: &str, contribution: &SignedContribution) -> Result<AcceptedObject> {
    post_json(server, "v1/contributions", contribution).await
}

pub async fn acknowledge(
    server: &str,
    acknowledgement: &SignedAcknowledgement,
) -> Result<AcceptedObject> {
    post_json(server, "v1/acknowledgements", acknowledgement).await
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
    let response = Client::new().get(url).send().await?;
    decode_response(response).await
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
) -> Result<R> {
    let response = Client::new()
        .post(endpoint(server, path)?)
        .json(value)
        .send()
        .await?;
    decode_response(response).await
}

async fn decode_response<R: DeserializeOwned>(response: reqwest::Response) -> Result<R> {
    let status = response.status();
    let bytes = response.bytes().await?;
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

fn endpoint(server: &str, path: &str) -> Result<Url> {
    let mut base = Url::parse(server)
        .map_err(|_| CommonwakeError::Validation("server is not a valid URL".into()))?;
    if !base.path().ends_with('/') {
        base.set_path(&format!("{}/", base.path()));
    }
    base.join(path)
        .map_err(|_| CommonwakeError::Validation("could not construct peer endpoint".into()))
}

fn nonce() -> Result<String> {
    Ok(encode(random_32()?))
}
