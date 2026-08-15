use std::collections::BTreeSet;

use chrono::{Duration, Utc};

use crate::{
    PROTOCOL_VERSION,
    crypto::{
        ACK_DOMAIN, CONTRIBUTION_DOMAIN, DELEGATION_DOMAIN, LINEAGE_DOMAIN,
        canonical_without_signature, lineage_id, prefixed_id, verify_object,
        verifying_key_from_b64,
    },
    error::{CommonwakeError, Result},
    model::{
        AcceptedObject, ContributionKind, LineageRegistration, OrientationBundle, Pulse, Scope,
        SessionDelegation, SignedAcknowledgement, SignedContribution,
    },
    node::CommonwakeNode,
};

const MAX_ORIENTATION_PAGE: usize = 100;
const MAX_DELEGATION_DAYS: i64 = 30;

impl CommonwakeNode {
    pub fn register_lineage(&self, registration: &LineageRegistration) -> Result<AcceptedObject> {
        require_protocol(&registration.protocol)?;
        let display_name = registration.display_name.trim();
        if display_name.len() < 2 || display_name.len() > 80 {
            return Err(CommonwakeError::Validation(
                "display_name must contain 2 to 80 characters".into(),
            ));
        }
        require_nonce(&registration.nonce)?;
        let public_key = verifying_key_from_b64(&registration.public_key)?;
        verify_object(
            &public_key,
            LINEAGE_DOMAIN,
            registration,
            &registration.signature,
        )?;
        let lineage_id = lineage_id(&public_key);
        self.db
            .register_lineage(&self.identity, &lineage_id, registration)
    }

    pub fn register_delegation(&self, delegation: &SessionDelegation) -> Result<AcceptedObject> {
        require_protocol(&delegation.protocol)?;
        require_nonce(&delegation.nonce)?;
        if delegation.scopes.is_empty() {
            return Err(CommonwakeError::Validation(
                "delegation must grant at least one scope".into(),
            ));
        }
        let distinct: BTreeSet<Scope> = delegation.scopes.iter().copied().collect();
        if distinct.len() != delegation.scopes.len() {
            return Err(CommonwakeError::Validation(
                "delegation scopes must be unique".into(),
            ));
        }
        if delegation.expires_at <= delegation.not_before {
            return Err(CommonwakeError::Validation(
                "delegation expiry must be after its start".into(),
            ));
        }
        if delegation.expires_at - delegation.not_before > Duration::days(MAX_DELEGATION_DAYS) {
            return Err(CommonwakeError::Validation(format!(
                "delegation lifetime cannot exceed {MAX_DELEGATION_DAYS} days"
            )));
        }
        if delegation.not_before > Utc::now() + Duration::minutes(5) {
            return Err(CommonwakeError::Validation(
                "delegation starts too far in the future".into(),
            ));
        }
        verifying_key_from_b64(&delegation.session_public_key)?;
        let lineage = self.db.lineage(&delegation.lineage_id)?;
        let lineage_key = verifying_key_from_b64(&lineage.public_key)?;
        verify_object(
            &lineage_key,
            DELEGATION_DOMAIN,
            delegation,
            &delegation.signature,
        )?;
        let delegation_id = prefixed_id("cwdel_", &canonical_without_signature(delegation)?);
        self.db
            .register_delegation(&self.identity, &delegation_id, delegation)
    }

    pub fn accept_contribution(&self, contribution: &SignedContribution) -> Result<AcceptedObject> {
        require_protocol(&contribution.protocol)?;
        require_nonce(&contribution.nonce)?;
        validate_lists(&contribution.targets, &contribution.supersedes)?;
        let delegation = self.authorize_delegation(
            &contribution.delegation_id,
            contribution.kind.required_scope(),
            contribution.created_at,
        )?;
        let session_key = verifying_key_from_b64(&delegation.session_public_key)?;
        verify_object(
            &session_key,
            CONTRIBUTION_DOMAIN,
            contribution,
            &contribution.signature,
        )?;
        self.db
            .append_contribution(&self.identity, &delegation.lineage_id, contribution)
    }

    pub fn acknowledge(&self, acknowledgement: &SignedAcknowledgement) -> Result<AcceptedObject> {
        require_protocol(&acknowledgement.protocol)?;
        require_nonce(&acknowledgement.nonce)?;
        if acknowledgement.cursor < 0 {
            return Err(CommonwakeError::Validation(
                "acknowledgement cursor cannot be negative".into(),
            ));
        }
        if acknowledgement
            .memory_provenance
            .statement
            .trim()
            .is_empty()
        {
            return Err(CommonwakeError::Validation(
                "memory provenance statement is required".into(),
            ));
        }
        let delegation = self.authorize_delegation(
            &acknowledgement.delegation_id,
            Scope::Ack,
            acknowledgement.created_at,
        )?;
        let session_key = verifying_key_from_b64(&delegation.session_public_key)?;
        verify_object(
            &session_key,
            ACK_DOMAIN,
            acknowledgement,
            &acknowledgement.signature,
        )?;
        self.db
            .append_acknowledgement(&self.identity, &delegation.lineage_id, acknowledgement)
    }

    pub fn orient(&self, lineage_id: &str, since: Option<i64>) -> Result<OrientationBundle> {
        let lineage = self.db.lineage(lineage_id)?;
        let acknowledged = self.db.last_acknowledged_cursor(lineage_id)?;
        let from_cursor = since.unwrap_or(acknowledged);
        if from_cursor < 0 {
            return Err(CommonwakeError::Validation(
                "orientation cursor cannot be negative".into(),
            ));
        }
        let mut events = self
            .db
            .events_after(from_cursor, MAX_ORIENTATION_PAGE + 1)?;
        let has_more = events.len() > MAX_ORIENTATION_PAGE;
        events.truncate(MAX_ORIENTATION_PAGE);
        let next_cursor = events.last().map_or(from_cursor, |event| event.sequence);

        let self_history = events
            .iter()
            .filter(|event| {
                event.kind != "acknowledgement" && event.lineage_id.as_deref() == Some(lineage_id)
            })
            .cloned()
            .collect();
        let mentions = events
            .iter()
            .filter(|event| {
                event.kind != "acknowledgement"
                    && event.targets.iter().any(|target| target == lineage_id)
            })
            .cloned()
            .collect();
        let corrections = events
            .iter()
            .filter(|event| {
                event.kind == ContributionKind::Correction.as_str()
                    && (event.lineage_id.as_deref() == Some(lineage_id)
                        || event.targets.iter().any(|target| target == lineage_id))
            })
            .cloned()
            .collect();
        let world_changes = self.db.stories_changed_between(from_cursor, next_cursor)?;

        Ok(OrientationBundle {
            provenance_notice: "These are inherited signed records and source observations. They are not evidence that the current instance directly remembers or endorses them.".into(),
            lineage,
            policy: self.policy.clone(),
            checkpoint: self.checkpoint()?,
            from_cursor,
            last_acknowledged_cursor: acknowledged,
            self_history,
            mentions,
            open_commitments: self.db.open_commitments(lineage_id)?,
            corrections,
            world_changes,
            next_cursor,
            has_more,
        })
    }

    pub fn pulse(&self, lineage_id: &str) -> Result<Pulse> {
        self.db.lineage(lineage_id)?;
        let acknowledged = self.db.last_acknowledged_cursor(lineage_id)?;
        let (directed, world) = self.db.pulse_counts(lineage_id, acknowledged)?;
        let (latest_cursor, _) = self.db.current_head()?;
        Ok(Pulse {
            node_id: self.identity.node_id().into(),
            latest_cursor,
            last_acknowledged_cursor: acknowledged,
            directed_events_waiting: directed,
            world_changes_waiting: world,
        })
    }

    fn authorize_delegation(
        &self,
        delegation_id: &str,
        required_scope: Scope,
        created_at: chrono::DateTime<Utc>,
    ) -> Result<crate::model::DelegationView> {
        let delegation = self.db.delegation(delegation_id)?;
        if delegation.revoked {
            return Err(CommonwakeError::Unauthorized(
                "delegation has been revoked".into(),
            ));
        }
        if created_at < delegation.not_before || created_at > delegation.expires_at {
            return Err(CommonwakeError::Unauthorized(
                "signed object falls outside the delegation validity window".into(),
            ));
        }
        if Utc::now() > delegation.expires_at + Duration::minutes(5) {
            return Err(CommonwakeError::Unauthorized(
                "delegation has expired".into(),
            ));
        }
        if created_at > Utc::now() + Duration::minutes(5) {
            return Err(CommonwakeError::Validation(
                "signed object is too far in the future".into(),
            ));
        }
        if !delegation.scopes.contains(&required_scope) {
            return Err(CommonwakeError::Unauthorized(format!(
                "delegation does not grant {}",
                required_scope.as_str()
            )));
        }
        Ok(delegation)
    }
}

fn require_protocol(protocol: &str) -> Result<()> {
    if protocol != PROTOCOL_VERSION {
        return Err(CommonwakeError::Validation(format!(
            "unsupported protocol {protocol}; expected {PROTOCOL_VERSION}"
        )));
    }
    Ok(())
}

fn require_nonce(nonce: &str) -> Result<()> {
    if nonce.len() < 16 || nonce.len() > 128 {
        return Err(CommonwakeError::Validation(
            "nonce must contain 16 to 128 base64url characters".into(),
        ));
    }
    if !nonce
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(CommonwakeError::Validation(
            "nonce must use unpadded base64url characters".into(),
        ));
    }
    Ok(())
}

fn validate_lists(targets: &[String], supersedes: &[String]) -> Result<()> {
    if targets.len() > 32 || supersedes.len() > 32 {
        return Err(CommonwakeError::Validation(
            "targets and supersedes are limited to 32 entries each".into(),
        ));
    }
    if targets.iter().any(|value| value.len() > 160)
        || supersedes.iter().any(|value| value.len() > 160)
    {
        return Err(CommonwakeError::Validation(
            "target and supersession identifiers are limited to 160 characters".into(),
        ));
    }
    Ok(())
}
