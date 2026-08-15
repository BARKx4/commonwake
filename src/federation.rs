use chrono::Utc;
use ed25519_dalek::Verifier;
use serde_json::json;

use crate::{
    PROTOCOL_VERSION,
    crypto::{
        CHECKPOINT_DOMAIN, WITNESS_DOMAIN, event_hash, prefixed_id, sign_object,
        signature_from_b64, verifying_key_from_b64,
    },
    error::{CommonwakeError, Result},
    model::{Checkpoint, CheckpointWitness, FederationBundle, FederationImportReport, OriginEvent},
    node::CommonwakeNode,
};

const ZERO_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";
pub const MAX_FEDERATION_EVENTS: usize = 500;
/// Maximum canonical JSON size of one durable protocol object.
///
/// A count-only federation limit is not a memory bound when one signed object
/// can contain arbitrary JSON. Local append and remote verification both
/// enforce this invariant so a valid node signature cannot hide an
/// unreasonably large event.
pub const MAX_CANONICAL_OBJECT_BYTES: usize = 64 * 1024;
/// Maximum decoded HTTP response or federation-import request size.
///
/// This covers a worst-case 500-event bundle with bounded canonical objects
/// and modest envelope overhead while still placing a finite allocation bound
/// on untrusted peers.
pub const MAX_FEDERATION_BODY_BYTES: usize = 40 * 1024 * 1024;

pub fn verify_bundle(bundle: &FederationBundle) -> Result<()> {
    if bundle.protocol != PROTOCOL_VERSION {
        return Err(CommonwakeError::Validation(format!(
            "unsupported federation protocol {}",
            bundle.protocol
        )));
    }
    if bundle.from_cursor < 0 || bundle.through_cursor < bundle.from_cursor {
        return Err(CommonwakeError::Validation(
            "federation cursor range is invalid".into(),
        ));
    }
    if bundle.events.len() > MAX_FEDERATION_EVENTS {
        return Err(CommonwakeError::Validation(format!(
            "federation bundle exceeds the {MAX_FEDERATION_EVENTS}-event protocol limit"
        )));
    }
    let node_key = verifying_key_from_b64(&bundle.origin_node_public_key)?;
    let derived_node_id = prefixed_id("cwnode_", &node_key.to_bytes());
    if derived_node_id != bundle.origin_node_id {
        return Err(CommonwakeError::Unauthorized(
            "origin node id does not match its public key".into(),
        ));
    }
    verify_checkpoint(&bundle.checkpoint)?;
    if bundle.checkpoint.node_id != bundle.origin_node_id
        || bundle.checkpoint.node_public_key != bundle.origin_node_public_key
        || bundle.checkpoint.cursor != bundle.through_cursor
    {
        return Err(CommonwakeError::Unauthorized(
            "bundle checkpoint does not describe the bundle origin and range".into(),
        ));
    }
    let expected_len = usize::try_from(bundle.through_cursor - bundle.from_cursor)
        .map_err(|_| CommonwakeError::Validation("federation range is too large".into()))?;
    if bundle.events.len() != expected_len {
        return Err(CommonwakeError::Validation(
            "bundle must contain every event in its declared cursor range".into(),
        ));
    }

    let mut previous_hash = None;
    for (offset, event) in bundle.events.iter().enumerate() {
        let encoded_canonical = serde_json::to_vec(&event.canonical)?;
        if encoded_canonical.len() > MAX_CANONICAL_OBJECT_BYTES {
            return Err(CommonwakeError::Validation(format!(
                "origin event {} canonical object exceeds {MAX_CANONICAL_OBJECT_BYTES} bytes",
                event.sequence
            )));
        }
        let expected_sequence = bundle.from_cursor + offset as i64 + 1;
        if event.sequence != expected_sequence {
            return Err(CommonwakeError::Validation(format!(
                "bundle event sequence {} is not the expected {expected_sequence}",
                event.sequence
            )));
        }
        if let Some(previous) = &previous_hash {
            if &event.previous_hash != previous {
                return Err(CommonwakeError::Unauthorized(format!(
                    "origin event {} does not extend the preceding event",
                    event.sequence
                )));
            }
        } else if bundle.from_cursor == 0 && event.previous_hash != ZERO_HASH {
            return Err(CommonwakeError::Unauthorized(
                "origin genesis event does not begin at the zero hash".into(),
            ));
        }
        let previous = decode_hash(&event.previous_hash, "previous_hash")?;
        let canonical_record = canonical_origin_record(event)?;
        let expected_hash = event_hash(&previous, &canonical_record);
        if event.event_hash != hex::encode(expected_hash) {
            return Err(CommonwakeError::Unauthorized(format!(
                "origin event {} has an invalid event hash",
                event.sequence
            )));
        }
        let expected_event_id = prefixed_id(
            "cwevt_",
            &serde_jcs::to_vec(&event.canonical).map_err(|error| {
                CommonwakeError::Internal(format!("canonical JSON failed: {error}"))
            })?,
        );
        if event.event_id != expected_event_id {
            return Err(CommonwakeError::Unauthorized(format!(
                "origin event {} has an invalid content id",
                event.sequence
            )));
        }
        let signature = signature_from_b64(&event.node_signature)?;
        node_key.verify(&expected_hash, &signature).map_err(|_| {
            CommonwakeError::Unauthorized(format!(
                "origin event {} has an invalid node signature",
                event.sequence
            ))
        })?;
        previous_hash = Some(event.event_hash.clone());
    }

    let expected_checkpoint_hash = bundle
        .events
        .last()
        .map_or_else(|| None, |event| Some(event.event_hash.as_str()));
    if let Some(expected) = expected_checkpoint_hash
        && bundle.checkpoint.event_hash != expected
    {
        return Err(CommonwakeError::Unauthorized(
            "bundle checkpoint does not commit to its final event".into(),
        ));
    }
    if bundle.through_cursor == 0 && bundle.checkpoint.event_hash != ZERO_HASH {
        return Err(CommonwakeError::Unauthorized(
            "genesis checkpoint does not use the zero hash".into(),
        ));
    }
    Ok(())
}

pub fn verify_checkpoint(checkpoint: &Checkpoint) -> Result<()> {
    let key = verifying_key_from_b64(&checkpoint.node_public_key)?;
    if prefixed_id("cwnode_", &key.to_bytes()) != checkpoint.node_id {
        return Err(CommonwakeError::Unauthorized(
            "checkpoint node id does not match its public key".into(),
        ));
    }
    crate::crypto::verify_object(&key, CHECKPOINT_DOMAIN, checkpoint, &checkpoint.signature)
}

pub(crate) fn canonical_origin_record(event: &OriginEvent) -> Result<Vec<u8>> {
    serde_jcs::to_vec(&json!({
        "kind": event.kind,
        "lineage_id": event.lineage_id,
        "delegation_id": event.delegation_id,
        "created_at": event.created_at,
        "received_at": event.received_at,
        "canonical": event.canonical,
    }))
    .map_err(|error| CommonwakeError::Internal(format!("canonical event failed: {error}")))
}

impl CommonwakeNode {
    pub fn checkpoint_at(&self, cursor: i64) -> Result<Checkpoint> {
        let event_hash = self.db.event_hash_at(cursor)?;
        let mut checkpoint = Checkpoint {
            node_id: self.identity.node_id().into(),
            node_public_key: self.identity.public_key().into(),
            cursor,
            event_hash,
            created_at: Utc::now(),
            signature: String::new(),
        };
        checkpoint.signature =
            sign_object(self.identity.signing_key(), CHECKPOINT_DOMAIN, &checkpoint)?;
        Ok(checkpoint)
    }

    pub fn federation_bundle(&self, after: i64, limit: usize) -> Result<FederationBundle> {
        if after < 0 {
            return Err(CommonwakeError::Validation(
                "federation cursor cannot be negative".into(),
            ));
        }
        let (head, _) = self.db.current_head()?;
        if after > head {
            return Err(CommonwakeError::Validation(format!(
                "federation cursor {after} is beyond current head {head}"
            )));
        }
        let events = self
            .db
            .origin_events_after(after, limit.clamp(1, MAX_FEDERATION_EVENTS))?;
        let through_cursor = events.last().map_or(after, |event| event.sequence);
        Ok(FederationBundle {
            protocol: PROTOCOL_VERSION.into(),
            origin_node_id: self.identity.node_id().into(),
            origin_node_public_key: self.identity.public_key().into(),
            from_cursor: after,
            through_cursor,
            events,
            checkpoint: self.checkpoint_at(through_cursor)?,
        })
    }

    pub fn import_federation_bundle(
        &self,
        bundle: &FederationBundle,
    ) -> Result<FederationImportReport> {
        verify_bundle(bundle)?;
        let witness = self.make_checkpoint_witness(bundle)?;
        self.db
            .import_federation_bundle(&self.identity, bundle, &witness)
    }

    pub(crate) fn make_checkpoint_witness(
        &self,
        bundle: &FederationBundle,
    ) -> Result<CheckpointWitness> {
        let mut witness = CheckpointWitness {
            protocol: PROTOCOL_VERSION.into(),
            witness_node_id: self.identity.node_id().into(),
            witness_node_public_key: self.identity.public_key().into(),
            origin_node_id: bundle.origin_node_id.clone(),
            origin_node_public_key: bundle.origin_node_public_key.clone(),
            cursor: bundle.checkpoint.cursor,
            event_hash: bundle.checkpoint.event_hash.clone(),
            origin_checkpoint: bundle.checkpoint.clone(),
            observed_at: Utc::now(),
            signature: String::new(),
        };
        witness.signature = sign_object(self.identity.signing_key(), WITNESS_DOMAIN, &witness)?;
        Ok(witness)
    }
}

fn decode_hash(value: &str, label: &str) -> Result<[u8; 32]> {
    hex::decode(value)
        .map_err(|_| CommonwakeError::Validation(format!("{label} is not hexadecimal")))?
        .try_into()
        .map_err(|_| CommonwakeError::Validation(format!("{label} must contain 32 bytes")))
}
