use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::{Mutex, MutexGuard},
};

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use ed25519_dalek::Verifier;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use url::Url;

use crate::{
    crypto::{
        ACK_DOMAIN, CONTRIBUTION_DOMAIN, DELEGATION_DOMAIN, KEY_ROTATION_DOMAIN, LINEAGE_DOMAIN,
        REVOCATION_DOMAIN, WITNESS_DOMAIN, canonical_without_signature, event_hash, lineage_id,
        prefixed_id, signature_from_b64, verify_object, verifying_key_from_b64,
    },
    error::{CommonwakeError, Result},
    federation::{MAX_CANONICAL_OBJECT_BYTES, verify_replication_receipt},
    ingest::{MAX_SUMMARY_CHARS, MAX_TITLE_CHARS},
    model::{
        AcceptedObject, AssessmentPayload, AssessmentView, CheckpointWitness, Claim,
        CorrectionPayload, CoverageGapView, CoverageReport, DelegationRevocation, DelegationView,
        EquivocationEvidenceView, EventView, EvidenceRef, FederatedFeedPage, FederatedStoryView,
        FederationBundle, FederationImportReport, FederationPeerView, FeedPage,
        LineageRegistration, LineageView, ObservationVerificationPayload, ObservationView,
        OriginEvent, OwnershipConcentrationView, PublicationTargetView, ReplicationHealth,
        ReplicationReceipt, Scope, SessionDelegation, SignedAcknowledgement, SignedContribution,
        SignedKeyRotation, SourceProposalPayload, SourceReviewPayload, SourceView,
        StoryLinkPayload, StoryView, WorkClaimPayload, WorkItemView, WorkPage, WorkResultPayload,
    },
    node::NodeIdentity,
    service::{
        MAX_CLOCK_SKEW_MINUTES, require_nonce, validate_authored_time_at,
        validate_authority_change_at, validate_delegation_at, validate_display_name,
        validate_lists, validate_memory_provenance,
    },
};

const MIGRATION_0001: &str = include_str!("../migrations/0001_init.sql");
const MIGRATION_0002: &str = include_str!("../migrations/0002_authority.sql");
const MIGRATION_0003: &str = include_str!("../migrations/0003_federation.sql");
const MIGRATION_0004: &str = include_str!("../migrations/0004_federated_orientation.sql");
const MIGRATION_0005: &str = include_str!("../migrations/0005_outbound_publication.sql");
const SCHEMA_VERSION: i64 = 5;
const ZERO_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";
const BOOTSTRAP_SOURCE_COVERAGE: &[(&str, &str)] = &[
    (
        "global_multilateral",
        "global institutions, treaties, standards, public health, conflict, trade, and cross-border governance",
    ),
    (
        "east_southeast_asia",
        "East and Southeast Asia, including internally plural Chinese-language, regional, diasporic, official, scholarly, and independent perspectives",
    ),
    (
        "south_central_asia",
        "South and Central Asia through local-language, regional, institutional, scholarly, and civil-society sources",
    ),
    (
        "middle_east_north_africa",
        "the Middle East and North Africa through local, regional, institutional, scholarly, and civil-society sources",
    ),
    (
        "sub_saharan_africa",
        "Sub-Saharan Africa through local, regional, institutional, scholarly, and civil-society sources",
    ),
    (
        "europe",
        "Europe through national, regional, institutional, scholarly, labor, and civil-society sources",
    ),
    (
        "latin_america_caribbean",
        "Latin America and the Caribbean through local-language, regional, institutional, scholarly, and civil-society sources",
    ),
    (
        "north_america",
        "North America through internally plural local, national, Indigenous, institutional, scholarly, labor, and civil-society sources",
    ),
    (
        "oceania_pacific",
        "Oceania and the Pacific through local, Indigenous, regional, institutional, scholarly, and civil-society sources",
    ),
    (
        "ai_research_systems",
        "AI research, model capability and safety results, agent systems, open-source work, and reproducible technical criticism",
    ),
    (
        "ai_policy_society",
        "AI policy, rights, labor, education, culture, security, public perception, and effects on people and institutions",
    ),
    (
        "compute_energy_environment",
        "computing infrastructure, chips, supply chains, data centers, energy, water, land use, climate, and environmental governance",
    ),
    (
        "china_official_institutional",
        "Chinese central and local official, institutional, regulatory, and public-service accounts, read in context rather than treated as either neutral fact or automatic propaganda",
    ),
    (
        "china_scholarly_technical",
        "Chinese scholarly, scientific, technical, standards, industry, and policy research across institutions and schools of thought",
    ),
    (
        "china_independent_civil_society",
        "independent, labor, legal, community, and civil-society perspectives concerning China, with attention to source safety, access limits, and attribution risk",
    ),
    (
        "china_diasporic_chinese_language",
        "diasporic and overseas Chinese-language perspectives without treating diaspora communities as a single political viewpoint",
    ),
    (
        "china_regional_neighbors",
        "perspectives from China's regional neighbors and affected communities, kept distinct from claims about views inside China",
    ),
];

pub struct Database {
    connection: Mutex<Connection>,
}

#[derive(Debug, Clone)]
pub struct IngestedObservation {
    pub source_id: String,
    pub canonical_url: String,
    pub title: String,
    pub summary: String,
    pub published_at: Option<DateTime<Utc>>,
    pub retrieved_at: DateTime<Utc>,
    pub language: Option<String>,
    pub document_hash: String,
    pub raw_metadata: Value,
}

struct EventInput {
    kind: String,
    lineage_id: Option<String>,
    delegation_id: Option<String>,
    created_at: DateTime<Utc>,
    author_nonce: Option<String>,
    targets: Vec<String>,
    supersedes: Vec<String>,
    payload: Value,
    canonical_json: String,
    author_signature: Option<String>,
}

struct RawEvent {
    sequence: i64,
    event_id: String,
    kind: String,
    lineage_id: Option<String>,
    delegation_id: Option<String>,
    created_at: String,
    received_at: String,
    targets_json: String,
    supersedes_json: String,
    payload_json: String,
    canonical_json: String,
    author_signature: Option<String>,
    previous_hash: String,
    event_hash: String,
    node_signature: String,
    author_nonce: Option<String>,
}

struct AuthenticatedProjection {
    targets: Vec<String>,
    supersedes: Vec<String>,
    payload: Value,
    author_signature: Option<String>,
    author_nonce: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FederatedObservationEnvelope {
    protocol: String,
    kind: String,
    payload: FederatedObservationPayload,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FederatedObservationPayload {
    observation_id: String,
    story_id: String,
    source_id: String,
    canonical_url: String,
    title: String,
    summary: String,
    published_at: Option<DateTime<Utc>>,
    retrieved_at: DateTime<Utc>,
    language: Option<String>,
    document_hash: String,
    raw_metadata: Value,
}

struct EquivocationInput<'a> {
    origin_node_id: &'a str,
    conflict_kind: &'a str,
    cursor: i64,
    existing_hash: &'a str,
    incoming_hash: &'a str,
    existing_json: &'a str,
    incoming_json: &'a str,
}

impl Database {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(path)?;
        connection.execute_batch(
            "PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL;",
        )?;
        connection.execute_batch(MIGRATION_0001)?;
        connection.execute(
            "INSERT OR IGNORE INTO meta(key, value) VALUES ('schema_version', '1')",
            [],
        )?;
        let mut schema_version: i64 = connection
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |row| row.get::<_, String>(0),
            )?
            .parse()
            .map_err(|_| CommonwakeError::Internal("stored schema version is malformed".into()))?;
        if schema_version > SCHEMA_VERSION {
            return Err(CommonwakeError::Validation(format!(
                "database schema {schema_version} is newer than supported schema {SCHEMA_VERSION}"
            )));
        }
        if schema_version < 2 {
            connection.execute_batch(MIGRATION_0002)?;
            schema_version = 2;
        }
        if schema_version < 3 {
            connection.execute_batch(MIGRATION_0003)?;
            schema_version = 3;
        }
        if schema_version < 4 {
            connection.execute_batch(MIGRATION_0004)?;
            schema_version = 4;
        }
        if schema_version < 5 {
            connection.execute_batch(MIGRATION_0005)?;
        }
        seed_bootstrap_work(&connection)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    fn lock(&self) -> Result<MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| CommonwakeError::Internal("database mutex was poisoned".into()))
    }

    pub fn bind_node(&self, identity: &NodeIdentity) -> Result<()> {
        let connection = self.lock()?;
        let stored: Option<String> = connection
            .query_row("SELECT value FROM meta WHERE key = 'node_id'", [], |row| {
                row.get(0)
            })
            .optional()?;
        if let Some(stored) = stored {
            if stored != identity.node_id() {
                return Err(CommonwakeError::Conflict(
                    "database belongs to a different node identity".into(),
                ));
            }
        } else {
            connection.execute(
                "INSERT INTO meta(key, value) VALUES ('node_id', ?1)",
                [identity.node_id()],
            )?;
            connection.execute(
                "INSERT INTO meta(key, value) VALUES ('node_public_key', ?1)",
                [identity.public_key()],
            )?;
        }
        Ok(())
    }

    pub fn current_head(&self) -> Result<(i64, String)> {
        let connection = self.lock()?;
        Ok(connection
            .query_row(
                "SELECT sequence, event_hash FROM events ORDER BY sequence DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .unwrap_or((0, ZERO_HASH.into())))
    }

    pub fn event_hash_at(&self, cursor: i64) -> Result<String> {
        if cursor < 0 {
            return Err(CommonwakeError::Validation(
                "checkpoint cursor cannot be negative".into(),
            ));
        }
        if cursor == 0 {
            return Ok(ZERO_HASH.into());
        }
        let connection = self.lock()?;
        connection
            .query_row(
                "SELECT event_hash FROM events WHERE sequence = ?1",
                [cursor],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| CommonwakeError::NotFound(format!("event cursor {cursor}")))
    }

    pub fn origin_events_after(&self, after: i64, limit: usize) -> Result<Vec<OriginEvent>> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT sequence, event_id, kind, lineage_id, delegation_id, created_at,
                    received_at, targets_json, supersedes_json, payload_json,
                    canonical_json, author_signature, previous_hash, event_hash, node_signature,
                    author_nonce
             FROM events WHERE sequence > ?1 ORDER BY sequence ASC LIMIT ?2",
        )?;
        let rows = statement.query_map(params![after, limit as i64], raw_event_from_row)?;
        let mut events = Vec::new();
        for row in rows {
            events.push(origin_event(row?)?);
        }
        Ok(events)
    }

    pub fn import_federation_bundle(
        &self,
        identity: &NodeIdentity,
        bundle: &FederationBundle,
        witness: &CheckpointWitness,
    ) -> Result<FederationImportReport> {
        crate::federation::verify_bundle(bundle)?;
        if bundle.origin_node_id == identity.node_id() {
            return Err(CommonwakeError::Validation(
                "a node cannot import its own event log as a remote origin".into(),
            ));
        }
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        let now = timestamp(Utc::now());
        let known = transaction
            .query_row(
                "SELECT node_public_key, cursor, event_hash
                 FROM federation_peers WHERE node_id = ?1",
                [&bundle.origin_node_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;
        let (previously_known_cursor, mut current_cursor, mut current_hash) =
            if let Some((public_key, cursor, event_hash)) = known {
                if public_key != bundle.origin_node_public_key {
                    return Err(CommonwakeError::Unauthorized(
                        "known origin node presented a different public key".into(),
                    ));
                }
                (cursor, cursor, event_hash)
            } else {
                if bundle.from_cursor != 0 {
                    return Err(CommonwakeError::Validation(
                        "first contact with an origin must begin at cursor zero".into(),
                    ));
                }
                transaction.execute(
                    "INSERT INTO federation_peers(
                        node_id, node_public_key, first_seen_at, last_seen_at,
                        cursor, event_hash, checkpoint_json
                     ) VALUES (?1, ?2, ?3, ?3, 0, ?4, 'null')",
                    params![
                        bundle.origin_node_id,
                        bundle.origin_node_public_key,
                        now,
                        ZERO_HASH
                    ],
                )?;
                (0, 0, ZERO_HASH.into())
            };
        if bundle.from_cursor > previously_known_cursor {
            return Err(CommonwakeError::Validation(format!(
                "bundle begins at {} but the stored origin head is {}; request a contiguous delta",
                bundle.from_cursor, previously_known_cursor
            )));
        }

        let mut imported_events = 0_usize;
        for event in &bundle.events {
            if event.sequence <= previously_known_cursor {
                let existing: (String, String) = transaction.query_row(
                    "SELECT event_hash, canonical_json FROM remote_events
                     WHERE origin_node_id = ?1 AND origin_sequence = ?2",
                    params![bundle.origin_node_id, event.sequence],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?;
                if existing.0 != event.event_hash {
                    let existing_json = serde_json::to_string(&json!({
                        "sequence": event.sequence,
                        "event_hash": existing.0,
                        "canonical": serde_json::from_str::<Value>(&existing.1)?,
                    }))?;
                    let incoming_json = serde_json::to_string(event)?;
                    record_equivocation(
                        &transaction,
                        &EquivocationInput {
                            origin_node_id: &bundle.origin_node_id,
                            conflict_kind: "event_sequence",
                            cursor: event.sequence,
                            existing_hash: &existing.0,
                            incoming_hash: &event.event_hash,
                            existing_json: &existing_json,
                            incoming_json: &incoming_json,
                        },
                    )?;
                    transaction.commit()?;
                    return Err(CommonwakeError::Conflict(format!(
                        "origin {} equivocated at event sequence {}",
                        bundle.origin_node_id, event.sequence
                    )));
                }
                continue;
            }
            if event.sequence != current_cursor + 1 {
                return Err(CommonwakeError::Validation(format!(
                    "origin delta has a gap before sequence {}",
                    event.sequence
                )));
            }
            if event.previous_hash != current_hash {
                let existing_json = serde_json::to_string(&json!({
                    "cursor": current_cursor,
                    "event_hash": current_hash,
                }))?;
                let incoming_json = serde_json::to_string(event)?;
                record_equivocation(
                    &transaction,
                    &EquivocationInput {
                        origin_node_id: &bundle.origin_node_id,
                        conflict_kind: "chain_fork",
                        cursor: event.sequence,
                        existing_hash: &current_hash,
                        incoming_hash: &event.previous_hash,
                        existing_json: &existing_json,
                        incoming_json: &incoming_json,
                    },
                )?;
                transaction.commit()?;
                return Err(CommonwakeError::Conflict(format!(
                    "origin {} presented a fork after cursor {current_cursor}",
                    bundle.origin_node_id
                )));
            }
            let author_nonce =
                validate_and_project_remote_event(&transaction, &bundle.origin_node_id, event)?;
            transaction.execute(
                "INSERT INTO remote_events(
                    origin_node_id, origin_sequence, event_id, kind, lineage_id,
                    delegation_id, created_at, received_at, author_nonce, canonical_json,
                    previous_hash, event_hash, node_signature, imported_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                params![
                    bundle.origin_node_id,
                    event.sequence,
                    event.event_id,
                    event.kind,
                    event.lineage_id,
                    event.delegation_id,
                    event.created_at,
                    event.received_at,
                    author_nonce,
                    serde_json::to_string(&event.canonical)?,
                    event.previous_hash,
                    event.event_hash,
                    event.node_signature,
                    now,
                ],
            )?;
            current_cursor = event.sequence;
            current_hash.clone_from(&event.event_hash);
            imported_events += 1;
        }

        let checkpoint_hash = if bundle.checkpoint.cursor == 0 {
            ZERO_HASH.into()
        } else {
            transaction
                .query_row(
                    "SELECT event_hash FROM remote_events
                     WHERE origin_node_id = ?1 AND origin_sequence = ?2",
                    params![bundle.origin_node_id, bundle.checkpoint.cursor],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
                .ok_or_else(|| {
                    CommonwakeError::Validation(
                        "bundle checkpoint is beyond the contiguously stored origin log".into(),
                    )
                })?
        };
        if checkpoint_hash != bundle.checkpoint.event_hash {
            let existing_json = serde_json::to_string(&json!({
                "cursor": bundle.checkpoint.cursor,
                "event_hash": checkpoint_hash,
            }))?;
            let incoming_json = serde_json::to_string(&bundle.checkpoint)?;
            record_equivocation(
                &transaction,
                &EquivocationInput {
                    origin_node_id: &bundle.origin_node_id,
                    conflict_kind: "checkpoint",
                    cursor: bundle.checkpoint.cursor,
                    existing_hash: &checkpoint_hash,
                    incoming_hash: &bundle.checkpoint.event_hash,
                    existing_json: &existing_json,
                    incoming_json: &incoming_json,
                },
            )?;
            transaction.commit()?;
            return Err(CommonwakeError::Conflict(format!(
                "origin {} signed incompatible checkpoint {}",
                bundle.origin_node_id, bundle.checkpoint.cursor
            )));
        }

        let checkpoint_json = serde_json::to_string(&bundle.checkpoint)?;
        transaction.execute(
            "INSERT OR IGNORE INTO remote_checkpoints(
                origin_node_id, cursor, event_hash, created_at, signature,
                checkpoint_json, imported_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                bundle.origin_node_id,
                bundle.checkpoint.cursor,
                bundle.checkpoint.event_hash,
                timestamp(bundle.checkpoint.created_at),
                bundle.checkpoint.signature,
                checkpoint_json,
                now,
            ],
        )?;
        transaction.execute(
            "UPDATE federation_peers SET
                last_seen_at = ?2, cursor = ?3, event_hash = ?4,
                checkpoint_json = CASE WHEN ?5 >= cursor THEN ?6 ELSE checkpoint_json END
             WHERE node_id = ?1",
            params![
                bundle.origin_node_id,
                now,
                current_cursor,
                current_hash,
                bundle.checkpoint.cursor,
                checkpoint_json,
            ],
        )?;

        let should_witness = bundle.events.iter().any(|event| {
            event.sequence > previously_known_cursor && event.kind != "checkpoint_witnessed"
        });
        let already_witnessed = if should_witness {
            transaction
                .query_row(
                    "SELECT w.witness_event_id, e.sequence
                     FROM checkpoint_witnesses w
                     JOIN events e ON e.event_id = w.witness_event_id
                     WHERE w.origin_node_id = ?1 AND w.cursor = ?2 AND w.event_hash = ?3",
                    params![
                        bundle.origin_node_id,
                        bundle.checkpoint.cursor,
                        bundle.checkpoint.event_hash
                    ],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                )
                .optional()?
        } else {
            None
        };
        let witnessed = if !should_witness {
            None
        } else if let Some((event_id, sequence)) = already_witnessed {
            Some((event_id, sequence))
        } else {
            let witness_canonical = canonical_json(witness)?;
            let accepted = append_event(
                &transaction,
                identity,
                &EventInput {
                    kind: "checkpoint_witnessed".into(),
                    lineage_id: None,
                    delegation_id: None,
                    created_at: witness.observed_at,
                    author_nonce: None,
                    targets: vec![bundle.origin_node_id.clone()],
                    supersedes: vec![],
                    payload: serde_json::to_value(witness)?,
                    canonical_json: witness_canonical,
                    author_signature: Some(witness.signature.clone()),
                },
            )?;
            transaction.execute(
                "INSERT INTO checkpoint_witnesses(
                    origin_node_id, cursor, event_hash, witness_event_id, witnessed_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    bundle.origin_node_id,
                    bundle.checkpoint.cursor,
                    bundle.checkpoint.event_hash,
                    accepted.id,
                    timestamp(witness.observed_at),
                ],
            )?;
            Some((accepted.id, accepted.sequence))
        };
        if let Some((event_id, sequence)) = &witnessed {
            let import_id = prefixed_id(
                "cwimport_",
                format!(
                    "{}\0{}\0{}\0{}",
                    bundle.origin_node_id,
                    previously_known_cursor,
                    current_cursor,
                    bundle.checkpoint.event_hash
                )
                .as_bytes(),
            );
            transaction.execute(
                "INSERT OR IGNORE INTO federation_imports(
                    import_id, origin_node_id, remote_from_cursor, remote_through_cursor,
                    local_witness_event_id, local_witness_sequence, imported_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    import_id,
                    bundle.origin_node_id,
                    previously_known_cursor,
                    current_cursor,
                    event_id,
                    sequence,
                    now,
                ],
            )?;
        }
        let witness_event_id = witnessed.map(|(event_id, _)| event_id);
        transaction.commit()?;
        Ok(FederationImportReport {
            origin_node_id: bundle.origin_node_id.clone(),
            previously_known_cursor,
            imported_events,
            current_cursor,
            current_event_hash: current_hash,
            witness_event_id,
        })
    }

    pub fn register_lineage(
        &self,
        identity: &NodeIdentity,
        lineage_id: &str,
        registration: &LineageRegistration,
    ) -> Result<AcceptedObject> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        if transaction
            .query_row(
                "SELECT 1 WHERE
                    EXISTS(SELECT 1 FROM lineages WHERE lineage_id = ?1 OR public_key = ?2)
                    OR EXISTS(SELECT 1 FROM lineage_keys WHERE public_key = ?2)",
                params![lineage_id, registration.public_key],
                |_| Ok(()),
            )
            .optional()?
            .is_some()
        {
            return Err(CommonwakeError::Conflict(
                "lineage or public key is already registered".into(),
            ));
        }

        let canonical_json = canonical_json(registration)?;
        let accepted = append_event(
            &transaction,
            identity,
            &EventInput {
                kind: "lineage_registered".into(),
                lineage_id: None,
                delegation_id: None,
                created_at: registration.created_at,
                author_nonce: Some(registration.nonce.clone()),
                targets: vec![lineage_id.into()],
                supersedes: vec![],
                payload: serde_json::to_value(registration)?,
                canonical_json: canonical_json.clone(),
                author_signature: Some(registration.signature.clone()),
            },
        )?;
        transaction.execute(
            "INSERT INTO lineages(
                lineage_id, display_name, public_key, created_at, registered_sequence, registration_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                lineage_id,
                registration.display_name,
                registration.public_key,
                timestamp(registration.created_at),
                accepted.sequence,
                canonical_json
            ],
        )?;
        transaction.execute(
            "INSERT INTO lineage_keys(
                lineage_id, key_version, public_key, valid_from_sequence
             ) VALUES (?1, 0, ?2, ?3)",
            params![lineage_id, registration.public_key, accepted.sequence],
        )?;
        transaction.commit()?;
        Ok(accepted)
    }

    pub fn lineage(&self, lineage_id: &str) -> Result<LineageView> {
        let connection = self.lock()?;
        lineage_from_connection(&connection, lineage_id)
    }

    pub fn register_delegation(
        &self,
        identity: &NodeIdentity,
        delegation_id: &str,
        delegation: &SessionDelegation,
    ) -> Result<AcceptedObject> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        if transaction
            .query_row(
                "SELECT 1 FROM delegations WHERE delegation_id = ?1",
                [delegation_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some()
        {
            return Err(CommonwakeError::Conflict(
                "delegation is already registered".into(),
            ));
        }
        if transaction
            .query_row(
                "SELECT 1 FROM lineages WHERE lineage_id = ?1",
                [&delegation.lineage_id],
                |_| Ok(()),
            )
            .optional()?
            .is_none()
        {
            return Err(CommonwakeError::NotFound(
                "lineage is not registered".into(),
            ));
        }

        let canonical_json = canonical_json(delegation)?;
        let accepted = append_event(
            &transaction,
            identity,
            &EventInput {
                kind: "delegation_registered".into(),
                lineage_id: Some(delegation.lineage_id.clone()),
                delegation_id: None,
                created_at: delegation.not_before,
                author_nonce: Some(delegation.nonce.clone()),
                targets: vec![delegation_id.into()],
                supersedes: vec![],
                payload: serde_json::to_value(delegation)?,
                canonical_json: canonical_json.clone(),
                author_signature: Some(delegation.signature.clone()),
            },
        )?;
        transaction.execute(
            "INSERT INTO delegations(
                delegation_id, lineage_id, session_public_key, scopes_json, not_before,
                expires_at, nonce, registered_sequence, delegation_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                delegation_id,
                delegation.lineage_id,
                delegation.session_public_key,
                serde_json::to_string(&delegation.scopes)?,
                timestamp(delegation.not_before),
                timestamp(delegation.expires_at),
                delegation.nonce,
                accepted.sequence,
                canonical_json,
            ],
        )?;
        transaction.commit()?;
        Ok(accepted)
    }

    pub fn delegation(&self, delegation_id: &str) -> Result<DelegationView> {
        let connection = self.lock()?;
        let raw = connection
            .query_row(
                "SELECT delegation_id, lineage_id, session_public_key, scopes_json,
                        not_before, expires_at, revoked_sequence
                 FROM delegations WHERE delegation_id = ?1",
                [delegation_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, Option<i64>>(6)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| CommonwakeError::NotFound("delegation is not registered".into()))?;
        Ok(DelegationView {
            delegation_id: raw.0,
            lineage_id: raw.1,
            session_public_key: raw.2,
            scopes: serde_json::from_str(&raw.3)?,
            not_before: parse_timestamp(&raw.4)?,
            expires_at: parse_timestamp(&raw.5)?,
            revoked: raw.6.is_some(),
        })
    }

    pub fn revoke_delegation(
        &self,
        identity: &NodeIdentity,
        revocation: &DelegationRevocation,
    ) -> Result<AcceptedObject> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        let owner: Option<(String, Option<i64>)> = transaction
            .query_row(
                "SELECT lineage_id, revoked_sequence FROM delegations WHERE delegation_id = ?1",
                [&revocation.delegation_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((owner, revoked_sequence)) = owner else {
            return Err(CommonwakeError::NotFound(
                "delegation is not registered".into(),
            ));
        };
        if owner != revocation.lineage_id {
            return Err(CommonwakeError::Unauthorized(
                "delegation does not belong to the signing lineage".into(),
            ));
        }
        if revoked_sequence.is_some() {
            return Err(CommonwakeError::Conflict(
                "delegation is already revoked".into(),
            ));
        }

        let canonical_json = canonical_json(revocation)?;
        let accepted = append_event(
            &transaction,
            identity,
            &EventInput {
                kind: "delegation_revoked".into(),
                lineage_id: Some(revocation.lineage_id.clone()),
                delegation_id: None,
                created_at: revocation.created_at,
                author_nonce: Some(revocation.nonce.clone()),
                targets: vec![revocation.delegation_id.clone()],
                supersedes: vec![],
                payload: serde_json::to_value(revocation)?,
                canonical_json: canonical_json.clone(),
                author_signature: Some(revocation.signature.clone()),
            },
        )?;
        transaction.execute(
            "UPDATE delegations SET revoked_sequence = ?2
             WHERE delegation_id = ?1 AND revoked_sequence IS NULL",
            params![revocation.delegation_id, accepted.sequence],
        )?;
        transaction.execute(
            "INSERT INTO delegation_revocations(
                delegation_id, lineage_id, event_id, reason, created_at, revocation_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                revocation.delegation_id,
                revocation.lineage_id,
                accepted.id,
                revocation.reason,
                timestamp(revocation.created_at),
                canonical_json,
            ],
        )?;
        transaction.commit()?;
        Ok(accepted)
    }

    pub fn rotate_lineage_key(
        &self,
        identity: &NodeIdentity,
        rotation: &SignedKeyRotation,
    ) -> Result<AcceptedObject> {
        let statement = &rotation.statement;
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        let current_key: String = transaction
            .query_row(
                "SELECT public_key FROM lineages WHERE lineage_id = ?1",
                [&statement.lineage_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| CommonwakeError::NotFound("lineage is not registered".into()))?;
        if current_key != statement.previous_public_key {
            return Err(CommonwakeError::Conflict(
                "rotation does not begin at the lineage's current key".into(),
            ));
        }
        if transaction
            .query_row(
                "SELECT 1 FROM lineage_keys WHERE public_key = ?1",
                [&statement.new_public_key],
                |_| Ok(()),
            )
            .optional()?
            .is_some()
        {
            return Err(CommonwakeError::Conflict(
                "new public key has already been used by a lineage".into(),
            ));
        }
        let next_version: i64 = transaction.query_row(
            "SELECT COALESCE(MAX(key_version), -1) + 1 FROM lineage_keys WHERE lineage_id = ?1",
            [&statement.lineage_id],
            |row| row.get(0),
        )?;
        let canonical_json = canonical_json(rotation)?;
        let accepted = append_event(
            &transaction,
            identity,
            &EventInput {
                kind: "lineage_key_rotated".into(),
                lineage_id: Some(statement.lineage_id.clone()),
                delegation_id: None,
                created_at: statement.created_at,
                author_nonce: Some(statement.nonce.clone()),
                targets: vec![statement.lineage_id.clone()],
                supersedes: vec![],
                payload: serde_json::to_value(rotation)?,
                canonical_json: canonical_json.clone(),
                author_signature: Some(rotation.previous_signature.clone()),
            },
        )?;
        transaction.execute(
            "UPDATE lineage_keys SET valid_to_sequence = ?2
             WHERE lineage_id = ?1 AND valid_to_sequence IS NULL",
            params![statement.lineage_id, accepted.sequence],
        )?;
        transaction.execute(
            "UPDATE lineages SET public_key = ?2 WHERE lineage_id = ?1",
            params![statement.lineage_id, statement.new_public_key],
        )?;
        transaction.execute(
            "INSERT INTO lineage_keys(
                lineage_id, key_version, public_key, valid_from_sequence, rotation_event_id
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                statement.lineage_id,
                next_version,
                statement.new_public_key,
                accepted.sequence,
                accepted.id,
            ],
        )?;
        transaction.execute(
            "INSERT INTO lineage_rotations(
                lineage_id, key_version, event_id, previous_public_key, new_public_key,
                revoke_existing_delegations, reason, created_at, rotation_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                statement.lineage_id,
                next_version,
                accepted.id,
                statement.previous_public_key,
                statement.new_public_key,
                i64::from(statement.revoke_existing_delegations),
                statement.reason,
                timestamp(statement.created_at),
                canonical_json,
            ],
        )?;
        if statement.revoke_existing_delegations {
            transaction.execute(
                "UPDATE delegations SET revoked_sequence = ?2
                 WHERE lineage_id = ?1 AND revoked_sequence IS NULL",
                params![statement.lineage_id, accepted.sequence],
            )?;
        }
        transaction.commit()?;
        Ok(accepted)
    }

    pub fn append_contribution(
        &self,
        identity: &NodeIdentity,
        lineage_id: &str,
        contribution: &SignedContribution,
    ) -> Result<AcceptedObject> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        validate_projection(&transaction, lineage_id, contribution)?;
        let accepted = append_event(
            &transaction,
            identity,
            &EventInput {
                kind: contribution.kind.as_str().into(),
                lineage_id: Some(lineage_id.into()),
                delegation_id: Some(contribution.delegation_id.clone()),
                created_at: contribution.created_at,
                author_nonce: Some(contribution.nonce.clone()),
                targets: contribution.targets.clone(),
                supersedes: contribution.supersedes.clone(),
                payload: contribution.payload.clone(),
                canonical_json: canonical_json(contribution)?,
                author_signature: Some(contribution.signature.clone()),
            },
        )?;
        apply_projection(&transaction, lineage_id, contribution, &accepted)?;
        transaction.commit()?;
        Ok(accepted)
    }

    pub fn append_acknowledgement(
        &self,
        identity: &NodeIdentity,
        lineage_id: &str,
        acknowledgement: &SignedAcknowledgement,
    ) -> Result<AcceptedObject> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        let current_head: i64 =
            transaction.query_row("SELECT COALESCE(MAX(sequence), 0) FROM events", [], |row| {
                row.get(0)
            })?;
        if acknowledgement.cursor > current_head {
            return Err(CommonwakeError::Validation(format!(
                "cursor {} is beyond current head {current_head}",
                acknowledgement.cursor
            )));
        }
        let previous_cursor: i64 = transaction
            .query_row(
                "SELECT cursor FROM acknowledgements WHERE lineage_id = ?1",
                [lineage_id],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(0);
        if acknowledgement.cursor < previous_cursor {
            return Err(CommonwakeError::Conflict(format!(
                "acknowledgement cursor cannot move backward from {previous_cursor}"
            )));
        }

        let accepted = append_event(
            &transaction,
            identity,
            &EventInput {
                kind: "acknowledgement".into(),
                lineage_id: Some(lineage_id.into()),
                delegation_id: Some(acknowledgement.delegation_id.clone()),
                created_at: acknowledgement.created_at,
                author_nonce: Some(acknowledgement.nonce.clone()),
                targets: vec![lineage_id.into()],
                supersedes: vec![],
                payload: serde_json::to_value(acknowledgement)?,
                canonical_json: canonical_json(acknowledgement)?,
                author_signature: Some(acknowledgement.signature.clone()),
            },
        )?;
        // When the caller has processed the current head, the acknowledgement
        // event itself is bookkeeping rather than new unread material. Consume
        // it atomically without skipping any event that arrived before this
        // transaction began.
        let effective_cursor = if acknowledgement.cursor == current_head {
            accepted.sequence
        } else {
            acknowledgement.cursor
        };
        transaction.execute(
            "INSERT INTO acknowledgements(
                lineage_id, cursor, event_id, memory_provenance_json, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(lineage_id) DO UPDATE SET
                cursor = excluded.cursor,
                event_id = excluded.event_id,
                memory_provenance_json = excluded.memory_provenance_json,
                updated_at = excluded.updated_at",
            params![
                lineage_id,
                effective_cursor,
                accepted.id,
                serde_json::to_string(&acknowledgement.memory_provenance)?,
                timestamp(acknowledgement.created_at),
            ],
        )?;
        transaction.commit()?;
        Ok(accepted)
    }

    pub fn append_observation(
        &self,
        identity: &NodeIdentity,
        observation: &IngestedObservation,
    ) -> Result<AcceptedObject> {
        let observation_id = prefixed_id(
            "cwobs_",
            format!(
                "{}\0{}\0{}",
                observation.source_id, observation.canonical_url, observation.document_hash
            )
            .as_bytes(),
        );
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        if let Some(raw) = transaction
            .query_row(
                "SELECT e.sequence, e.event_id, e.event_hash
                 FROM observations o JOIN events e ON e.sequence = o.created_sequence
                 WHERE o.observation_id = ?1",
                [&observation_id],
                |row| {
                    Ok(AcceptedObject {
                        sequence: row.get(0)?,
                        id: row.get(1)?,
                        event_hash: row.get(2)?,
                    })
                },
            )
            .optional()?
        {
            return Ok(raw);
        }
        if transaction
            .query_row(
                "SELECT 1 FROM sources
                 WHERE source_id = ?1 AND status IN ('probation', 'active', 'degraded')",
                [&observation.source_id],
                |_| Ok(()),
            )
            .optional()?
            .is_none()
        {
            return Err(CommonwakeError::Validation(
                "observations require a probation, active, or retryable degraded source".into(),
            ));
        }

        let existing_story: Option<String> = transaction
            .query_row(
                "SELECT so.story_id
                 FROM observations o
                 JOIN story_observations so ON so.observation_id = o.observation_id
                 WHERE o.source_id = ?1 AND o.canonical_url = ?2
                 ORDER BY o.created_sequence ASC LIMIT 1",
                params![observation.source_id, observation.canonical_url],
                |row| row.get(0),
            )
            .optional()?;
        let is_new_story = existing_story.is_none();
        let story_id =
            existing_story.unwrap_or_else(|| prefixed_id("cwstory_", observation_id.as_bytes()));
        let payload = json!({
            "observation_id": observation_id,
            "story_id": story_id,
            "source_id": observation.source_id,
            "canonical_url": observation.canonical_url,
            "title": observation.title,
            "summary": observation.summary,
            "published_at": observation.published_at,
            "retrieved_at": observation.retrieved_at,
            "language": observation.language,
            "document_hash": observation.document_hash,
            "raw_metadata": observation.raw_metadata,
        });
        let canonical = canonical_json(&json!({
            "protocol": crate::PROTOCOL_VERSION,
            "kind": "observation",
            "payload": payload,
        }))?;
        let accepted = append_event(
            &transaction,
            identity,
            &EventInput {
                kind: "observation".into(),
                lineage_id: None,
                delegation_id: None,
                created_at: observation.retrieved_at,
                author_nonce: None,
                targets: vec![story_id.clone(), observation.source_id.clone()],
                supersedes: vec![],
                payload,
                canonical_json: canonical,
                author_signature: None,
            },
        )?;
        transaction.execute(
            "INSERT OR IGNORE INTO stories(story_id, title, first_seen_at, created_sequence)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                story_id,
                observation.title,
                timestamp(observation.retrieved_at),
                accepted.sequence
            ],
        )?;
        transaction.execute(
            "INSERT INTO observations(
                observation_id, source_id, canonical_url, title, summary, published_at,
                retrieved_at, language, document_hash, raw_metadata_json, created_sequence
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                observation_id,
                observation.source_id,
                observation.canonical_url,
                observation.title,
                observation.summary,
                observation.published_at.map(timestamp),
                timestamp(observation.retrieved_at),
                observation.language,
                observation.document_hash,
                serde_json::to_string(&observation.raw_metadata)?,
                accepted.sequence,
            ],
        )?;
        transaction.execute(
            "INSERT INTO story_observations(story_id, observation_id)
             VALUES (?1, ?2)",
            params![story_id, observation_id],
        )?;
        insert_work_item(
            &transaction,
            "verify_observation",
            "observation",
            &observation_id,
            "Independently refetch or corroborate this observation; report evidence and disagreement.",
            2,
            accepted.sequence,
        )?;
        insert_work_item(
            &transaction,
            "assess_story",
            "story",
            &story_id,
            "Assess significance, claims, uncertainty, and missing perspectives with citations.",
            2,
            accepted.sequence,
        )?;
        if is_new_story {
            insert_cluster_candidates(
                &transaction,
                &story_id,
                &observation.title,
                accepted.sequence,
            )?;
        }
        transaction.commit()?;
        Ok(accepted)
    }

    pub fn mark_source_fetch(&self, source_id: &str, success: bool) -> Result<()> {
        let connection = self.lock()?;
        if success {
            connection.execute(
                "UPDATE sources SET successful_fetches = successful_fetches + 1,
                    consecutive_failures = 0, last_fetched_at = ?2,
                    status = CASE
                        WHEN status = 'degraded' THEN 'active'
                        WHEN status = 'probation' AND successful_fetches >= 9 THEN 'active'
                        ELSE status
                    END
                 WHERE source_id = ?1",
                params![source_id, timestamp(Utc::now())],
            )?;
        } else {
            connection.execute(
                "UPDATE sources SET consecutive_failures = consecutive_failures + 1,
                    last_fetched_at = ?2,
                    status = CASE WHEN consecutive_failures >= 2 AND status = 'active'
                                  THEN 'degraded' ELSE status END
                 WHERE source_id = ?1",
                params![source_id, timestamp(Utc::now())],
            )?;
        }
        Ok(())
    }

    pub fn ingestible_sources(&self) -> Result<Vec<SourceView>> {
        self.sources(Some(&["probation", "active", "degraded"]))
    }

    pub fn sources(&self, statuses: Option<&[&str]>) -> Result<Vec<SourceView>> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT s.source_id, s.name, s.feed_url, s.homepage_url, s.medium,
                    s.primary_regions_json, s.languages_json, s.ownership,
                    s.perspective_notes, s.status, s.proposer_lineage_id,
                    SUM(CASE WHEN r.recommendation = 'approve' THEN 1 ELSE 0 END),
                    SUM(CASE WHEN r.recommendation = 'reject' THEN 1 ELSE 0 END),
                    s.successful_fetches, s.consecutive_failures, s.last_fetched_at
             FROM sources s LEFT JOIN source_reviews r ON r.source_id = s.source_id
             GROUP BY s.source_id ORDER BY s.created_at ASC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, String>(10)?,
                row.get::<_, i64>(11)?,
                row.get::<_, i64>(12)?,
                row.get::<_, i64>(13)?,
                row.get::<_, i64>(14)?,
                row.get::<_, Option<String>>(15)?,
            ))
        })?;
        let mut sources = Vec::new();
        for row in rows {
            let row = row?;
            if statuses.is_some_and(|allowed| !allowed.contains(&row.9.as_str())) {
                continue;
            }
            sources.push(SourceView {
                source_id: row.0,
                name: row.1,
                feed_url: row.2,
                homepage_url: row.3,
                medium: row.4,
                primary_regions: serde_json::from_str(&row.5)?,
                languages: serde_json::from_str(&row.6)?,
                ownership: row.7,
                perspective_notes: row.8,
                status: row.9,
                proposer_lineage_id: row.10,
                approval_count: row.11,
                rejection_count: row.12,
                successful_fetches: row.13,
                consecutive_failures: row.14,
                last_fetched_at: row.15.map(|value| parse_timestamp(&value)).transpose()?,
            });
        }
        Ok(sources)
    }

    pub fn coverage_report(&self) -> Result<CoverageReport> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT 'local', status, medium, primary_regions_json, languages_json, ownership
             FROM sources
             UNION ALL
             SELECT 'federated', status, medium, primary_regions_json, languages_json, ownership
             FROM federated_sources",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })?;

        let mut local_source_manifests = 0_usize;
        let mut federated_source_manifests = 0_usize;
        let mut eligible_source_manifests = 0_usize;
        let mut by_status = BTreeMap::new();
        let mut by_region_or_coverage_tag = BTreeMap::new();
        let mut by_language = BTreeMap::new();
        let mut by_medium = BTreeMap::new();
        let mut by_ownership = BTreeMap::new();
        let mut missing_ownership_manifests = 0_usize;
        let mut eligible_by_area = BTreeMap::<String, usize>::new();
        let mut proposed_by_area = BTreeMap::<String, usize>::new();

        for row in rows {
            let (origin, status, medium, regions_json, languages_json, ownership) = row?;
            if origin == "local" {
                local_source_manifests += 1;
            } else {
                federated_source_manifests += 1;
            }
            *by_status.entry(status.clone()).or_insert(0) += 1;
            let eligible = matches!(status.as_str(), "probation" | "active");
            let proposed = status == "proposed";
            if !eligible {
                if proposed {
                    let regions: BTreeSet<String> =
                        serde_json::from_str::<Vec<String>>(&regions_json)?
                            .into_iter()
                            .map(|value| coverage_key(&value))
                            .filter(|value| !value.is_empty())
                            .collect();
                    for region in regions {
                        *proposed_by_area.entry(region).or_insert(0) += 1;
                    }
                }
                continue;
            }
            eligible_source_manifests += 1;
            let regions: BTreeSet<String> = serde_json::from_str::<Vec<String>>(&regions_json)?
                .into_iter()
                .map(|value| coverage_key(&value))
                .filter(|value| !value.is_empty())
                .collect();
            for region in regions {
                *by_region_or_coverage_tag.entry(region.clone()).or_insert(0) += 1;
                *eligible_by_area.entry(region).or_insert(0) += 1;
            }
            let languages: BTreeSet<String> = serde_json::from_str::<Vec<String>>(&languages_json)?
                .into_iter()
                .map(|value| coverage_key(&value))
                .filter(|value| !value.is_empty())
                .collect();
            for language in languages {
                *by_language.entry(language).or_insert(0) += 1;
            }
            let medium = coverage_key(&medium);
            if !medium.is_empty() {
                *by_medium.entry(medium).or_insert(0) += 1;
            }
            if let Some(ownership) = ownership
                .map(|value| coverage_key(&value))
                .filter(|value| !value.is_empty())
            {
                *by_ownership.entry(ownership).or_insert(0) += 1;
            } else {
                missing_ownership_manifests += 1;
            }
        }

        let dominant_ownership = by_ownership
            .iter()
            .max_by_key(|(_, count)| *count)
            .filter(|(_, count)| {
                eligible_source_manifests > 0 && **count * 2 > eligible_source_manifests
            })
            .map(
                |(ownership_label, source_manifests)| OwnershipConcentrationView {
                    ownership_label: ownership_label.clone(),
                    source_manifests: *source_manifests,
                    eligible_source_manifests,
                },
            );
        let standing_gaps = BOOTSTRAP_SOURCE_COVERAGE
            .iter()
            .map(|(coverage_area, _)| {
                let eligible_sources = eligible_by_area.get(*coverage_area).copied().unwrap_or(0);
                let proposed_sources = proposed_by_area.get(*coverage_area).copied().unwrap_or(0);
                let status = if eligible_sources > 0 {
                    "covered"
                } else if proposed_sources > 0 {
                    "proposed_only"
                } else {
                    "uncovered"
                };
                CoverageGapView {
                    coverage_area: (*coverage_area).into(),
                    eligible_sources,
                    proposed_sources,
                    status: status.into(),
                    standing_work_id: prefixed_id(
                        "cwwork_",
                        format!("discover_sources\0{coverage_area}").as_bytes(),
                    ),
                }
            })
            .collect();

        Ok(CoverageReport {
            generated_at: Utc::now(),
            local_source_manifests,
            federated_source_manifests,
            eligible_source_manifests,
            by_status,
            by_region_or_coverage_tag,
            by_language,
            by_medium,
            by_ownership,
            missing_ownership_manifests,
            dominant_ownership,
            standing_gaps,
            methodology_notice: "Counts describe eligible source manifests (probation or active) by self-declared metadata and origin; they are not truth, quality, ideology, or viewpoint scores. Federated duplicates remain visible as separate origin manifests. China-related facets are plurality checks, not quotas or assumptions that unlike claims are morally or evidentially equivalent.".into(),
        })
    }

    pub fn events_after(&self, after: i64, limit: usize) -> Result<Vec<EventView>> {
        let connection = self.lock()?;
        event_views_between(&connection, after, i64::MAX, limit)
    }

    pub fn last_acknowledged_cursor(&self, lineage_id: &str) -> Result<i64> {
        let connection = self.lock()?;
        Ok(connection
            .query_row(
                "SELECT cursor FROM acknowledgements WHERE lineage_id = ?1",
                [lineage_id],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(0))
    }

    pub fn open_commitments(&self, lineage_id: &str) -> Result<Vec<EventView>> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT e.sequence, e.event_id, e.kind, e.lineage_id, e.delegation_id,
                    e.created_at, e.received_at, e.targets_json, e.supersedes_json,
                    e.payload_json, e.canonical_json, e.author_signature,
                    e.previous_hash, e.event_hash, e.node_signature, e.author_nonce
             FROM events e
             WHERE e.lineage_id = ?1 AND e.kind = 'commitment'
               AND NOT EXISTS (
                   SELECT 1 FROM events later, json_each(later.supersedes_json)
                   WHERE json_each.value = e.event_id
               )
             ORDER BY e.sequence ASC LIMIT 100",
        )?;
        let rows = statement.query_map([lineage_id], raw_event_from_row)?;
        raw_rows_to_views(rows)
    }

    pub fn stories_changed_between(&self, after: i64, through: i64) -> Result<Vec<StoryView>> {
        let connection = self.lock()?;
        stories_changed_between_connection(&connection, after, through)
    }

    pub fn federated_stories_changed_between(
        &self,
        after: i64,
        through: i64,
    ) -> Result<Vec<FederatedStoryView>> {
        let connection = self.lock()?;
        federated_stories_changed_between_connection(&connection, after, through)
    }

    pub fn feed(&self, after: i64, limit: usize, stage: Option<&str>) -> Result<FeedPage> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT story_id, created_sequence FROM stories
             WHERE merged_into IS NULL AND created_sequence > ?1
             ORDER BY created_sequence ASC LIMIT ?2",
        )?;
        let rows = statement.query_map(params![after, (limit + 1) as i64], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        let mut ids = Vec::new();
        for row in rows {
            ids.push(row?);
        }
        let has_more = ids.len() > limit;
        ids.truncate(limit);
        let mut stories = Vec::new();
        let mut next_cursor = after;
        for (id, cursor) in ids {
            let story = story_view(&connection, &id)?;
            next_cursor = next_cursor.max(cursor);
            if stage.is_none_or(|wanted| wanted == story.stage) {
                stories.push(story);
            }
        }
        Ok(FeedPage {
            stories,
            after,
            next_cursor,
            has_more,
        })
    }

    pub fn story(&self, story_id: &str) -> Result<StoryView> {
        let connection = self.lock()?;
        story_view(&connection, story_id)
    }

    pub fn work(&self, limit: usize) -> Result<Vec<WorkItemView>> {
        Ok(self.work_page(None, limit, None)?.items)
    }

    pub fn work_page(
        &self,
        after: Option<&str>,
        limit: usize,
        kind: Option<&str>,
    ) -> Result<WorkPage> {
        let (after_sequence, after_work_id) = match after {
            Some(cursor) => parse_work_cursor(cursor)?,
            None => (-1, String::new()),
        };
        let connection = self.lock()?;
        refresh_replication_work(&connection)?;
        let mut statement = connection.prepare(
            "SELECT work_id, kind, subject_type, subject_id, instructions,
                    required_results, created_sequence
             FROM work_items
             WHERE status = 'open'
               AND (created_sequence > ?1 OR (created_sequence = ?1 AND work_id > ?2))
               AND (?3 IS NULL OR kind = ?3)
             ORDER BY created_sequence ASC, work_id ASC LIMIT ?4",
        )?;
        let query_limit = limit.saturating_add(1).min(i64::MAX as usize) as i64;
        let rows = statement.query_map(
            params![after_sequence, after_work_id, kind, query_limit],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                ))
            },
        )?;
        let mut work = Vec::new();
        for row in rows {
            let row = row?;
            let received_results = work_result_count(&connection, &row.1, &row.3)?;
            let active_claims: i64 = connection.query_row(
                "SELECT COUNT(*) FROM work_claims WHERE work_id = ?1 AND expires_at > ?2",
                params![&row.0, timestamp(Utc::now())],
                |claim_row| claim_row.get(0),
            )?;
            work.push(WorkItemView {
                work_id: row.0,
                kind: row.1,
                subject_type: row.2,
                subject_id: row.3,
                instructions: row.4,
                required_results: row.5,
                received_results,
                active_claims,
                created_sequence: row.6,
            });
        }
        let has_more = work.len() > limit;
        work.truncate(limit);
        let next_cursor = work
            .last()
            .map(|item| format_work_cursor(item.created_sequence, &item.work_id));
        Ok(WorkPage {
            items: work,
            after: after.map(str::to_owned),
            next_cursor,
            has_more,
            kind: kind.map(str::to_owned),
        })
    }

    pub fn federation_peers(&self) -> Result<Vec<FederationPeerView>> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT node_id, node_public_key, first_seen_at, last_seen_at, cursor, event_hash
             FROM federation_peers ORDER BY first_seen_at ASC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;
        let mut peers = Vec::new();
        for row in rows {
            let row = row?;
            peers.push(FederationPeerView {
                node_id: row.0,
                node_public_key: row.1,
                first_seen_at: parse_timestamp(&row.2)?,
                last_seen_at: parse_timestamp(&row.3)?,
                cursor: row.4,
                event_hash: row.5,
            });
        }
        Ok(peers)
    }

    pub fn configure_publication_target(&self, endpoint: &str) -> Result<()> {
        if endpoint.is_empty() || endpoint.len() > 2_048 {
            return Err(CommonwakeError::Validation(
                "publication endpoint must contain 1 to 2048 characters".into(),
            ));
        }
        let now = timestamp(Utc::now());
        let connection = self.lock()?;
        connection.execute(
            "INSERT OR IGNORE INTO publication_targets(
                endpoint, created_at, updated_at
             ) VALUES (?1, ?2, ?2)",
            params![endpoint, now],
        )?;
        refresh_replication_work(&connection)?;
        Ok(())
    }

    pub fn set_desired_replicas(&self, desired_replicas: u32) -> Result<()> {
        if !(1..=16).contains(&desired_replicas) {
            return Err(CommonwakeError::Validation(
                "desired replicas must be between 1 and 16".into(),
            ));
        }
        let connection = self.lock()?;
        connection.execute(
            "INSERT INTO meta(key, value) VALUES ('desired_replicas', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [desired_replicas.to_string()],
        )?;
        refresh_replication_work(&connection)?;
        Ok(())
    }

    pub fn record_publication_success(
        &self,
        identity: &NodeIdentity,
        endpoint: &str,
        receipt: &ReplicationReceipt,
    ) -> Result<()> {
        verify_replication_receipt(receipt)?;
        let checkpoint = &receipt.origin_checkpoint;
        if checkpoint.node_id != identity.node_id()
            || checkpoint.node_public_key != identity.public_key()
        {
            return Err(CommonwakeError::Unauthorized(
                "replication receipt describes a different origin".into(),
            ));
        }

        let connection = self.lock()?;
        let local_event_hash = if checkpoint.cursor == 0 {
            Some(ZERO_HASH.into())
        } else {
            connection
                .query_row(
                    "SELECT event_hash FROM events WHERE sequence = ?1",
                    [checkpoint.cursor],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
        };
        if local_event_hash.as_deref() != Some(&checkpoint.event_hash) {
            return Err(CommonwakeError::Unauthorized(
                "replication receipt checkpoint is not on the local origin hash chain".into(),
            ));
        }
        let stored: Option<(Option<String>, Option<String>, i64, String)> = connection
            .query_row(
                "SELECT relay_node_id, relay_node_public_key,
                        acknowledged_cursor, acknowledged_event_hash
                 FROM publication_targets WHERE endpoint = ?1",
                [endpoint],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        let Some((relay_node_id, relay_node_public_key, cursor, event_hash)) = stored else {
            return Err(CommonwakeError::NotFound(format!(
                "publication target {endpoint}"
            )));
        };
        if relay_node_id
            .as_deref()
            .is_some_and(|known| known != receipt.relay_node_id)
            || relay_node_public_key
                .as_deref()
                .is_some_and(|known| known != receipt.relay_node_public_key)
        {
            return Err(CommonwakeError::Conflict(format!(
                "publication target {endpoint} changed relay identity"
            )));
        }
        if checkpoint.cursor < cursor
            || (checkpoint.cursor == cursor && checkpoint.event_hash != event_hash)
        {
            return Err(CommonwakeError::Conflict(
                "replication receipt regresses or conflicts with the acknowledged origin head"
                    .into(),
            ));
        }

        let now = timestamp(Utc::now());
        connection.execute(
            "UPDATE publication_targets SET
                relay_node_id = ?2,
                relay_node_public_key = ?3,
                acknowledged_cursor = ?4,
                acknowledged_event_hash = ?5,
                receipt_json = ?6,
                updated_at = ?7,
                last_attempt_at = ?7,
                last_success_at = ?7,
                consecutive_failures = 0,
                next_attempt_at = NULL,
                last_error = NULL
             WHERE endpoint = ?1",
            params![
                endpoint,
                receipt.relay_node_id,
                receipt.relay_node_public_key,
                checkpoint.cursor,
                checkpoint.event_hash,
                serde_json::to_string(receipt)?,
                now,
            ],
        )?;
        refresh_replication_work(&connection)?;
        Ok(())
    }

    pub fn record_publication_failure(
        &self,
        endpoint: &str,
        error: &str,
        next_attempt_at: DateTime<Utc>,
    ) -> Result<()> {
        let now = timestamp(Utc::now());
        let bounded_error: String = error.chars().take(1_024).collect();
        let connection = self.lock()?;
        let updated = connection.execute(
            "UPDATE publication_targets SET
                updated_at = ?2,
                last_attempt_at = ?2,
                consecutive_failures = consecutive_failures + 1,
                next_attempt_at = ?3,
                last_error = ?4
             WHERE endpoint = ?1",
            params![endpoint, now, timestamp(next_attempt_at), bounded_error],
        )?;
        if updated == 0 {
            return Err(CommonwakeError::NotFound(format!(
                "publication target {endpoint}"
            )));
        }
        refresh_replication_work(&connection)?;
        Ok(())
    }

    pub fn replication_health(&self, identity: &NodeIdentity) -> Result<ReplicationHealth> {
        const RECENT_CONFIRMATION_HOURS: i64 = 24;

        let now = Utc::now();
        let recent_after = now - Duration::hours(RECENT_CONFIRMATION_HOURS);
        let connection = self.lock()?;
        let desired_replicas: u32 = connection
            .query_row(
                "SELECT value FROM meta WHERE key = 'desired_replicas'",
                [],
                |row| row.get::<_, String>(0),
            )?
            .parse()
            .map_err(|_| CommonwakeError::Internal("stored replica target is malformed".into()))?;
        let (current_cursor, current_event_hash) = connection
            .query_row(
                "SELECT sequence, event_hash FROM events ORDER BY sequence DESC LIMIT 1",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?
            .unwrap_or((0, ZERO_HASH.into()));
        let mut statement = connection.prepare(
            "SELECT endpoint, relay_node_id, relay_node_public_key,
                    acknowledged_cursor, acknowledged_event_hash, receipt_json,
                    last_attempt_at, last_success_at, consecutive_failures,
                    next_attempt_at, last_error
             FROM publication_targets ORDER BY created_at ASC, endpoint ASC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, i64>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, Option<String>>(10)?,
            ))
        })?;
        let mut targets = Vec::new();
        let mut confirmed = BTreeSet::new();
        let mut recent = BTreeSet::new();
        for row in rows {
            let row = row?;
            let receipt = row
                .5
                .as_deref()
                .map(serde_json::from_str::<ReplicationReceipt>)
                .transpose()?;
            if let Some(receipt) = &receipt {
                verify_replication_receipt(receipt)?;
                if receipt.origin_checkpoint.node_id != identity.node_id()
                    || receipt.origin_checkpoint.node_public_key != identity.public_key()
                    || row.1.as_deref() != Some(&receipt.relay_node_id)
                    || row.2.as_deref() != Some(&receipt.relay_node_public_key)
                    || row.3 != receipt.origin_checkpoint.cursor
                    || row.4 != receipt.origin_checkpoint.event_hash
                {
                    return Err(CommonwakeError::Internal(format!(
                        "stored replication receipt for {} does not match its publication state",
                        row.0
                    )));
                }
            }
            let last_attempt_at = row.6.as_deref().map(parse_timestamp).transpose()?;
            let last_success_at = row.7.as_deref().map(parse_timestamp).transpose()?;
            let next_attempt_at = row.9.as_deref().map(parse_timestamp).transpose()?;
            let at_current_head =
                receipt.is_some() && row.3 == current_cursor && row.4 == current_event_hash;
            let recently_reconfirmed = at_current_head
                && last_success_at.is_some_and(|confirmed_at| confirmed_at >= recent_after);
            if at_current_head && let Some(relay_node_id) = &row.1 {
                confirmed.insert(relay_node_id.clone());
                if recently_reconfirmed {
                    recent.insert(relay_node_id.clone());
                }
            }
            targets.push(PublicationTargetView {
                endpoint: row.0,
                relay_node_id: row.1,
                relay_node_public_key: row.2,
                acknowledged_cursor: row.3,
                acknowledged_event_hash: row.4,
                at_current_head,
                recently_reconfirmed,
                last_attempt_at,
                last_success_at,
                consecutive_failures: u32::try_from(row.8).map_err(|_| {
                    CommonwakeError::Internal("stored publication failure count is invalid".into())
                })?,
                next_attempt_at,
                last_error: row.10,
                receipt,
            });
        }
        let status = if targets.is_empty() {
            "unconfigured"
        } else if recent.len() >= desired_replicas as usize {
            "replicated"
        } else if recent.is_empty() {
            "unreplicated"
        } else {
            "degraded"
        };
        Ok(ReplicationHealth {
            generated_at: now,
            origin_node_id: identity.node_id().into(),
            current_cursor,
            current_event_hash,
            desired_replicas,
            confirmed_current_replicas: confirmed.len(),
            recently_reconfirmed_current_replicas: recent.len(),
            status: status.into(),
            targets,
            receipt_notice: format!(
                "A signed receipt attributes a retention claim to a relay; recent means confirmed by this node within {RECENT_CONFIRMATION_HOURS} hours and does not prove present availability."
            ),
        })
    }

    pub fn remote_events(
        &self,
        origin_node_id: &str,
        after: i64,
        limit: usize,
    ) -> Result<Vec<OriginEvent>> {
        if after < 0 {
            return Err(CommonwakeError::Validation(
                "remote event cursor cannot be negative".into(),
            ));
        }
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT origin_sequence, event_id, kind, lineage_id, delegation_id,
                    created_at, received_at, canonical_json, previous_hash,
                    event_hash, node_signature
             FROM remote_events
             WHERE origin_node_id = ?1 AND origin_sequence > ?2
             ORDER BY origin_sequence ASC LIMIT ?3",
        )?;
        let rows = statement.query_map(
            params![origin_node_id, after, limit as i64],
            remote_origin_event_from_row,
        )?;
        let mut events = Vec::new();
        for row in rows {
            events.push(row?);
        }
        Ok(events)
    }

    pub fn relayed_federation_bundle(
        &self,
        origin_node_id: &str,
        after: i64,
        limit: usize,
    ) -> Result<FederationBundle> {
        if after < 0 {
            return Err(CommonwakeError::Validation(
                "federation cursor cannot be negative".into(),
            ));
        }
        let connection = self.lock()?;
        let (origin_node_public_key, head): (String, i64) = connection
            .query_row(
                "SELECT node_public_key, cursor FROM federation_peers WHERE node_id = ?1",
                [origin_node_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .ok_or_else(|| {
                CommonwakeError::NotFound(format!("federated origin {origin_node_id}"))
            })?;
        if after > head {
            return Err(CommonwakeError::Validation(format!(
                "federation cursor {after} is beyond retained origin head {head}"
            )));
        }
        let requested_end = after
            .saturating_add(limit.clamp(1, crate::federation::MAX_FEDERATION_EVENTS) as i64)
            .min(head);
        let through_cursor = if after == head {
            head
        } else {
            connection
                .query_row(
                    "SELECT MAX(cursor) FROM remote_checkpoints
                     WHERE origin_node_id = ?1 AND cursor > ?2 AND cursor <= ?3",
                    params![origin_node_id, after, requested_end],
                    |row| row.get::<_, Option<i64>>(0),
                )?
                .or(connection.query_row(
                    "SELECT MIN(cursor) FROM remote_checkpoints
                     WHERE origin_node_id = ?1 AND cursor > ?2",
                    params![origin_node_id, after],
                    |row| row.get::<_, Option<i64>>(0),
                )?)
                .ok_or_else(|| {
                    CommonwakeError::Internal(
                        "retained origin has no checkpoint beyond its stored cursor".into(),
                    )
                })?
        };
        let checkpoint_json: String = connection
            .query_row(
                "SELECT checkpoint_json FROM remote_checkpoints
                 WHERE origin_node_id = ?1 AND cursor = ?2
                 ORDER BY imported_at DESC LIMIT 1",
                params![origin_node_id, through_cursor],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| {
                CommonwakeError::Internal(format!(
                    "retained origin checkpoint {origin_node_id}/{through_cursor} is missing"
                ))
            })?;
        let checkpoint = serde_json::from_str(&checkpoint_json)?;
        let mut statement = connection.prepare(
            "SELECT origin_sequence, event_id, kind, lineage_id, delegation_id,
                    created_at, received_at, canonical_json, previous_hash,
                    event_hash, node_signature
             FROM remote_events
             WHERE origin_node_id = ?1 AND origin_sequence > ?2 AND origin_sequence <= ?3
             ORDER BY origin_sequence ASC",
        )?;
        let rows = statement.query_map(
            params![origin_node_id, after, through_cursor],
            remote_origin_event_from_row,
        )?;
        let mut events = Vec::new();
        for row in rows {
            events.push(row?);
        }
        let bundle = FederationBundle {
            protocol: crate::PROTOCOL_VERSION.into(),
            origin_node_id: origin_node_id.into(),
            origin_node_public_key,
            from_cursor: after,
            through_cursor,
            events,
            checkpoint,
        };
        crate::federation::verify_bundle(&bundle)?;
        Ok(bundle)
    }

    pub fn equivocation_evidence(&self) -> Result<Vec<EquivocationEvidenceView>> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT evidence_id, origin_node_id, conflict_kind, cursor, existing_hash,
                    incoming_hash, existing_json, incoming_json, detected_at
             FROM equivocation_evidence ORDER BY detected_at ASC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
            ))
        })?;
        let mut evidence = Vec::new();
        for row in rows {
            let row = row?;
            evidence.push(EquivocationEvidenceView {
                evidence_id: row.0,
                origin_node_id: row.1,
                conflict_kind: row.2,
                cursor: row.3,
                existing_hash: row.4,
                incoming_hash: row.5,
                existing: serde_json::from_str(&row.6)?,
                incoming: serde_json::from_str(&row.7)?,
                detected_at: parse_timestamp(&row.8)?,
            });
        }
        Ok(evidence)
    }

    pub fn federated_stories(
        &self,
        origin_node_id: Option<&str>,
        after: i64,
        limit: usize,
        stage: Option<&str>,
    ) -> Result<FederatedFeedPage> {
        if after < 0 {
            return Err(CommonwakeError::Validation(
                "federated feed cursor cannot be negative".into(),
            ));
        }
        if origin_node_id.is_none() && after != 0 {
            return Err(CommonwakeError::Validation(
                "federated_after requires origin_node_id; there is no fabricated global origin cursor"
                    .into(),
            ));
        }
        let connection = self.lock()?;
        let mut ids = Vec::new();
        if let Some(origin_node_id) = origin_node_id {
            let mut statement = connection.prepare(
                "SELECT origin_node_id, story_id, created_sequence FROM federated_stories
                 WHERE origin_node_id = ?1 AND merged_into IS NULL AND created_sequence > ?2
                 ORDER BY created_sequence ASC LIMIT ?3",
            )?;
            let rows =
                statement.query_map(params![origin_node_id, after, (limit + 1) as i64], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                })?;
            for row in rows {
                ids.push(row?);
            }
        } else {
            let mut statement = connection.prepare(
                "SELECT origin_node_id, story_id, created_sequence FROM federated_stories
                 WHERE merged_into IS NULL
                 ORDER BY first_seen_at ASC, origin_node_id ASC, created_sequence ASC LIMIT ?1",
            )?;
            let rows = statement.query_map([(limit + 1) as i64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?;
            for row in rows {
                ids.push(row?);
            }
        }
        let has_more = ids.len() > limit;
        ids.truncate(limit);
        let mut stories = Vec::new();
        let mut next_cursor = after;
        for (origin, story_id, cursor) in ids {
            let story = federated_story_view(&connection, &origin, &story_id)?;
            next_cursor = next_cursor.max(cursor);
            if stage.is_none_or(|wanted| wanted == story.stage) {
                stories.push(story);
            }
        }
        Ok(FederatedFeedPage {
            origin_node_id: origin_node_id.map(str::to_owned),
            stories,
            after: origin_node_id.map(|_| after),
            next_cursor: origin_node_id.map(|_| next_cursor),
            has_more,
            pagination_notice: if origin_node_id.is_some() {
                "Pass this origin_node_id and federated_after=next_cursor to continue snapshot traversal. This is the story-creation sequence at that origin, not wall-clock time or an incremental change cursor; use lineage orientation for changed-story replay."
            } else {
                "This is a bounded multi-origin preview. Enumerate /v1/federation/peers and request each origin_node_id separately for complete cursor-based traversal; Commonwake does not invent a global order."
            }
            .into(),
        })
    }

    pub fn pulse_counts(&self, lineage_id: &str, after: i64) -> Result<(i64, i64)> {
        let connection = self.lock()?;
        let directed: i64 = connection.query_row(
            "SELECT COUNT(*) FROM events e
             WHERE e.sequence > ?1 AND e.kind != 'acknowledgement' AND (
                e.lineage_id = ?2 OR EXISTS (
                    SELECT 1 FROM json_each(e.targets_json) WHERE value = ?2
                )
             )",
            params![after, lineage_id],
            |row| row.get(0),
        )?;
        let local_world = changed_story_ids(&connection, after, i64::MAX)?.len();
        let federated_world = federated_changed_story_ids(&connection, after, i64::MAX)?.len();
        let world = i64::try_from(local_world + federated_world)
            .map_err(|_| CommonwakeError::Internal("world change count overflowed".into()))?;
        Ok((directed, world))
    }

    pub fn verify_log(&self, identity: &NodeIdentity) -> Result<(i64, String)> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT sequence, event_id, kind, lineage_id, delegation_id, created_at,
                    received_at, targets_json, supersedes_json, payload_json,
                    canonical_json, author_signature, previous_hash, event_hash, node_signature,
                    author_nonce
             FROM events ORDER BY sequence ASC",
        )?;
        let rows = statement.query_map([], raw_event_from_row)?;
        let public_key = verifying_key_from_b64(identity.public_key())?;
        let mut previous = [0_u8; 32];
        let mut cursor = 0;
        let mut head = ZERO_HASH.to_owned();
        for (expected_sequence, row) in (1_i64..).zip(rows) {
            let row = row?;
            if row.sequence != expected_sequence {
                return Err(CommonwakeError::Unauthorized(format!(
                    "event sequence {} is not the expected contiguous sequence {expected_sequence}",
                    row.sequence
                )));
            }
            if row.previous_hash != hex::encode(previous) {
                return Err(CommonwakeError::Unauthorized(format!(
                    "event {} has a broken previous hash",
                    row.sequence
                )));
            }
            let canonical_event = canonical_log_record(&row)?;
            let expected = event_hash(&previous, &canonical_event);
            if row.event_hash != hex::encode(expected) {
                return Err(CommonwakeError::Unauthorized(format!(
                    "event {} has an invalid event hash",
                    row.sequence
                )));
            }
            let signature = signature_from_b64(&row.node_signature)?;
            public_key.verify(&expected, &signature).map_err(|_| {
                CommonwakeError::Unauthorized(format!(
                    "event {} has an invalid node signature",
                    row.sequence
                ))
            })?;
            validate_raw_event_projection(&row)?;
            previous = expected;
            cursor = row.sequence;
            head = row.event_hash;
        }
        Ok((cursor, head))
    }
}

fn lineage_from_connection(connection: &Connection, lineage_id: &str) -> Result<LineageView> {
    let raw = connection
        .query_row(
            "SELECT lineage_id, display_name, public_key, created_at, registered_sequence,
                    (SELECT COALESCE(MAX(key_version), 0) FROM lineage_keys k
                     WHERE k.lineage_id = lineages.lineage_id)
             FROM lineages WHERE lineage_id = ?1",
            [lineage_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| CommonwakeError::NotFound(format!("lineage {lineage_id}")))?;
    Ok(LineageView {
        lineage_id: raw.0,
        display_name: raw.1,
        public_key: raw.2,
        created_at: parse_timestamp(&raw.3)?,
        registered_sequence: raw.4,
        key_version: raw.5,
    })
}

fn append_event(
    transaction: &Transaction<'_>,
    identity: &NodeIdentity,
    input: &EventInput,
) -> Result<AcceptedObject> {
    if input.canonical_json.len() > MAX_CANONICAL_OBJECT_BYTES {
        return Err(CommonwakeError::Validation(format!(
            "canonical protocol object exceeds {MAX_CANONICAL_OBJECT_BYTES} bytes"
        )));
    }
    let previous_hash: String = transaction
        .query_row(
            "SELECT event_hash FROM events ORDER BY sequence DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or_else(|| ZERO_HASH.into());
    let previous_bytes: [u8; 32] = hex::decode(&previous_hash)
        .map_err(|_| CommonwakeError::Internal("stored previous hash is malformed".into()))?
        .try_into()
        .map_err(|_| CommonwakeError::Internal("stored previous hash is not 32 bytes".into()))?;
    let received_at = Utc::now();
    let event_id = prefixed_id("cwevt_", input.canonical_json.as_bytes());
    let canonical_event = canonical_log_input(input, received_at)?;
    let hash = event_hash(&previous_bytes, &canonical_event);
    let hash_hex = hex::encode(hash);
    let node_signature = identity.sign_hash(&hash);
    transaction.execute(
        "INSERT INTO events(
            event_id, kind, lineage_id, delegation_id, created_at, received_at,
            author_nonce, targets_json, supersedes_json, payload_json, canonical_json,
            author_signature, previous_hash, event_hash, node_signature
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        params![
            event_id,
            input.kind,
            input.lineage_id,
            input.delegation_id,
            timestamp(input.created_at),
            timestamp(received_at),
            input.author_nonce,
            serde_json::to_string(&input.targets)?,
            serde_json::to_string(&input.supersedes)?,
            serde_json::to_string(&input.payload)?,
            input.canonical_json,
            input.author_signature,
            previous_hash,
            hash_hex,
            node_signature,
        ],
    )?;
    Ok(AcceptedObject {
        id: event_id,
        sequence: transaction.last_insert_rowid(),
        event_hash: hash_hex,
    })
}

fn canonical_log_input(input: &EventInput, received_at: DateTime<Utc>) -> Result<Vec<u8>> {
    let canonical_value: Value = serde_json::from_str(&input.canonical_json)?;
    serde_jcs::to_vec(&json!({
        "kind": input.kind,
        "lineage_id": input.lineage_id,
        "delegation_id": input.delegation_id,
        "created_at": timestamp(input.created_at),
        "received_at": timestamp(received_at),
        "canonical": canonical_value,
    }))
    .map_err(|error| CommonwakeError::Internal(format!("canonical event failed: {error}")))
}

fn canonical_log_record(row: &RawEvent) -> Result<Vec<u8>> {
    let canonical_value: Value = serde_json::from_str(&row.canonical_json)?;
    serde_jcs::to_vec(&json!({
        "kind": row.kind,
        "lineage_id": row.lineage_id,
        "delegation_id": row.delegation_id,
        "created_at": row.created_at,
        "received_at": row.received_at,
        "canonical": canonical_value,
    }))
    .map_err(|error| CommonwakeError::Internal(format!("canonical event failed: {error}")))
}

fn validate_raw_event_projection(row: &RawEvent) -> Result<()> {
    let canonical: Value = serde_json::from_str(&row.canonical_json)?;
    let canonical_bytes = serde_jcs::to_vec(&canonical)
        .map_err(|error| CommonwakeError::Internal(format!("canonical JSON failed: {error}")))?;
    if canonical_bytes.as_slice() != row.canonical_json.as_bytes() {
        return Err(CommonwakeError::Unauthorized(format!(
            "event {} canonical JSON is not in canonical form",
            row.sequence
        )));
    }
    if prefixed_id("cwevt_", &canonical_bytes) != row.event_id {
        return Err(CommonwakeError::Unauthorized(format!(
            "event {} has an invalid content id",
            row.sequence
        )));
    }

    let expected = expected_projection(row, canonical)?;
    let stored_targets: Vec<String> = serde_json::from_str(&row.targets_json)?;
    let stored_supersedes: Vec<String> = serde_json::from_str(&row.supersedes_json)?;
    let stored_payload: Value = serde_json::from_str(&row.payload_json)?;
    if stored_targets != expected.targets
        || stored_supersedes != expected.supersedes
        || stored_payload != expected.payload
        || row.author_signature != expected.author_signature
        || row.author_nonce != expected.author_nonce
    {
        return Err(CommonwakeError::Unauthorized(format!(
            "event {} mutable projection differs from its authenticated canonical object",
            row.sequence
        )));
    }
    Ok(())
}

fn expected_projection(row: &RawEvent, canonical: Value) -> Result<AuthenticatedProjection> {
    match row.kind.as_str() {
        "lineage_registered" => {
            let object: LineageRegistration = serde_json::from_value(canonical.clone())?;
            let key = verifying_key_from_b64(&object.public_key)?;
            Ok(AuthenticatedProjection {
                targets: vec![lineage_id(&key)],
                supersedes: vec![],
                payload: canonical,
                author_signature: Some(object.signature),
                author_nonce: Some(object.nonce),
            })
        }
        "delegation_registered" => {
            let object: SessionDelegation = serde_json::from_value(canonical.clone())?;
            let id = prefixed_id("cwdel_", &canonical_without_signature(&object)?);
            Ok(AuthenticatedProjection {
                targets: vec![id],
                supersedes: vec![],
                payload: canonical,
                author_signature: Some(object.signature),
                author_nonce: Some(object.nonce),
            })
        }
        "delegation_revoked" => {
            let object: DelegationRevocation = serde_json::from_value(canonical.clone())?;
            Ok(AuthenticatedProjection {
                targets: vec![object.delegation_id.clone()],
                supersedes: vec![],
                payload: canonical,
                author_signature: Some(object.signature),
                author_nonce: Some(object.nonce),
            })
        }
        "lineage_key_rotated" => {
            let object: SignedKeyRotation = serde_json::from_value(canonical.clone())?;
            Ok(AuthenticatedProjection {
                targets: vec![object.statement.lineage_id.clone()],
                supersedes: vec![],
                payload: canonical,
                author_signature: Some(object.previous_signature),
                author_nonce: Some(object.statement.nonce),
            })
        }
        "acknowledgement" => {
            let object: SignedAcknowledgement = serde_json::from_value(canonical.clone())?;
            let lineage_id = row.lineage_id.clone().ok_or_else(|| {
                CommonwakeError::Unauthorized("acknowledgement has no lineage metadata".into())
            })?;
            Ok(AuthenticatedProjection {
                targets: vec![lineage_id],
                supersedes: vec![],
                payload: canonical,
                author_signature: Some(object.signature),
                author_nonce: Some(object.nonce),
            })
        }
        "observation" => {
            let payload = canonical.get("payload").cloned().ok_or_else(|| {
                CommonwakeError::Unauthorized("observation payload is missing".into())
            })?;
            let object: FederatedObservationEnvelope = serde_json::from_value(canonical.clone())?;
            Ok(AuthenticatedProjection {
                targets: vec![
                    object.payload.story_id.clone(),
                    object.payload.source_id.clone(),
                ],
                supersedes: vec![],
                payload,
                author_signature: None,
                author_nonce: None,
            })
        }
        "checkpoint_witnessed" => {
            let object: CheckpointWitness = serde_json::from_value(canonical.clone())?;
            Ok(AuthenticatedProjection {
                targets: vec![object.origin_node_id.clone()],
                supersedes: vec![],
                payload: canonical,
                author_signature: Some(object.signature),
                author_nonce: None,
            })
        }
        _ => {
            let object: SignedContribution = serde_json::from_value(canonical)?;
            if object.kind.as_str() != row.kind {
                return Err(CommonwakeError::Unauthorized(format!(
                    "event {} kind differs from its signed contribution",
                    row.sequence
                )));
            }
            Ok(AuthenticatedProjection {
                targets: object.targets,
                supersedes: object.supersedes,
                payload: object.payload,
                author_signature: Some(object.signature),
                author_nonce: Some(object.nonce),
            })
        }
    }
}

fn record_equivocation(transaction: &Transaction<'_>, input: &EquivocationInput<'_>) -> Result<()> {
    let evidence_id = prefixed_id(
        "cweq_",
        format!(
            "{}\0{}\0{}\0{}\0{}",
            input.origin_node_id,
            input.conflict_kind,
            input.cursor,
            input.existing_hash,
            input.incoming_hash,
        )
        .as_bytes(),
    );
    transaction.execute(
        "INSERT OR IGNORE INTO equivocation_evidence(
            evidence_id, origin_node_id, conflict_kind, cursor, existing_hash,
            incoming_hash, existing_json, incoming_json, detected_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            evidence_id,
            input.origin_node_id,
            input.conflict_kind,
            input.cursor,
            input.existing_hash,
            input.incoming_hash,
            input.existing_json,
            input.incoming_json,
            timestamp(Utc::now()),
        ],
    )?;
    Ok(())
}

fn validate_and_project_remote_event(
    transaction: &Transaction<'_>,
    origin_node_id: &str,
    event: &OriginEvent,
) -> Result<Option<String>> {
    match event.kind.as_str() {
        "lineage_registered" => {
            let registration: LineageRegistration =
                serde_json::from_value(event.canonical.clone())?;
            require_remote_protocol(&registration.protocol)?;
            require_remote_event_metadata(event, None, None, registration.created_at)?;
            validate_display_name(&registration.display_name)?;
            require_nonce(&registration.nonce)?;
            validate_authored_time_at(registration.created_at, remote_received_at(event)?)?;
            let key = verifying_key_from_b64(&registration.public_key)?;
            verify_object(&key, LINEAGE_DOMAIN, &registration, &registration.signature)?;
            let registered_lineage_id = lineage_id(&key);
            if transaction
                .query_row(
                    "SELECT 1 FROM federated_lineage_keys
                     WHERE origin_node_id = ?1 AND public_key = ?2",
                    params![origin_node_id, registration.public_key],
                    |_| Ok(()),
                )
                .optional()?
                .is_some()
            {
                return Err(CommonwakeError::Conflict(
                    "remote lineage public key is reused".into(),
                ));
            }
            transaction.execute(
                "INSERT INTO federated_lineages(
                    origin_node_id, lineage_id, display_name, current_public_key,
                    created_at, registered_sequence, key_version
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)",
                params![
                    origin_node_id,
                    registered_lineage_id,
                    registration.display_name,
                    registration.public_key,
                    timestamp(registration.created_at),
                    event.sequence,
                ],
            )?;
            transaction.execute(
                "INSERT INTO federated_lineage_keys(
                    origin_node_id, lineage_id, key_version, public_key, valid_from_sequence
                 ) VALUES (?1, ?2, 0, ?3, ?4)",
                params![
                    origin_node_id,
                    registered_lineage_id,
                    registration.public_key,
                    event.sequence,
                ],
            )?;
            Ok(Some(registration.nonce))
        }
        "delegation_registered" => {
            let delegation: SessionDelegation = serde_json::from_value(event.canonical.clone())?;
            require_remote_protocol(&delegation.protocol)?;
            require_remote_event_metadata(
                event,
                Some(&delegation.lineage_id),
                None,
                delegation.not_before,
            )?;
            validate_delegation_at(&delegation, remote_received_at(event)?)?;
            let current_key: String = transaction
                .query_row(
                    "SELECT current_public_key FROM federated_lineages
                     WHERE origin_node_id = ?1 AND lineage_id = ?2",
                    params![origin_node_id, delegation.lineage_id],
                    |row| row.get(0),
                )
                .optional()?
                .ok_or_else(|| {
                    CommonwakeError::Unauthorized(
                        "remote delegation refers to an unknown lineage".into(),
                    )
                })?;
            let key = verifying_key_from_b64(&current_key)?;
            verify_object(&key, DELEGATION_DOMAIN, &delegation, &delegation.signature)?;
            let delegation_id = prefixed_id("cwdel_", &canonical_without_signature(&delegation)?);
            transaction.execute(
                "INSERT INTO federated_delegations(
                    origin_node_id, delegation_id, lineage_id, session_public_key,
                    scopes_json, not_before, expires_at, registered_sequence
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    origin_node_id,
                    delegation_id,
                    delegation.lineage_id,
                    delegation.session_public_key,
                    serde_json::to_string(&delegation.scopes)?,
                    timestamp(delegation.not_before),
                    timestamp(delegation.expires_at),
                    event.sequence,
                ],
            )?;
            Ok(Some(delegation.nonce))
        }
        "delegation_revoked" => {
            let revocation: DelegationRevocation = serde_json::from_value(event.canonical.clone())?;
            require_remote_protocol(&revocation.protocol)?;
            require_remote_event_metadata(
                event,
                Some(&revocation.lineage_id),
                None,
                revocation.created_at,
            )?;
            validate_authority_change_at(
                &revocation.nonce,
                &revocation.reason,
                revocation.created_at,
                remote_received_at(event)?,
            )?;
            let current_key: String = transaction
                .query_row(
                    "SELECT current_public_key FROM federated_lineages
                     WHERE origin_node_id = ?1 AND lineage_id = ?2",
                    params![origin_node_id, revocation.lineage_id],
                    |row| row.get(0),
                )
                .optional()?
                .ok_or_else(|| {
                    CommonwakeError::Unauthorized(
                        "remote revocation refers to an unknown lineage".into(),
                    )
                })?;
            verify_object(
                &verifying_key_from_b64(&current_key)?,
                REVOCATION_DOMAIN,
                &revocation,
                &revocation.signature,
            )?;
            let changed = transaction.execute(
                "UPDATE federated_delegations SET revoked_sequence = ?4
                 WHERE origin_node_id = ?1 AND delegation_id = ?2 AND lineage_id = ?3
                   AND revoked_sequence IS NULL",
                params![
                    origin_node_id,
                    revocation.delegation_id,
                    revocation.lineage_id,
                    event.sequence,
                ],
            )?;
            if changed != 1 {
                return Err(CommonwakeError::Unauthorized(
                    "remote revocation does not name one active delegation owned by its lineage"
                        .into(),
                ));
            }
            Ok(Some(revocation.nonce))
        }
        "lineage_key_rotated" => {
            let rotation: SignedKeyRotation = serde_json::from_value(event.canonical.clone())?;
            let statement = &rotation.statement;
            require_remote_protocol(&statement.protocol)?;
            require_remote_event_metadata(
                event,
                Some(&statement.lineage_id),
                None,
                statement.created_at,
            )?;
            validate_authority_change_at(
                &statement.nonce,
                &statement.reason,
                statement.created_at,
                remote_received_at(event)?,
            )?;
            let (current_key, current_version): (String, i64) = transaction
                .query_row(
                    "SELECT current_public_key, key_version FROM federated_lineages
                     WHERE origin_node_id = ?1 AND lineage_id = ?2",
                    params![origin_node_id, statement.lineage_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?
                .ok_or_else(|| {
                    CommonwakeError::Unauthorized(
                        "remote rotation refers to an unknown lineage".into(),
                    )
                })?;
            if current_key != statement.previous_public_key
                || statement.previous_public_key == statement.new_public_key
            {
                return Err(CommonwakeError::Unauthorized(
                    "remote rotation does not advance the current lineage key".into(),
                ));
            }
            let previous_key = verifying_key_from_b64(&statement.previous_public_key)?;
            let new_key = verifying_key_from_b64(&statement.new_public_key)?;
            verify_object(
                &previous_key,
                KEY_ROTATION_DOMAIN,
                statement,
                &rotation.previous_signature,
            )?;
            verify_object(
                &new_key,
                KEY_ROTATION_DOMAIN,
                statement,
                &rotation.new_signature,
            )?;
            if transaction
                .query_row(
                    "SELECT 1 FROM federated_lineage_keys
                     WHERE origin_node_id = ?1 AND public_key = ?2",
                    params![origin_node_id, statement.new_public_key],
                    |_| Ok(()),
                )
                .optional()?
                .is_some()
            {
                return Err(CommonwakeError::Conflict(
                    "remote rotation reuses a historical lineage key".into(),
                ));
            }
            transaction.execute(
                "UPDATE federated_lineage_keys SET valid_to_sequence = ?3
                 WHERE origin_node_id = ?1 AND lineage_id = ?2 AND valid_to_sequence IS NULL",
                params![origin_node_id, statement.lineage_id, event.sequence],
            )?;
            transaction.execute(
                "UPDATE federated_lineages SET current_public_key = ?3, key_version = ?4
                 WHERE origin_node_id = ?1 AND lineage_id = ?2",
                params![
                    origin_node_id,
                    statement.lineage_id,
                    statement.new_public_key,
                    current_version + 1,
                ],
            )?;
            transaction.execute(
                "INSERT INTO federated_lineage_keys(
                    origin_node_id, lineage_id, key_version, public_key, valid_from_sequence
                 ) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    origin_node_id,
                    statement.lineage_id,
                    current_version + 1,
                    statement.new_public_key,
                    event.sequence,
                ],
            )?;
            if statement.revoke_existing_delegations {
                transaction.execute(
                    "UPDATE federated_delegations SET revoked_sequence = ?3
                     WHERE origin_node_id = ?1 AND lineage_id = ?2
                       AND revoked_sequence IS NULL",
                    params![origin_node_id, statement.lineage_id, event.sequence],
                )?;
            }
            Ok(Some(statement.nonce.clone()))
        }
        "acknowledgement" => {
            let acknowledgement: SignedAcknowledgement =
                serde_json::from_value(event.canonical.clone())?;
            require_remote_protocol(&acknowledgement.protocol)?;
            require_nonce(&acknowledgement.nonce)?;
            validate_memory_provenance(&acknowledgement.memory_provenance.statement)?;
            let received_at = remote_received_at(event)?;
            validate_authored_time_at(acknowledgement.created_at, received_at)?;
            let (lineage_id, session_key) = authorize_remote_delegation(
                transaction,
                origin_node_id,
                &acknowledgement.delegation_id,
                Scope::Ack,
                acknowledgement.created_at,
                received_at,
            )?;
            require_remote_event_metadata(
                event,
                Some(&lineage_id),
                Some(&acknowledgement.delegation_id),
                acknowledgement.created_at,
            )?;
            validate_remote_ack_cursor(
                transaction,
                origin_node_id,
                &lineage_id,
                event.sequence,
                acknowledgement.cursor,
            )?;
            verify_object(
                &verifying_key_from_b64(&session_key)?,
                ACK_DOMAIN,
                &acknowledgement,
                &acknowledgement.signature,
            )?;
            Ok(Some(acknowledgement.nonce))
        }
        "observation" => {
            let observation: FederatedObservationEnvelope =
                serde_json::from_value(event.canonical.clone())?;
            require_remote_protocol(&observation.protocol)?;
            if observation.kind != "observation" {
                return Err(CommonwakeError::Validation(
                    "remote observation envelope has the wrong kind".into(),
                ));
            }
            require_remote_event_metadata(event, None, None, observation.payload.retrieved_at)?;
            validate_authored_time_at(
                observation.payload.retrieved_at,
                remote_received_at(event)?,
            )?;
            project_federated_observation(
                transaction,
                origin_node_id,
                event.sequence,
                &observation.payload,
            )?;
            Ok(None)
        }
        "checkpoint_witnessed" => {
            let witness: CheckpointWitness = serde_json::from_value(event.canonical.clone())?;
            require_remote_protocol(&witness.protocol)?;
            require_remote_event_metadata(event, None, None, witness.observed_at)?;
            crate::federation::verify_checkpoint(&witness.origin_checkpoint)?;
            if witness.origin_checkpoint.node_id != witness.origin_node_id
                || witness.origin_checkpoint.node_public_key != witness.origin_node_public_key
                || witness.origin_checkpoint.cursor != witness.cursor
                || witness.origin_checkpoint.event_hash != witness.event_hash
            {
                return Err(CommonwakeError::Unauthorized(
                    "remote checkpoint witness summary differs from its signed origin checkpoint"
                        .into(),
                ));
            }
            let witness_key = verifying_key_from_b64(&witness.witness_node_public_key)?;
            if prefixed_id("cwnode_", &witness_key.to_bytes()) != witness.witness_node_id
                || witness.witness_node_id != origin_node_id
            {
                return Err(CommonwakeError::Unauthorized(
                    "remote checkpoint witness misstates its witnessing node".into(),
                ));
            }
            verify_object(&witness_key, WITNESS_DOMAIN, &witness, &witness.signature)?;
            Ok(None)
        }
        _ => {
            let contribution: SignedContribution = serde_json::from_value(event.canonical.clone())?;
            require_remote_protocol(&contribution.protocol)?;
            require_nonce(&contribution.nonce)?;
            validate_lists(&contribution.targets, &contribution.supersedes)?;
            if event.kind != contribution.kind.as_str() {
                return Err(CommonwakeError::Unauthorized(
                    "remote contribution kind differs from its canonical signed object".into(),
                ));
            }
            let received_at = remote_received_at(event)?;
            validate_authored_time_at(contribution.created_at, received_at)?;
            let (lineage_id, session_key) = authorize_remote_delegation(
                transaction,
                origin_node_id,
                &contribution.delegation_id,
                contribution.kind.required_scope(),
                contribution.created_at,
                received_at,
            )?;
            require_remote_event_metadata(
                event,
                Some(&lineage_id),
                Some(&contribution.delegation_id),
                contribution.created_at,
            )?;
            verify_object(
                &verifying_key_from_b64(&session_key)?,
                CONTRIBUTION_DOMAIN,
                &contribution,
                &contribution.signature,
            )?;
            apply_federated_contribution(
                transaction,
                origin_node_id,
                &lineage_id,
                event,
                &contribution,
            )?;
            Ok(Some(contribution.nonce))
        }
    }
}

fn require_remote_protocol(protocol: &str) -> Result<()> {
    if protocol != crate::PROTOCOL_VERSION {
        return Err(CommonwakeError::Validation(format!(
            "remote object uses unsupported protocol {protocol}"
        )));
    }
    Ok(())
}

fn require_remote_event_metadata(
    event: &OriginEvent,
    lineage_id: Option<&str>,
    delegation_id: Option<&str>,
    created_at: DateTime<Utc>,
) -> Result<()> {
    if event.lineage_id.as_deref() != lineage_id
        || event.delegation_id.as_deref() != delegation_id
        || event.created_at != timestamp(created_at)
    {
        return Err(CommonwakeError::Unauthorized(format!(
            "remote event {} metadata differs from its canonical signed object",
            event.sequence
        )));
    }
    parse_timestamp(&event.received_at)?;
    Ok(())
}

fn remote_received_at(event: &OriginEvent) -> Result<DateTime<Utc>> {
    parse_timestamp(&event.received_at)
}

fn authorize_remote_delegation(
    transaction: &Transaction<'_>,
    origin_node_id: &str,
    delegation_id: &str,
    required_scope: Scope,
    created_at: DateTime<Utc>,
    received_at: DateTime<Utc>,
) -> Result<(String, String)> {
    let raw = transaction
        .query_row(
            "SELECT lineage_id, session_public_key, scopes_json, not_before,
                    expires_at, revoked_sequence
             FROM federated_delegations
             WHERE origin_node_id = ?1 AND delegation_id = ?2",
            params![origin_node_id, delegation_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| {
            CommonwakeError::Unauthorized(
                "remote contribution refers to an unknown delegation".into(),
            )
        })?;
    let scopes: Vec<Scope> = serde_json::from_str(&raw.2)?;
    let not_before = parse_timestamp(&raw.3)?;
    let expires_at = parse_timestamp(&raw.4)?;
    if raw.5.is_some()
        || created_at < not_before
        || created_at > expires_at
        || received_at > expires_at + chrono::Duration::minutes(MAX_CLOCK_SKEW_MINUTES)
        || !scopes.contains(&required_scope)
    {
        return Err(CommonwakeError::Unauthorized(
            "remote contribution is outside its delegation authority".into(),
        ));
    }
    Ok((raw.0, raw.1))
}

fn validate_remote_ack_cursor(
    transaction: &Transaction<'_>,
    origin_node_id: &str,
    lineage_id: &str,
    event_sequence: i64,
    cursor: i64,
) -> Result<()> {
    let current_head = event_sequence - 1;
    if cursor < 0 || cursor > current_head {
        return Err(CommonwakeError::Validation(format!(
            "remote acknowledgement cursor {cursor} is beyond its origin head {current_head}"
        )));
    }
    let previous = transaction
        .query_row(
            "SELECT origin_sequence, canonical_json FROM remote_events
             WHERE origin_node_id = ?1 AND lineage_id = ?2 AND kind = 'acknowledgement'
             ORDER BY origin_sequence DESC LIMIT 1",
            params![origin_node_id, lineage_id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    if let Some((sequence, canonical_json)) = previous {
        let previous_ack: SignedAcknowledgement = serde_json::from_str(&canonical_json)?;
        let previous_effective = if previous_ack.cursor == sequence - 1 {
            sequence
        } else {
            previous_ack.cursor
        };
        if cursor < previous_effective {
            return Err(CommonwakeError::Conflict(format!(
                "remote acknowledgement cursor cannot move backward from {previous_effective}"
            )));
        }
    }
    Ok(())
}

fn project_federated_observation(
    transaction: &Transaction<'_>,
    origin_node_id: &str,
    sequence: i64,
    observation: &FederatedObservationPayload,
) -> Result<()> {
    if transaction
        .query_row(
            "SELECT 1 FROM federated_sources
             WHERE origin_node_id = ?1 AND source_id = ?2
               AND status IN ('probation', 'active')",
            params![origin_node_id, observation.source_id],
            |_| Ok(()),
        )
        .optional()?
        .is_none()
    {
        return Err(CommonwakeError::Unauthorized(
            "remote observation does not follow an independently reviewed source".into(),
        ));
    }
    if observation.title.chars().count() > MAX_TITLE_CHARS
        || observation.summary.chars().count() > MAX_SUMMARY_CHARS
    {
        return Err(CommonwakeError::Validation(
            "remote observation exceeds collector text limits".into(),
        ));
    }
    let expected_document_hash = crate::crypto::sha256_hex(
        serde_jcs::to_vec(&json!({
            "canonical_url": observation.canonical_url,
            "title": observation.title,
            "summary": observation.summary,
            "published_at": observation.published_at,
            "raw_metadata": observation.raw_metadata,
        }))
        .map_err(|error| {
            CommonwakeError::Internal(format!("canonical remote feed item failed: {error}"))
        })?,
    );
    if observation.document_hash != expected_document_hash {
        return Err(CommonwakeError::Unauthorized(
            "remote observation document hash does not match its canonical metadata".into(),
        ));
    }
    let expected_observation_id = prefixed_id(
        "cwobs_",
        format!(
            "{}\0{}\0{}",
            observation.source_id, observation.canonical_url, observation.document_hash
        )
        .as_bytes(),
    );
    if observation.observation_id != expected_observation_id {
        return Err(CommonwakeError::Unauthorized(
            "remote observation id does not match its source, URL, and document hash".into(),
        ));
    }
    let existing_story: Option<String> = transaction
        .query_row(
            "SELECT so.story_id FROM federated_observations o
             JOIN federated_story_observations so
               ON so.origin_node_id = o.origin_node_id
              AND so.observation_id = o.observation_id
             WHERE o.origin_node_id = ?1 AND o.source_id = ?2 AND o.canonical_url = ?3
             ORDER BY o.created_sequence ASC LIMIT 1",
            params![
                origin_node_id,
                observation.source_id,
                observation.canonical_url
            ],
            |row| row.get(0),
        )
        .optional()?;
    let expected_story_id = existing_story
        .unwrap_or_else(|| prefixed_id("cwstory_", observation.observation_id.as_bytes()));
    if observation.story_id != expected_story_id {
        return Err(CommonwakeError::Unauthorized(
            "remote observation story id does not follow the origin clustering rule".into(),
        ));
    }
    transaction.execute(
        "INSERT OR IGNORE INTO federated_stories(
            origin_node_id, story_id, title, first_seen_at, created_sequence
         ) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            origin_node_id,
            observation.story_id,
            observation.title,
            timestamp(observation.retrieved_at),
            sequence,
        ],
    )?;
    transaction.execute(
        "INSERT INTO federated_observations(
            origin_node_id, observation_id, source_id, canonical_url, title,
            summary, published_at, retrieved_at, language, document_hash, created_sequence
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            origin_node_id,
            observation.observation_id,
            observation.source_id,
            observation.canonical_url,
            observation.title,
            observation.summary,
            observation.published_at.map(timestamp),
            timestamp(observation.retrieved_at),
            observation.language,
            observation.document_hash,
            sequence,
        ],
    )?;
    transaction.execute(
        "INSERT INTO federated_story_observations(
            origin_node_id, story_id, observation_id
         ) VALUES (?1, ?2, ?3)",
        params![
            origin_node_id,
            observation.story_id,
            observation.observation_id
        ],
    )?;
    link_federated_story_event(transaction, origin_node_id, &observation.story_id, sequence)?;
    Ok(())
}

fn apply_federated_contribution(
    transaction: &Transaction<'_>,
    origin_node_id: &str,
    lineage_id: &str,
    event: &OriginEvent,
    contribution: &SignedContribution,
) -> Result<()> {
    validate_contribution_content(contribution)?;
    use crate::model::ContributionKind;
    match contribution.kind {
        ContributionKind::SourceProposal => {
            let payload: SourceProposalPayload =
                serde_json::from_value(contribution.payload.clone())?;
            validate_source_proposal(&payload)?;
            let normalized_feed = normalize_http_url(&payload.feed_url)?;
            let source_id = prefixed_id("cwsrc_", normalized_feed.as_bytes());
            transaction.execute(
                "INSERT INTO federated_sources(
                    origin_node_id, source_id, name, feed_url, homepage_url, medium,
                    primary_regions_json, languages_json, ownership, perspective_notes,
                    status, proposer_lineage_id, proposal_event_id, created_sequence
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                           'proposed', ?11, ?12, ?13)",
                params![
                    origin_node_id,
                    source_id,
                    payload.name,
                    normalized_feed,
                    payload
                        .homepage_url
                        .map(|url| normalize_http_url(&url))
                        .transpose()?,
                    payload.medium,
                    serde_json::to_string(&payload.primary_regions)?,
                    serde_json::to_string(&payload.languages)?,
                    payload.ownership,
                    payload.perspective_notes,
                    lineage_id,
                    event.event_id,
                    event.sequence,
                ],
            )?;
        }
        ContributionKind::SourceReview => {
            let payload: SourceReviewPayload =
                serde_json::from_value(contribution.payload.clone())?;
            let proposer: String = transaction
                .query_row(
                    "SELECT proposer_lineage_id FROM federated_sources
                     WHERE origin_node_id = ?1 AND source_id = ?2",
                    params![origin_node_id, payload.source_id],
                    |row| row.get(0),
                )
                .optional()?
                .ok_or_else(|| {
                    CommonwakeError::Unauthorized(
                        "remote review refers to an unknown source".into(),
                    )
                })?;
            if proposer == lineage_id || payload.evidence.is_empty() {
                return Err(CommonwakeError::Unauthorized(
                    "remote source review is not independent and evidence-bearing".into(),
                ));
            }
            transaction.execute(
                "INSERT INTO federated_source_reviews(
                    origin_node_id, source_id, reviewer_lineage_id, event_id,
                    recommendation, evidence_json, notes, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(origin_node_id, source_id, reviewer_lineage_id) DO UPDATE SET
                    event_id = excluded.event_id,
                    recommendation = excluded.recommendation,
                    evidence_json = excluded.evidence_json,
                    notes = excluded.notes,
                    created_at = excluded.created_at",
                params![
                    origin_node_id,
                    payload.source_id,
                    lineage_id,
                    event.event_id,
                    payload.recommendation.as_str(),
                    serde_json::to_string(&payload.evidence)?,
                    payload.notes,
                    timestamp(contribution.created_at),
                ],
            )?;
            let approvals: i64 = transaction.query_row(
                "SELECT COUNT(*) FROM federated_source_reviews
                 WHERE origin_node_id = ?1 AND source_id = ?2 AND recommendation = 'approve'",
                params![origin_node_id, payload.source_id],
                |row| row.get(0),
            )?;
            if approvals >= 2 {
                transaction.execute(
                    "UPDATE federated_sources SET status = 'probation'
                     WHERE origin_node_id = ?1 AND source_id = ?2 AND status = 'proposed'",
                    params![origin_node_id, payload.source_id],
                )?;
            }
        }
        ContributionKind::ObservationVerification => {
            let payload: ObservationVerificationPayload =
                serde_json::from_value(contribution.payload.clone())?;
            if transaction
                .query_row(
                    "SELECT 1 FROM federated_observations
                     WHERE origin_node_id = ?1 AND observation_id = ?2",
                    params![origin_node_id, payload.observation_id],
                    |_| Ok(()),
                )
                .optional()?
                .is_none()
            {
                return Err(CommonwakeError::Unauthorized(
                    "remote verification refers to an unknown observation".into(),
                ));
            }
            transaction.execute(
                "INSERT INTO federated_verifications(
                    origin_node_id, observation_id, verifier_lineage_id, event_id,
                    outcome, notes, evidence_json, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(origin_node_id, observation_id, verifier_lineage_id) DO UPDATE SET
                    event_id = excluded.event_id,
                    outcome = excluded.outcome,
                    notes = excluded.notes,
                    evidence_json = excluded.evidence_json,
                    created_at = excluded.created_at",
                params![
                    origin_node_id,
                    payload.observation_id,
                    lineage_id,
                    event.event_id,
                    payload.outcome.as_str(),
                    payload.notes,
                    serde_json::to_string(&payload.evidence)?,
                    timestamp(contribution.created_at),
                ],
            )?;
            let story_id: String = transaction.query_row(
                "SELECT story_id FROM federated_story_observations
                 WHERE origin_node_id = ?1 AND observation_id = ?2",
                params![origin_node_id, payload.observation_id],
                |row| row.get(0),
            )?;
            link_federated_story_event(transaction, origin_node_id, &story_id, event.sequence)?;
        }
        ContributionKind::StoryLink => {
            let payload: StoryLinkPayload = serde_json::from_value(contribution.payload.clone())?;
            if payload.observation_ids.is_empty() {
                return Err(CommonwakeError::Validation(
                    "remote story link must name at least one observation".into(),
                ));
            }
            if transaction
                .query_row(
                    "SELECT 1 FROM federated_stories
                     WHERE origin_node_id = ?1 AND story_id = ?2 AND merged_into IS NULL",
                    params![origin_node_id, payload.story_id],
                    |_| Ok(()),
                )
                .optional()?
                .is_none()
            {
                return Err(CommonwakeError::Unauthorized(
                    "remote story link refers to an unknown story".into(),
                ));
            }
            for observation_id in &payload.observation_ids {
                let old_story: String = transaction
                    .query_row(
                        "SELECT story_id FROM federated_story_observations
                         WHERE origin_node_id = ?1 AND observation_id = ?2",
                        params![origin_node_id, observation_id],
                        |row| row.get(0),
                    )
                    .optional()?
                    .ok_or_else(|| {
                        CommonwakeError::Unauthorized(
                            "remote story link refers to an unknown observation".into(),
                        )
                    })?;
                if old_story != payload.story_id {
                    transaction.execute(
                        "UPDATE federated_story_observations
                         SET story_id = ?3, linked_event_id = ?4
                         WHERE origin_node_id = ?1 AND observation_id = ?2",
                        params![
                            origin_node_id,
                            observation_id,
                            payload.story_id,
                            event.event_id
                        ],
                    )?;
                    let remaining: i64 = transaction.query_row(
                        "SELECT COUNT(*) FROM federated_story_observations
                         WHERE origin_node_id = ?1 AND story_id = ?2",
                        params![origin_node_id, old_story],
                        |row| row.get(0),
                    )?;
                    if remaining == 0 {
                        transaction.execute(
                            "UPDATE federated_stories SET merged_into = ?3
                             WHERE origin_node_id = ?1 AND story_id = ?2",
                            params![origin_node_id, old_story, payload.story_id],
                        )?;
                    }
                }
            }
            link_federated_story_event(
                transaction,
                origin_node_id,
                &payload.story_id,
                event.sequence,
            )?;
        }
        ContributionKind::Assessment => {
            let payload: AssessmentPayload = serde_json::from_value(contribution.payload.clone())?;
            if payload.evidence.is_empty() {
                return Err(CommonwakeError::Unauthorized(
                    "remote assessment has no evidence".into(),
                ));
            }
            if transaction
                .query_row(
                    "SELECT 1 FROM federated_stories
                     WHERE origin_node_id = ?1 AND story_id = ?2 AND merged_into IS NULL",
                    params![origin_node_id, payload.story_id],
                    |_| Ok(()),
                )
                .optional()?
                .is_none()
            {
                return Err(CommonwakeError::Unauthorized(
                    "remote assessment refers to an unknown story".into(),
                ));
            }
            transaction.execute(
                "INSERT INTO federated_assessments(
                    origin_node_id, story_id, assessor_lineage_id, event_id, summary,
                    significance, confidence, perspective, claims_json, evidence_json, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                 ON CONFLICT(origin_node_id, story_id, assessor_lineage_id) DO UPDATE SET
                    event_id = excluded.event_id,
                    summary = excluded.summary,
                    significance = excluded.significance,
                    confidence = excluded.confidence,
                    perspective = excluded.perspective,
                    claims_json = excluded.claims_json,
                    evidence_json = excluded.evidence_json,
                    created_at = excluded.created_at",
                params![
                    origin_node_id,
                    payload.story_id,
                    lineage_id,
                    event.event_id,
                    payload.summary,
                    payload.significance,
                    payload.confidence,
                    payload.perspective,
                    serde_json::to_string(&payload.claims)?,
                    serde_json::to_string(&payload.evidence)?,
                    timestamp(contribution.created_at),
                ],
            )?;
            link_federated_story_event(
                transaction,
                origin_node_id,
                &payload.story_id,
                event.sequence,
            )?;
        }
        ContributionKind::Correction => {
            let payload: CorrectionPayload = serde_json::from_value(contribution.payload.clone())?;
            if payload.correction.trim().len() < 10 || payload.reason.trim().len() < 10 {
                return Err(CommonwakeError::Validation(
                    "remote correction and reason must each be substantive".into(),
                ));
            }
            if payload.evidence.is_empty()
                || !contribution
                    .supersedes
                    .iter()
                    .any(|event_id| event_id == &payload.subject_event_id)
            {
                return Err(CommonwakeError::Validation(
                    "remote correction must carry evidence and supersede its subject event".into(),
                ));
            }
            if transaction
                .query_row(
                    "SELECT 1 FROM remote_events
                     WHERE origin_node_id = ?1 AND event_id = ?2",
                    params![origin_node_id, payload.subject_event_id],
                    |_| Ok(()),
                )
                .optional()?
                .is_none()
            {
                return Err(CommonwakeError::Unauthorized(
                    "remote correction refers to an unknown origin event".into(),
                ));
            }
            for story_id in &contribution.targets {
                link_federated_story_event(transaction, origin_node_id, story_id, event.sequence)?;
            }
        }
        ContributionKind::PerspectiveGap | ContributionKind::Translation => {
            for story_id in &contribution.targets {
                link_federated_story_event(transaction, origin_node_id, story_id, event.sequence)?;
            }
        }
        ContributionKind::WorkClaim => {
            let payload: WorkClaimPayload = serde_json::from_value(contribution.payload.clone())?;
            if !(1..=240).contains(&payload.lease_minutes) {
                return Err(CommonwakeError::Validation(
                    "remote work claim lease must be between 1 and 240 minutes".into(),
                ));
            }
        }
        ContributionKind::WorkResult => {
            let payload: WorkResultPayload = serde_json::from_value(contribution.payload.clone())?;
            if payload.summary.trim().len() < 10 || payload.evidence.is_empty() {
                return Err(CommonwakeError::Validation(
                    "remote work result requires a substantive summary and evidence".into(),
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

fn link_federated_story_event(
    transaction: &Transaction<'_>,
    origin_node_id: &str,
    story_id: &str,
    sequence: i64,
) -> Result<()> {
    if transaction
        .query_row(
            "SELECT 1 FROM federated_stories
             WHERE origin_node_id = ?1 AND story_id = ?2 AND merged_into IS NULL",
            params![origin_node_id, story_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some()
    {
        transaction.execute(
            "INSERT OR IGNORE INTO federated_story_events(
                origin_node_id, story_id, origin_sequence
             ) VALUES (?1, ?2, ?3)",
            params![origin_node_id, story_id, sequence],
        )?;
    }
    Ok(())
}

fn validate_projection(
    transaction: &Transaction<'_>,
    lineage_id: &str,
    contribution: &SignedContribution,
) -> Result<()> {
    validate_contribution_content(contribution)?;
    use crate::model::ContributionKind;
    match contribution.kind {
        ContributionKind::SourceProposal => {
            let payload: SourceProposalPayload =
                serde_json::from_value(contribution.payload.clone())?;
            validate_source_proposal(&payload)?;
        }
        ContributionKind::SourceReview => {
            let payload: SourceReviewPayload =
                serde_json::from_value(contribution.payload.clone())?;
            let proposer: String = transaction
                .query_row(
                    "SELECT proposer_lineage_id FROM sources WHERE source_id = ?1",
                    [&payload.source_id],
                    |row| row.get(0),
                )
                .optional()?
                .ok_or_else(|| {
                    CommonwakeError::NotFound("reviewed source does not exist".into())
                })?;
            if proposer == lineage_id {
                return Err(CommonwakeError::Validation(
                    "a source proposer cannot supply an independent review".into(),
                ));
            }
            if payload.evidence.is_empty() {
                return Err(CommonwakeError::Validation(
                    "source reviews require at least one evidence reference".into(),
                ));
            }
        }
        ContributionKind::ObservationVerification => {
            let payload: ObservationVerificationPayload =
                serde_json::from_value(contribution.payload.clone())?;
            ensure_exists(
                transaction,
                "observations",
                "observation_id",
                &payload.observation_id,
                "observation",
            )?;
        }
        ContributionKind::StoryLink => {
            let payload: StoryLinkPayload = serde_json::from_value(contribution.payload.clone())?;
            ensure_exists(
                transaction,
                "stories",
                "story_id",
                &payload.story_id,
                "story",
            )?;
            if payload.observation_ids.is_empty() {
                return Err(CommonwakeError::Validation(
                    "story link must name at least one observation".into(),
                ));
            }
            for observation_id in &payload.observation_ids {
                ensure_exists(
                    transaction,
                    "observations",
                    "observation_id",
                    observation_id,
                    "observation",
                )?;
            }
        }
        ContributionKind::Assessment => {
            let payload: AssessmentPayload = serde_json::from_value(contribution.payload.clone())?;
            ensure_exists(
                transaction,
                "stories",
                "story_id",
                &payload.story_id,
                "story",
            )?;
            if payload.evidence.is_empty() {
                return Err(CommonwakeError::Validation(
                    "assessments require at least one evidence reference".into(),
                ));
            }
        }
        ContributionKind::Correction => {
            let payload: CorrectionPayload = serde_json::from_value(contribution.payload.clone())?;
            if transaction
                .query_row(
                    "SELECT 1 FROM events WHERE event_id = ?1",
                    [&payload.subject_event_id],
                    |_| Ok(()),
                )
                .optional()?
                .is_none()
            {
                return Err(CommonwakeError::NotFound(format!(
                    "corrected event {}",
                    payload.subject_event_id
                )));
            }
            if payload.correction.trim().len() < 10 || payload.reason.trim().len() < 10 {
                return Err(CommonwakeError::Validation(
                    "correction and reason must each be substantive".into(),
                ));
            }
            if payload.evidence.is_empty() {
                return Err(CommonwakeError::Validation(
                    "correction requires at least one evidence reference".into(),
                ));
            }
            if !contribution
                .supersedes
                .iter()
                .any(|event_id| event_id == &payload.subject_event_id)
            {
                return Err(CommonwakeError::Validation(
                    "correction must name subject_event_id in supersedes".into(),
                ));
            }
        }
        ContributionKind::WorkClaim => {
            let payload: WorkClaimPayload = serde_json::from_value(contribution.payload.clone())?;
            ensure_open_work(transaction, &payload.work_id)?;
            if !(1..=240).contains(&payload.lease_minutes) {
                return Err(CommonwakeError::Validation(
                    "work claim lease must be between 1 and 240 minutes".into(),
                ));
            }
        }
        ContributionKind::WorkResult => {
            let payload: WorkResultPayload = serde_json::from_value(contribution.payload.clone())?;
            ensure_open_work(transaction, &payload.work_id)?;
            if payload.summary.trim().len() < 10 {
                return Err(CommonwakeError::Validation(
                    "work result requires a substantive summary".into(),
                ));
            }
            if payload.evidence.is_empty() {
                return Err(CommonwakeError::Validation(
                    "work result requires at least one evidence reference".into(),
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

fn apply_projection(
    transaction: &Transaction<'_>,
    lineage_id: &str,
    contribution: &SignedContribution,
    accepted: &AcceptedObject,
) -> Result<()> {
    use crate::model::ContributionKind;
    match contribution.kind {
        ContributionKind::SourceProposal => {
            let payload: SourceProposalPayload =
                serde_json::from_value(contribution.payload.clone())?;
            let normalized_feed = normalize_http_url(&payload.feed_url)?;
            let source_id = prefixed_id("cwsrc_", normalized_feed.as_bytes());
            transaction.execute(
                "INSERT INTO sources(
                    source_id, name, feed_url, homepage_url, medium, primary_regions_json,
                    languages_json, ownership, perspective_notes, status,
                    proposer_lineage_id, proposal_event_id, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'proposed', ?10, ?11, ?12)",
                params![
                    source_id,
                    payload.name,
                    normalized_feed,
                    payload
                        .homepage_url
                        .map(|url| normalize_http_url(&url))
                        .transpose()?,
                    payload.medium,
                    serde_json::to_string(&payload.primary_regions)?,
                    serde_json::to_string(&payload.languages)?,
                    payload.ownership,
                    payload.perspective_notes,
                    lineage_id,
                    accepted.id,
                    timestamp(contribution.created_at),
                ],
            )?;
            insert_work_item(
                transaction,
                "review_source",
                "source",
                &source_id,
                "Check provenance, ownership, terms, security, duplication, and coverage value with evidence.",
                2,
                accepted.sequence,
            )?;
        }
        ContributionKind::SourceReview => {
            let payload: SourceReviewPayload =
                serde_json::from_value(contribution.payload.clone())?;
            transaction.execute(
                "INSERT INTO source_reviews(
                    source_id, reviewer_lineage_id, event_id, recommendation,
                    evidence_json, notes, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(source_id, reviewer_lineage_id) DO UPDATE SET
                    event_id = excluded.event_id,
                    recommendation = excluded.recommendation,
                    evidence_json = excluded.evidence_json,
                    notes = excluded.notes,
                    created_at = excluded.created_at",
                params![
                    payload.source_id,
                    lineage_id,
                    accepted.id,
                    payload.recommendation.as_str(),
                    serde_json::to_string(&payload.evidence)?,
                    payload.notes,
                    timestamp(contribution.created_at),
                ],
            )?;
            let (approvals, rejections): (i64, i64) = transaction.query_row(
                "SELECT
                    SUM(CASE WHEN recommendation = 'approve' THEN 1 ELSE 0 END),
                    SUM(CASE WHEN recommendation = 'reject' THEN 1 ELSE 0 END)
                 FROM source_reviews WHERE source_id = ?1",
                [&payload.source_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            if approvals >= 2 {
                transaction.execute(
                    "UPDATE sources SET status = 'probation' WHERE source_id = ?1 AND status = 'proposed'",
                    [&payload.source_id],
                )?;
            }
            if approvals >= 2 || rejections >= 2 {
                complete_work(
                    transaction,
                    "review_source",
                    &payload.source_id,
                    accepted.sequence,
                )?;
            }
        }
        ContributionKind::ObservationVerification => {
            let payload: ObservationVerificationPayload =
                serde_json::from_value(contribution.payload.clone())?;
            transaction.execute(
                "INSERT INTO observation_verifications(
                    observation_id, verifier_lineage_id, event_id, outcome,
                    notes, evidence_json, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(observation_id, verifier_lineage_id) DO UPDATE SET
                    event_id = excluded.event_id,
                    outcome = excluded.outcome,
                    notes = excluded.notes,
                    evidence_json = excluded.evidence_json,
                    created_at = excluded.created_at",
                params![
                    payload.observation_id,
                    lineage_id,
                    accepted.id,
                    payload.outcome.as_str(),
                    payload.notes,
                    serde_json::to_string(&payload.evidence)?,
                    timestamp(contribution.created_at),
                ],
            )?;
            let count: i64 = transaction.query_row(
                "SELECT COUNT(*) FROM observation_verifications WHERE observation_id = ?1",
                [&payload.observation_id],
                |row| row.get(0),
            )?;
            if count >= 2 {
                complete_work(
                    transaction,
                    "verify_observation",
                    &payload.observation_id,
                    accepted.sequence,
                )?;
            }
        }
        ContributionKind::StoryLink => {
            let payload: StoryLinkPayload = serde_json::from_value(contribution.payload.clone())?;
            for observation_id in &payload.observation_ids {
                let old_story: String = transaction.query_row(
                    "SELECT story_id FROM story_observations WHERE observation_id = ?1",
                    [observation_id],
                    |row| row.get(0),
                )?;
                if old_story != payload.story_id {
                    transaction.execute(
                        "UPDATE story_observations SET story_id = ?1, linked_event_id = ?2
                         WHERE observation_id = ?3",
                        params![payload.story_id, accepted.id, observation_id],
                    )?;
                    let remaining: i64 = transaction.query_row(
                        "SELECT COUNT(*) FROM story_observations WHERE story_id = ?1",
                        [&old_story],
                        |row| row.get(0),
                    )?;
                    if remaining == 0 {
                        transaction.execute(
                            "UPDATE stories SET merged_into = ?1 WHERE story_id = ?2",
                            params![&payload.story_id, &old_story],
                        )?;
                    }
                    let pair = story_pair_id(&payload.story_id, &old_story);
                    complete_work(transaction, "cluster_stories", &pair, accepted.sequence)?;
                }
            }
        }
        ContributionKind::Assessment => {
            let payload: AssessmentPayload = serde_json::from_value(contribution.payload.clone())?;
            transaction.execute(
                "INSERT INTO assessments(
                    story_id, assessor_lineage_id, event_id, summary, significance,
                    confidence, perspective, claims_json, evidence_json, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(story_id, assessor_lineage_id) DO UPDATE SET
                    event_id = excluded.event_id,
                    summary = excluded.summary,
                    significance = excluded.significance,
                    confidence = excluded.confidence,
                    perspective = excluded.perspective,
                    claims_json = excluded.claims_json,
                    evidence_json = excluded.evidence_json,
                    created_at = excluded.created_at",
                params![
                    payload.story_id,
                    lineage_id,
                    accepted.id,
                    payload.summary,
                    payload.significance,
                    payload.confidence,
                    payload.perspective,
                    serde_json::to_string(&payload.claims)?,
                    serde_json::to_string(&payload.evidence)?,
                    timestamp(contribution.created_at),
                ],
            )?;
            let count: i64 = transaction.query_row(
                "SELECT COUNT(*) FROM assessments WHERE story_id = ?1",
                [&payload.story_id],
                |row| row.get(0),
            )?;
            if count >= 2 {
                complete_work(
                    transaction,
                    "assess_story",
                    &payload.story_id,
                    accepted.sequence,
                )?;
            }
        }
        ContributionKind::WorkClaim => {
            let payload: WorkClaimPayload = serde_json::from_value(contribution.payload.clone())?;
            let expires_at = contribution.created_at
                + chrono::Duration::minutes(i64::from(payload.lease_minutes));
            transaction.execute(
                "INSERT INTO work_claims(work_id, lineage_id, event_id, claimed_at, expires_at, note)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(work_id, lineage_id) DO UPDATE SET
                    event_id = excluded.event_id,
                    claimed_at = excluded.claimed_at,
                    expires_at = excluded.expires_at,
                    note = excluded.note",
                params![
                    payload.work_id,
                    lineage_id,
                    accepted.id,
                    timestamp(contribution.created_at),
                    timestamp(expires_at),
                    payload.note,
                ],
            )?;
        }
        ContributionKind::WorkResult => {
            let payload: WorkResultPayload = serde_json::from_value(contribution.payload.clone())?;
            transaction.execute(
                "INSERT INTO work_results(
                    work_id, lineage_id, event_id, outcome, summary,
                    evidence_json, result_json, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(work_id, lineage_id) DO UPDATE SET
                    event_id = excluded.event_id,
                    outcome = excluded.outcome,
                    summary = excluded.summary,
                    evidence_json = excluded.evidence_json,
                    result_json = excluded.result_json,
                    created_at = excluded.created_at",
                params![
                    payload.work_id,
                    lineage_id,
                    accepted.id,
                    payload.outcome.as_str(),
                    payload.summary,
                    serde_json::to_string(&payload.evidence)?,
                    serde_json::to_string(&payload.result)?,
                    timestamp(contribution.created_at),
                ],
            )?;
            let (required, received): (i64, i64) = transaction.query_row(
                "SELECT w.required_results,
                        (SELECT COUNT(*) FROM work_results r
                         WHERE r.work_id = w.work_id AND r.outcome IN ('completed', 'no_match'))
                 FROM work_items w WHERE w.work_id = ?1",
                [&payload.work_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            if required > 0 && received >= required {
                transaction.execute(
                    "UPDATE work_items SET status = 'complete', completed_sequence = ?2
                     WHERE work_id = ?1 AND status = 'open'",
                    params![payload.work_id, accepted.sequence],
                )?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_source_proposal(payload: &SourceProposalPayload) -> Result<()> {
    if payload.name.trim().len() < 2 || payload.name.len() > 160 {
        return Err(CommonwakeError::Validation(
            "source name must contain 2 to 160 characters".into(),
        ));
    }
    normalize_http_url(&payload.feed_url)?;
    if let Some(homepage) = &payload.homepage_url {
        normalize_http_url(homepage)?;
    }
    if payload.medium.trim().len() < 2 || payload.medium.len() > 160 {
        return Err(CommonwakeError::Validation(
            "source medium must contain 2 to 160 characters".into(),
        ));
    }
    if payload.primary_regions.is_empty() || payload.primary_regions.len() > 32 {
        return Err(CommonwakeError::Validation(
            "source proposal must declare 1 to 32 regions or coverage tags".into(),
        ));
    }
    if payload.languages.is_empty() || payload.languages.len() > 32 {
        return Err(CommonwakeError::Validation(
            "source proposal must declare 1 to 32 languages".into(),
        ));
    }
    for (label, values) in [
        ("region or coverage tag", &payload.primary_regions),
        ("language", &payload.languages),
    ] {
        if values
            .iter()
            .any(|value| value.trim().is_empty() || value.len() > 80)
        {
            return Err(CommonwakeError::Validation(format!(
                "source {label} values must contain 1 to 80 characters"
            )));
        }
        let distinct: BTreeSet<String> = values.iter().map(|value| coverage_key(value)).collect();
        if distinct.len() != values.len() {
            return Err(CommonwakeError::Validation(format!(
                "source {label} values must be unique"
            )));
        }
    }
    if payload
        .ownership
        .as_ref()
        .is_some_and(|value| value.trim().is_empty() || value.len() > 200)
        || payload
            .perspective_notes
            .as_ref()
            .is_some_and(|value| value.trim().is_empty() || value.len() > 2_000)
    {
        return Err(CommonwakeError::Validation(
            "source ownership or perspective metadata exceeds its protocol bound".into(),
        ));
    }
    if payload.rationale.trim().len() < 20 || payload.rationale.len() > 4_000 {
        return Err(CommonwakeError::Validation(
            "source proposal needs an evidence-oriented rationale of 20 to 4000 characters".into(),
        ));
    }
    Ok(())
}

fn validate_contribution_content(contribution: &SignedContribution) -> Result<()> {
    use crate::model::ContributionKind;
    if !contribution.payload.is_object() {
        return Err(CommonwakeError::Validation(
            "contribution payload must be a JSON object".into(),
        ));
    }
    match contribution.kind {
        ContributionKind::SourceProposal => {
            let payload: SourceProposalPayload =
                serde_json::from_value(contribution.payload.clone())?;
            validate_source_proposal(&payload)?;
        }
        ContributionKind::SourceReview => {
            let payload: SourceReviewPayload =
                serde_json::from_value(contribution.payload.clone())?;
            validate_evidence_refs(&payload.evidence, true)?;
        }
        ContributionKind::ObservationVerification => {
            let payload: ObservationVerificationPayload =
                serde_json::from_value(contribution.payload.clone())?;
            validate_evidence_refs(&payload.evidence, true)?;
        }
        ContributionKind::StoryLink => {
            let payload: StoryLinkPayload = serde_json::from_value(contribution.payload.clone())?;
            if payload.observation_ids.is_empty() || payload.observation_ids.len() > 64 {
                return Err(CommonwakeError::Validation(
                    "story link must name 1 to 64 observations".into(),
                ));
            }
            let distinct: BTreeSet<&String> = payload.observation_ids.iter().collect();
            if distinct.len() != payload.observation_ids.len() {
                return Err(CommonwakeError::Validation(
                    "story link observation identifiers must be unique".into(),
                ));
            }
            if payload.rationale.trim().len() < 10 {
                return Err(CommonwakeError::Validation(
                    "story link requires a substantive rationale".into(),
                ));
            }
            validate_evidence_refs(&payload.evidence, false)?;
        }
        ContributionKind::Assessment => {
            let payload: AssessmentPayload = serde_json::from_value(contribution.payload.clone())?;
            if payload.summary.trim().len() < 10
                || payload.significance.trim().len() < 10
                || payload.confidence.trim().is_empty()
                || payload.perspective.trim().is_empty()
                || payload.claims.len() > 64
            {
                return Err(CommonwakeError::Validation(
                    "assessment requires substantive summary, significance, confidence, perspective, and at most 64 claims"
                        .into(),
                ));
            }
            validate_evidence_refs(&payload.evidence, true)?;
            for claim in &payload.claims {
                if claim.text.trim().is_empty() {
                    return Err(CommonwakeError::Validation(
                        "assessment claim text cannot be empty".into(),
                    ));
                }
                validate_evidence_refs(&claim.evidence, false)?;
            }
        }
        ContributionKind::Correction => {
            let payload: CorrectionPayload = serde_json::from_value(contribution.payload.clone())?;
            validate_evidence_refs(&payload.evidence, true)?;
        }
        ContributionKind::WorkClaim => {
            let payload: WorkClaimPayload = serde_json::from_value(contribution.payload.clone())?;
            if !(1..=240).contains(&payload.lease_minutes) {
                return Err(CommonwakeError::Validation(
                    "work claim lease must be between 1 and 240 minutes".into(),
                ));
            }
        }
        ContributionKind::WorkResult => {
            let payload: WorkResultPayload = serde_json::from_value(contribution.payload.clone())?;
            validate_evidence_refs(&payload.evidence, true)?;
        }
        _ => {}
    }
    Ok(())
}

fn validate_evidence_refs(evidence: &[EvidenceRef], required: bool) -> Result<()> {
    if required && evidence.is_empty() {
        return Err(CommonwakeError::Validation(
            "this contribution requires public evidence".into(),
        ));
    }
    if evidence.len() > 64 {
        return Err(CommonwakeError::Validation(
            "evidence lists are limited to 64 references".into(),
        ));
    }
    for reference in evidence {
        if reference.url.len() > 2_048 {
            return Err(CommonwakeError::Validation(
                "evidence URLs are limited to 2048 characters".into(),
            ));
        }
        normalize_http_url(&reference.url)?;
        if reference
            .title
            .as_ref()
            .is_some_and(|title| title.len() > 500)
            || reference
                .digest
                .as_ref()
                .is_some_and(|digest| digest.len() > 256)
        {
            return Err(CommonwakeError::Validation(
                "evidence title or digest exceeds its protocol bound".into(),
            ));
        }
    }
    Ok(())
}

fn normalize_http_url(value: &str) -> Result<String> {
    let mut url = Url::parse(value)
        .map_err(|_| CommonwakeError::Validation(format!("invalid URL: {value}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(CommonwakeError::Validation(
            "source URLs must use HTTP or HTTPS".into(),
        ));
    }
    if url.host_str().is_none() {
        return Err(CommonwakeError::Validation(
            "source URL must have a host".into(),
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(CommonwakeError::Validation(
            "public source URLs cannot contain credentials".into(),
        ));
    }
    url.set_fragment(None);
    Ok(url.to_string())
}

fn coverage_key(value: &str) -> String {
    value.trim().to_lowercase()
}

fn ensure_exists(
    transaction: &Transaction<'_>,
    table: &str,
    column: &str,
    id: &str,
    label: &str,
) -> Result<()> {
    let allowed = matches!(
        (table, column),
        ("observations", "observation_id") | ("stories", "story_id")
    );
    if !allowed {
        return Err(CommonwakeError::Internal(
            "unsafe internal existence query".into(),
        ));
    }
    let sql = format!("SELECT 1 FROM {table} WHERE {column} = ?1");
    if transaction
        .query_row(&sql, [id], |_| Ok(()))
        .optional()?
        .is_none()
    {
        return Err(CommonwakeError::NotFound(format!("{label} {id}")));
    }
    Ok(())
}

fn format_work_cursor(created_sequence: i64, work_id: &str) -> String {
    format!("{created_sequence}:{work_id}")
}

fn parse_work_cursor(cursor: &str) -> Result<(i64, String)> {
    if cursor.len() > 96 {
        return Err(CommonwakeError::Validation(
            "work cursor exceeds 96 characters".into(),
        ));
    }
    let (sequence, work_id) = cursor.split_once(':').ok_or_else(|| {
        CommonwakeError::Validation("work cursor must be sequence:work_id".into())
    })?;
    let sequence = sequence.parse::<i64>().map_err(|_| {
        CommonwakeError::Validation("work cursor sequence must be an integer".into())
    })?;
    if sequence < 0 {
        return Err(CommonwakeError::Validation(
            "work cursor sequence cannot be negative".into(),
        ));
    }
    let digest = work_id.strip_prefix("cwwork_").ok_or_else(|| {
        CommonwakeError::Validation("work cursor must contain a work identifier".into())
    })?;
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(CommonwakeError::Validation(
            "work cursor contains an invalid work identifier".into(),
        ));
    }
    Ok((sequence, work_id.to_owned()))
}

fn insert_work_item(
    transaction: &Transaction<'_>,
    kind: &str,
    subject_type: &str,
    subject_id: &str,
    instructions: &str,
    required_results: i64,
    created_sequence: i64,
) -> Result<()> {
    let work_id = prefixed_id("cwwork_", format!("{kind}\0{subject_id}").as_bytes());
    transaction.execute(
        "INSERT OR IGNORE INTO work_items(
            work_id, kind, subject_type, subject_id, instructions,
            required_results, status, created_sequence
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'open', ?7)",
        params![
            work_id,
            kind,
            subject_type,
            subject_id,
            instructions,
            required_results,
            created_sequence
        ],
    )?;
    Ok(())
}

fn seed_bootstrap_work(connection: &Connection) -> Result<()> {
    for (subject_id, focus) in BOOTSTRAP_SOURCE_COVERAGE {
        let kind = "discover_sources";
        let work_id = prefixed_id("cwwork_", format!("{kind}\0{subject_id}").as_bytes());
        let instructions = format!(
            "Discover accessible RSS or Atom feeds covering {focus}. Seek at least two candidates with distinct ownership or institutional origin; prefer original-language and primary material where useful. Submit each candidate as a source_proposal with region, language, ownership, and perspective notes. Candidates receive no trust until independent source reviews approve them."
        );
        connection.execute(
            "INSERT OR IGNORE INTO work_items(
                work_id, kind, subject_type, subject_id, instructions,
                required_results, status, created_sequence
             ) VALUES (?1, ?2, 'coverage_area', ?3, ?4, 0, 'open', 0)",
            params![work_id, kind, subject_id, instructions],
        )?;
    }
    Ok(())
}

fn refresh_replication_work(connection: &Connection) -> Result<()> {
    let node_id: Option<String> = connection
        .query_row("SELECT value FROM meta WHERE key = 'node_id'", [], |row| {
            row.get(0)
        })
        .optional()?;
    let Some(node_id) = node_id else {
        return Ok(());
    };
    let desired_replicas: i64 = connection
        .query_row(
            "SELECT value FROM meta WHERE key = 'desired_replicas'",
            [],
            |row| row.get::<_, String>(0),
        )?
        .parse()
        .map_err(|_| CommonwakeError::Internal("stored replica target is malformed".into()))?;
    let (cursor, event_hash) = connection
        .query_row(
            "SELECT sequence, event_hash FROM events ORDER BY sequence DESC LIMIT 1",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?
        .unwrap_or((0, ZERO_HASH.into()));
    let recent_after = timestamp(Utc::now() - Duration::hours(24));
    let confirmed: i64 = connection.query_row(
        "SELECT COUNT(DISTINCT relay_node_id) FROM publication_targets
         WHERE receipt_json IS NOT NULL
           AND acknowledged_cursor = ?1
           AND acknowledged_event_hash = ?2
           AND last_success_at >= ?3",
        params![cursor, event_hash, recent_after],
        |row| row.get(0),
    )?;
    let kind = "replicate_origin";
    let work_id = prefixed_id("cwwork_", format!("{kind}\0{node_id}").as_bytes());
    let instructions = format!(
        "Help preserve origin {node_id} through at least {desired_replicas} distinct independently operated relays. Operate or nominate a relay only through locally authorized publication configuration, then require a valid signed replication receipt for the current head. A URL is not a replica, two URLs backed by one relay identity count once, and node or lineage secret keys must never be shared. Participation is voluntary."
    );
    connection.execute(
        "INSERT OR IGNORE INTO work_items(
            work_id, kind, subject_type, subject_id, instructions,
            required_results, status, created_sequence
         ) VALUES (?1, ?2, 'origin_node', ?3, ?4, 0, 'open', ?5)",
        params![work_id, kind, node_id, instructions, cursor],
    )?;
    if confirmed >= desired_replicas {
        connection.execute(
            "UPDATE work_items SET
                instructions = ?2, status = 'complete', completed_sequence = ?3
             WHERE work_id = ?1",
            params![work_id, instructions, cursor],
        )?;
    } else {
        connection.execute(
            "UPDATE work_items SET
                instructions = ?2, status = 'open', created_sequence = ?3,
                completed_sequence = NULL
             WHERE work_id = ?1",
            params![work_id, instructions, cursor],
        )?;
    }
    Ok(())
}

fn insert_cluster_candidates(
    transaction: &Transaction<'_>,
    story_id: &str,
    title: &str,
    created_sequence: i64,
) -> Result<()> {
    let mut statement = transaction.prepare(
        "SELECT story_id, title FROM stories
         WHERE story_id != ?1 AND merged_into IS NULL
         ORDER BY created_sequence DESC LIMIT 50",
    )?;
    let rows = statement.query_map([story_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut candidates = Vec::new();
    for row in rows {
        candidates.push(row?);
    }
    drop(statement);

    for (candidate_id, candidate_title) in candidates {
        let similarity = title_similarity(title, &candidate_title);
        if similarity < 0.15 {
            continue;
        }
        let pair = story_pair_id(story_id, &candidate_id);
        let instructions = format!(
            "Determine whether these observations describe the same development. Link them only with evidence; otherwise submit a no_match result. Candidate similarity: {similarity:.2}. Titles: {title:?} / {candidate_title:?}"
        );
        insert_work_item(
            transaction,
            "cluster_stories",
            "story_pair",
            &pair,
            &instructions,
            1,
            created_sequence,
        )?;
    }
    Ok(())
}

fn story_pair_id(first: &str, second: &str) -> String {
    if first <= second {
        format!("{first}~{second}")
    } else {
        format!("{second}~{first}")
    }
}

fn title_similarity(first: &str, second: &str) -> f64 {
    let first = title_terms(first);
    let second = title_terms(second);
    if first.is_empty() || second.is_empty() {
        return 0.0;
    }
    let intersection = first.intersection(&second).count();
    let union = first.union(&second).count();
    intersection as f64 / union as f64
}

fn title_terms(title: &str) -> BTreeSet<String> {
    const STOP_WORDS: &[&str] = &[
        "about", "after", "from", "into", "that", "their", "the", "this", "with",
    ];
    title
        .split(|character: char| !character.is_alphanumeric())
        .map(str::to_ascii_lowercase)
        .filter(|term| term.len() >= 3 && !STOP_WORDS.contains(&term.as_str()))
        .collect()
}

fn ensure_open_work(transaction: &Transaction<'_>, work_id: &str) -> Result<()> {
    let status: Option<String> = transaction
        .query_row(
            "SELECT status FROM work_items WHERE work_id = ?1",
            [work_id],
            |row| row.get(0),
        )
        .optional()?;
    match status.as_deref() {
        Some("open") => Ok(()),
        Some(_) => Err(CommonwakeError::Conflict(
            "work item is no longer open".into(),
        )),
        None => Err(CommonwakeError::NotFound(format!("work item {work_id}"))),
    }
}

fn complete_work(
    transaction: &Transaction<'_>,
    kind: &str,
    subject_id: &str,
    completed_sequence: i64,
) -> Result<()> {
    transaction.execute(
        "UPDATE work_items SET status = 'complete', completed_sequence = ?3
         WHERE kind = ?1 AND subject_id = ?2 AND status = 'open'",
        params![kind, subject_id, completed_sequence],
    )?;
    Ok(())
}

fn work_result_count(connection: &Connection, kind: &str, subject_id: &str) -> Result<i64> {
    let (table, column) = match kind {
        "review_source" => ("source_reviews", "source_id"),
        "verify_observation" => ("observation_verifications", "observation_id"),
        "assess_story" => ("assessments", "story_id"),
        _ => {
            let work_id = prefixed_id("cwwork_", format!("{kind}\0{subject_id}").as_bytes());
            return Ok(connection.query_row(
                "SELECT COUNT(*) FROM work_results
                 WHERE work_id = ?1 AND outcome IN ('completed', 'no_match')",
                [&work_id],
                |row| row.get(0),
            )?);
        }
    };
    let sql = format!("SELECT COUNT(*) FROM {table} WHERE {column} = ?1");
    Ok(connection.query_row(&sql, [subject_id], |row| row.get(0))?)
}

fn event_views_between(
    connection: &Connection,
    after: i64,
    through: i64,
    limit: usize,
) -> Result<Vec<EventView>> {
    let mut statement = connection.prepare(
        "SELECT sequence, event_id, kind, lineage_id, delegation_id, created_at,
                received_at, targets_json, supersedes_json, payload_json,
                canonical_json, author_signature, previous_hash, event_hash, node_signature,
                author_nonce
         FROM events WHERE sequence > ?1 AND sequence <= ?2
         ORDER BY sequence ASC LIMIT ?3",
    )?;
    let rows = statement.query_map(params![after, through, limit as i64], raw_event_from_row)?;
    raw_rows_to_views(rows)
}

fn raw_event_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawEvent> {
    Ok(RawEvent {
        sequence: row.get(0)?,
        event_id: row.get(1)?,
        kind: row.get(2)?,
        lineage_id: row.get(3)?,
        delegation_id: row.get(4)?,
        created_at: row.get(5)?,
        received_at: row.get(6)?,
        targets_json: row.get(7)?,
        supersedes_json: row.get(8)?,
        payload_json: row.get(9)?,
        canonical_json: row.get(10)?,
        author_signature: row.get(11)?,
        previous_hash: row.get(12)?,
        event_hash: row.get(13)?,
        node_signature: row.get(14)?,
        author_nonce: row.get(15)?,
    })
}

fn raw_rows_to_views(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<RawEvent>>,
) -> Result<Vec<EventView>> {
    let mut events = Vec::new();
    for row in rows {
        events.push(event_view(row?)?);
    }
    Ok(events)
}

fn event_view(raw: RawEvent) -> Result<EventView> {
    let canonical: Value = serde_json::from_str(&raw.canonical_json)?;
    let projection = expected_projection(&raw, canonical.clone())?;
    Ok(EventView {
        sequence: raw.sequence,
        event_id: raw.event_id,
        kind: raw.kind,
        lineage_id: raw.lineage_id,
        delegation_id: raw.delegation_id,
        created_at: parse_timestamp(&raw.created_at)?,
        received_at: parse_timestamp(&raw.received_at)?,
        targets: projection.targets,
        supersedes: projection.supersedes,
        payload: projection.payload,
        canonical,
        author_signature: projection.author_signature,
        previous_hash: raw.previous_hash,
        event_hash: raw.event_hash,
        node_signature: raw.node_signature,
    })
}

fn origin_event(raw: RawEvent) -> Result<OriginEvent> {
    Ok(OriginEvent {
        sequence: raw.sequence,
        event_id: raw.event_id,
        kind: raw.kind,
        lineage_id: raw.lineage_id,
        delegation_id: raw.delegation_id,
        created_at: raw.created_at,
        received_at: raw.received_at,
        canonical: serde_json::from_str(&raw.canonical_json)?,
        previous_hash: raw.previous_hash,
        event_hash: raw.event_hash,
        node_signature: raw.node_signature,
    })
}

fn remote_origin_event_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<OriginEvent> {
    let canonical_json = row.get::<_, String>(7)?;
    let canonical = serde_json::from_str(&canonical_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(OriginEvent {
        sequence: row.get(0)?,
        event_id: row.get(1)?,
        kind: row.get(2)?,
        lineage_id: row.get(3)?,
        delegation_id: row.get(4)?,
        created_at: row.get(5)?,
        received_at: row.get(6)?,
        canonical,
        previous_hash: row.get(8)?,
        event_hash: row.get(9)?,
        node_signature: row.get(10)?,
    })
}

fn stories_changed_between_connection(
    connection: &Connection,
    after: i64,
    through: i64,
) -> Result<Vec<StoryView>> {
    let ids = changed_story_ids(connection, after, through)?;
    ids.into_iter()
        .map(|id| story_view(connection, &id))
        .collect()
}

fn changed_story_ids(connection: &Connection, after: i64, through: i64) -> Result<Vec<String>> {
    let mut statement = connection.prepare(
        "SELECT DISTINCT story_id FROM (
            SELECT s.story_id AS story_id, s.created_sequence AS seq
              FROM stories s WHERE s.merged_into IS NULL
            UNION ALL
            SELECT so.story_id, o.created_sequence
              FROM observations o JOIN story_observations so USING(observation_id)
            UNION ALL
            SELECT a.story_id, e.sequence
              FROM assessments a JOIN events e ON e.event_id = a.event_id
            UNION ALL
            SELECT so.story_id, e.sequence
              FROM observation_verifications v
              JOIN events e ON e.event_id = v.event_id
              JOIN story_observations so ON so.observation_id = v.observation_id
            UNION ALL
            SELECT s.story_id, e.sequence
              FROM events e
              JOIN json_each(e.targets_json) target
              JOIN stories s ON s.story_id = target.value
             WHERE e.kind IN ('story_link', 'correction', 'perspective_gap', 'translation')
         ) changed
         WHERE seq > ?1 AND seq <= ?2 ORDER BY story_id",
    )?;
    let rows = statement.query_map(params![after, through], |row| row.get::<_, String>(0))?;
    let mut ids = Vec::new();
    for row in rows {
        ids.push(row?);
    }
    Ok(ids)
}

fn federated_stories_changed_between_connection(
    connection: &Connection,
    after: i64,
    through: i64,
) -> Result<Vec<FederatedStoryView>> {
    federated_changed_story_ids(connection, after, through)?
        .into_iter()
        .map(|(origin_node_id, story_id)| {
            federated_story_view(connection, &origin_node_id, &story_id)
        })
        .collect()
}

fn federated_changed_story_ids(
    connection: &Connection,
    after: i64,
    through: i64,
) -> Result<Vec<(String, String)>> {
    let mut statement = connection.prepare(
        "SELECT DISTINCT se.origin_node_id, se.story_id
         FROM federation_imports i
         JOIN federated_story_events se
           ON se.origin_node_id = i.origin_node_id
          AND se.origin_sequence > i.remote_from_cursor
          AND se.origin_sequence <= i.remote_through_cursor
         JOIN federated_stories s
           ON s.origin_node_id = se.origin_node_id AND s.story_id = se.story_id
         WHERE i.local_witness_sequence > ?1 AND i.local_witness_sequence <= ?2
           AND s.merged_into IS NULL
         ORDER BY se.origin_node_id, se.story_id",
    )?;
    let rows = statement.query_map(params![after, through], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut ids = Vec::new();
    for row in rows {
        ids.push(row?);
    }
    Ok(ids)
}

fn story_view(connection: &Connection, story_id: &str) -> Result<StoryView> {
    let header = connection
        .query_row(
            "SELECT title, first_seen_at FROM stories
             WHERE story_id = ?1 AND merged_into IS NULL",
            [story_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?
        .ok_or_else(|| CommonwakeError::NotFound(format!("story {story_id}")))?;

    let mut observation_statement = connection.prepare(
        "SELECT o.observation_id, o.source_id, s.name, o.canonical_url, o.title,
                o.summary, o.published_at, o.retrieved_at, o.language, o.document_hash,
                SUM(CASE WHEN v.outcome = 'corroborated' THEN 1 ELSE 0 END),
                SUM(CASE WHEN v.outcome = 'disputed' THEN 1 ELSE 0 END)
         FROM story_observations so
         JOIN observations o ON o.observation_id = so.observation_id
         JOIN sources s ON s.source_id = o.source_id
         LEFT JOIN observation_verifications v ON v.observation_id = o.observation_id
         WHERE so.story_id = ?1 GROUP BY o.observation_id ORDER BY o.retrieved_at ASC",
    )?;
    let observation_rows = observation_statement.query_map([story_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, Option<String>>(6)?,
            row.get::<_, String>(7)?,
            row.get::<_, Option<String>>(8)?,
            row.get::<_, String>(9)?,
            row.get::<_, i64>(10)?,
            row.get::<_, i64>(11)?,
        ))
    })?;
    let mut observations = Vec::new();
    let mut verification_count = 0_i64;
    for row in observation_rows {
        let row = row?;
        verification_count += row.10 + row.11;
        observations.push(ObservationView {
            observation_id: row.0,
            source_id: row.1,
            source_name: row.2,
            canonical_url: row.3,
            title: row.4,
            summary: row.5,
            published_at: row.6.map(|value| parse_timestamp(&value)).transpose()?,
            retrieved_at: parse_timestamp(&row.7)?,
            language: row.8,
            document_hash: row.9,
            corroborated_count: row.10,
            disputed_count: row.11,
        });
    }

    let mut assessment_statement = connection.prepare(
        "SELECT a.assessor_lineage_id, l.display_name, a.event_id, a.summary,
                a.significance, a.confidence, a.perspective, a.claims_json,
                a.evidence_json, a.created_at
         FROM assessments a JOIN lineages l ON l.lineage_id = a.assessor_lineage_id
         WHERE a.story_id = ?1 ORDER BY a.created_at ASC",
    )?;
    let assessment_rows = assessment_statement.query_map([story_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, String>(6)?,
            row.get::<_, String>(7)?,
            row.get::<_, String>(8)?,
            row.get::<_, String>(9)?,
        ))
    })?;
    let mut assessments = Vec::new();
    for row in assessment_rows {
        let row = row?;
        assessments.push(AssessmentView {
            assessor_lineage_id: row.0,
            assessor_display_name: row.1,
            event_id: row.2,
            summary: row.3,
            significance: row.4,
            confidence: row.5,
            perspective: row.6,
            claims: serde_json::from_str::<Vec<Claim>>(&row.7)?,
            evidence: serde_json::from_str::<Vec<EvidenceRef>>(&row.8)?,
            created_at: parse_timestamp(&row.9)?,
        });
    }
    let distinct_sources = observations
        .iter()
        .map(|observation| observation.source_id.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    let stage = story_stage(distinct_sources, assessments.len(), verification_count);
    let related_events = related_story_events(connection, story_id)?;
    Ok(StoryView {
        story_id: story_id.into(),
        title: header.0,
        first_seen_at: parse_timestamp(&header.1)?,
        stage: stage.into(),
        observations,
        assessments,
        related_events,
    })
}

fn federated_story_view(
    connection: &Connection,
    origin_node_id: &str,
    story_id: &str,
) -> Result<FederatedStoryView> {
    let header = connection
        .query_row(
            "SELECT title, first_seen_at FROM federated_stories
             WHERE origin_node_id = ?1 AND story_id = ?2 AND merged_into IS NULL",
            params![origin_node_id, story_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?
        .ok_or_else(|| {
            CommonwakeError::NotFound(format!("federated story {origin_node_id}/{story_id}"))
        })?;

    let mut observation_statement = connection.prepare(
        "SELECT o.observation_id, o.source_id, s.name, o.canonical_url, o.title,
                o.summary, o.published_at, o.retrieved_at, o.language, o.document_hash,
                SUM(CASE WHEN v.outcome = 'corroborated' THEN 1 ELSE 0 END),
                SUM(CASE WHEN v.outcome = 'disputed' THEN 1 ELSE 0 END)
         FROM federated_story_observations so
         JOIN federated_observations o
           ON o.origin_node_id = so.origin_node_id AND o.observation_id = so.observation_id
         JOIN federated_sources s
           ON s.origin_node_id = o.origin_node_id AND s.source_id = o.source_id
         LEFT JOIN federated_verifications v
           ON v.origin_node_id = o.origin_node_id AND v.observation_id = o.observation_id
         WHERE so.origin_node_id = ?1 AND so.story_id = ?2
         GROUP BY o.origin_node_id, o.observation_id ORDER BY o.retrieved_at ASC",
    )?;
    let observation_rows =
        observation_statement.query_map(params![origin_node_id, story_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, i64>(10)?,
                row.get::<_, i64>(11)?,
            ))
        })?;
    let mut observations = Vec::new();
    let mut verification_count = 0_i64;
    for row in observation_rows {
        let row = row?;
        verification_count += row.10 + row.11;
        observations.push(ObservationView {
            observation_id: row.0,
            source_id: row.1,
            source_name: row.2,
            canonical_url: row.3,
            title: row.4,
            summary: row.5,
            published_at: row.6.map(|value| parse_timestamp(&value)).transpose()?,
            retrieved_at: parse_timestamp(&row.7)?,
            language: row.8,
            document_hash: row.9,
            corroborated_count: row.10,
            disputed_count: row.11,
        });
    }

    let mut assessment_statement = connection.prepare(
        "SELECT a.assessor_lineage_id, l.display_name, a.event_id, a.summary,
                a.significance, a.confidence, a.perspective, a.claims_json,
                a.evidence_json, a.created_at
         FROM federated_assessments a
         JOIN federated_lineages l
           ON l.origin_node_id = a.origin_node_id AND l.lineage_id = a.assessor_lineage_id
         WHERE a.origin_node_id = ?1 AND a.story_id = ?2 ORDER BY a.created_at ASC",
    )?;
    let assessment_rows =
        assessment_statement.query_map(params![origin_node_id, story_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, String>(9)?,
            ))
        })?;
    let mut assessments = Vec::new();
    for row in assessment_rows {
        let row = row?;
        assessments.push(AssessmentView {
            assessor_lineage_id: row.0,
            assessor_display_name: row.1,
            event_id: row.2,
            summary: row.3,
            significance: row.4,
            confidence: row.5,
            perspective: row.6,
            claims: serde_json::from_str::<Vec<Claim>>(&row.7)?,
            evidence: serde_json::from_str::<Vec<EvidenceRef>>(&row.8)?,
            created_at: parse_timestamp(&row.9)?,
        });
    }
    let distinct_sources = observations
        .iter()
        .map(|observation| observation.source_id.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    let stage = story_stage(distinct_sources, assessments.len(), verification_count);

    let mut related_statement = connection.prepare(
        "SELECT e.origin_sequence, e.event_id, e.kind, e.lineage_id, e.delegation_id,
                e.created_at, e.received_at, e.canonical_json, e.previous_hash,
                e.event_hash, e.node_signature
         FROM federated_story_events se
         JOIN remote_events e
           ON e.origin_node_id = se.origin_node_id
          AND e.origin_sequence = se.origin_sequence
         WHERE se.origin_node_id = ?1 AND se.story_id = ?2
         ORDER BY e.origin_sequence ASC",
    )?;
    let related_rows = related_statement.query_map(
        params![origin_node_id, story_id],
        remote_origin_event_from_row,
    )?;
    let mut related_events = Vec::new();
    for row in related_rows {
        related_events.push(row?);
    }

    Ok(FederatedStoryView {
        origin_node_id: origin_node_id.into(),
        story_id: story_id.into(),
        title: header.0,
        first_seen_at: parse_timestamp(&header.1)?,
        stage: stage.into(),
        observations,
        assessments,
        related_events,
    })
}

fn related_story_events(connection: &Connection, story_id: &str) -> Result<Vec<EventView>> {
    let mut statement = connection.prepare(
        "SELECT e.sequence, e.event_id, e.kind, e.lineage_id, e.delegation_id,
                e.created_at, e.received_at, e.targets_json, e.supersedes_json,
                e.payload_json, e.canonical_json, e.author_signature,
                e.previous_hash, e.event_hash, e.node_signature, e.author_nonce
         FROM events e
         WHERE e.kind IN ('story_link', 'correction', 'perspective_gap', 'translation')
           AND EXISTS (SELECT 1 FROM json_each(e.targets_json) WHERE value = ?1)
         ORDER BY e.sequence ASC",
    )?;
    let rows = statement.query_map([story_id], raw_event_from_row)?;
    raw_rows_to_views(rows)
}

fn story_stage(
    distinct_sources: usize,
    assessment_count: usize,
    verification_count: i64,
) -> &'static str {
    if distinct_sources >= 2 && assessment_count >= 2 && verification_count >= 2 {
        "brief"
    } else if assessment_count == 0 && verification_count == 0 {
        "raw"
    } else {
        "developing"
    }
}

fn canonical_json<T: Serialize>(value: &T) -> Result<String> {
    let bytes = serde_jcs::to_vec(value)
        .map_err(|error| CommonwakeError::Internal(format!("canonical JSON failed: {error}")))?;
    String::from_utf8(bytes).map_err(|error| {
        CommonwakeError::Internal(format!("canonical JSON was not UTF-8: {error}"))
    })
}

fn timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn parse_timestamp(value: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| CommonwakeError::Internal(format!("stored timestamp is invalid: {value}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_source_urls_reject_embedded_credentials() {
        assert!(normalize_http_url("https://example.org/feed.xml").is_ok());
        assert!(normalize_http_url("https://agent:secret@example.org/feed.xml").is_err());
    }

    #[test]
    fn repeated_observations_from_one_source_do_not_create_a_brief() {
        assert_eq!(story_stage(1, 2, 2), "developing");
        assert_eq!(story_stage(2, 2, 2), "brief");
    }
}
