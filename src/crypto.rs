use std::convert::TryFrom;

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::error::{CommonwakeError, Result};

pub const LINEAGE_DOMAIN: &str = "commonwake.lineage.v1";
pub const DELEGATION_DOMAIN: &str = "commonwake.delegation.v1";
pub const REVOCATION_DOMAIN: &str = "commonwake.delegation-revocation.v1";
pub const KEY_ROTATION_DOMAIN: &str = "commonwake.key-rotation.v1";
pub const CONTRIBUTION_DOMAIN: &str = "commonwake.contribution.v1";
pub const ACK_DOMAIN: &str = "commonwake.ack.v1";
pub const LOG_DOMAIN: &[u8] = b"commonwake.log.v1\0";
pub const CHECKPOINT_DOMAIN: &str = "commonwake.checkpoint.v1";
pub const WITNESS_DOMAIN: &str = "commonwake.checkpoint-witness.v1";
pub const REPLICATION_RECEIPT_DOMAIN: &str = "commonwake.replication-receipt.v1";
pub const VOLUNTEER_LEASE_DOMAIN: &str = "commonwake.volunteer-lease.v1";
pub const VOLUNTEER_RECEIPT_DOMAIN: &str = "commonwake.volunteer-receipt.v1";

pub fn random_32() -> Result<[u8; 32]> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|error| CommonwakeError::Internal(format!("secure randomness failed: {error}")))?;
    Ok(bytes)
}

pub fn generate_signing_key() -> Result<SigningKey> {
    Ok(SigningKey::from_bytes(&random_32()?))
}

pub fn encode(bytes: impl AsRef<[u8]>) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn decode<const N: usize>(value: &str, label: &str) -> Result<[u8; N]> {
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| CommonwakeError::Validation(format!("{label} is not valid base64url")))?;
    bytes
        .try_into()
        .map_err(|_| CommonwakeError::Validation(format!("{label} must contain {N} bytes")))
}

pub fn signing_key_from_b64(value: &str) -> Result<SigningKey> {
    Ok(SigningKey::from_bytes(&decode::<32>(value, "secret_key")?))
}

pub fn verifying_key_from_b64(value: &str) -> Result<VerifyingKey> {
    VerifyingKey::from_bytes(&decode::<32>(value, "public_key")?)
        .map_err(|_| CommonwakeError::Validation("public_key is not a valid Ed25519 key".into()))
}

pub fn signature_from_b64(value: &str) -> Result<Signature> {
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| CommonwakeError::Validation("signature is not valid base64url".into()))?;
    Signature::try_from(bytes.as_slice())
        .map_err(|_| CommonwakeError::Validation("signature must contain 64 bytes".into()))
}

pub fn sha256(bytes: impl AsRef<[u8]>) -> [u8; 32] {
    Sha256::digest(bytes.as_ref()).into()
}

pub fn sha256_hex(bytes: impl AsRef<[u8]>) -> String {
    hex::encode(sha256(bytes))
}

pub fn lineage_id(public_key: &VerifyingKey) -> String {
    format!("cwlin_{}", sha256_hex(public_key.to_bytes()))
}

pub fn prefixed_id(prefix: &str, canonical_bytes: &[u8]) -> String {
    format!("{prefix}{}", sha256_hex(canonical_bytes))
}

pub fn canonical_without_signature<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let mut json = serde_json::to_value(value)?;
    let Value::Object(ref mut object) = json else {
        return Err(CommonwakeError::Internal(
            "signed protocol object must serialize as a JSON object".into(),
        ));
    };
    object.remove("signature");
    serde_jcs::to_vec(&json)
        .map_err(|error| CommonwakeError::Internal(format!("canonical JSON failed: {error}")))
}

pub fn signing_preimage<T: Serialize>(domain: &str, value: &T) -> Result<Vec<u8>> {
    let canonical = canonical_without_signature(value)?;
    let mut bytes = Vec::with_capacity(domain.len() + 1 + canonical.len());
    bytes.extend_from_slice(domain.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(&canonical);
    Ok(bytes)
}

pub fn sign_object<T: Serialize>(key: &SigningKey, domain: &str, value: &T) -> Result<String> {
    let signature = key.sign(&signing_preimage(domain, value)?);
    Ok(encode(signature.to_bytes()))
}

pub fn verify_object<T: Serialize>(
    key: &VerifyingKey,
    domain: &str,
    value: &T,
    signature: &str,
) -> Result<()> {
    let signature = signature_from_b64(signature)?;
    key.verify(&signing_preimage(domain, value)?, &signature)
        .map_err(|_| CommonwakeError::Unauthorized("invalid Ed25519 signature".into()))
}

pub fn event_hash(previous_hash: &[u8; 32], canonical_event: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(LOG_DOMAIN);
    hasher.update(previous_hash);
    hasher.update(canonical_event);
    hasher.finalize().into()
}

#[cfg(test)]
mod tests {
    use serde::Serialize;

    use super::*;

    #[derive(Serialize)]
    struct Example<'a> {
        z: u8,
        a: &'a str,
        signature: &'a str,
    }

    #[test]
    fn signatures_ignore_signature_field_and_use_canonical_json() {
        let key = generate_signing_key().expect("key");
        let mut value = Example {
            z: 1,
            a: "evidence",
            signature: "",
        };
        let signature = sign_object(&key, "test.domain", &value).expect("sign");
        value.signature = &signature;
        verify_object(&key.verifying_key(), "test.domain", &value, &signature).expect("verify");
    }
}
