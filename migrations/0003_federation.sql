BEGIN IMMEDIATE;

CREATE TABLE IF NOT EXISTS federation_peers (
    node_id TEXT PRIMARY KEY,
    node_public_key TEXT NOT NULL,
    first_seen_at TEXT NOT NULL,
    last_seen_at TEXT NOT NULL,
    cursor INTEGER NOT NULL DEFAULT 0,
    event_hash TEXT NOT NULL,
    checkpoint_json TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS remote_events (
    origin_node_id TEXT NOT NULL REFERENCES federation_peers(node_id),
    origin_sequence INTEGER NOT NULL,
    event_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    lineage_id TEXT,
    delegation_id TEXT,
    created_at TEXT NOT NULL,
    received_at TEXT NOT NULL,
    author_nonce TEXT,
    canonical_json TEXT NOT NULL,
    previous_hash TEXT NOT NULL,
    event_hash TEXT NOT NULL,
    node_signature TEXT NOT NULL,
    imported_at TEXT NOT NULL,
    PRIMARY KEY(origin_node_id, origin_sequence),
    UNIQUE(origin_node_id, event_id),
    UNIQUE(origin_node_id, event_hash),
    UNIQUE(origin_node_id, delegation_id, author_nonce)
);

CREATE INDEX IF NOT EXISTS idx_remote_events_kind
    ON remote_events(origin_node_id, kind, origin_sequence);

CREATE TABLE IF NOT EXISTS remote_checkpoints (
    origin_node_id TEXT NOT NULL REFERENCES federation_peers(node_id),
    cursor INTEGER NOT NULL,
    event_hash TEXT NOT NULL,
    created_at TEXT NOT NULL,
    signature TEXT NOT NULL,
    checkpoint_json TEXT NOT NULL,
    imported_at TEXT NOT NULL,
    PRIMARY KEY(origin_node_id, cursor, event_hash)
);

CREATE TABLE IF NOT EXISTS equivocation_evidence (
    evidence_id TEXT PRIMARY KEY,
    origin_node_id TEXT NOT NULL,
    conflict_kind TEXT NOT NULL,
    cursor INTEGER NOT NULL,
    existing_hash TEXT NOT NULL,
    incoming_hash TEXT NOT NULL,
    existing_json TEXT NOT NULL,
    incoming_json TEXT NOT NULL,
    detected_at TEXT NOT NULL,
    UNIQUE(origin_node_id, conflict_kind, cursor, existing_hash, incoming_hash)
);

CREATE TABLE IF NOT EXISTS checkpoint_witnesses (
    origin_node_id TEXT NOT NULL,
    cursor INTEGER NOT NULL,
    event_hash TEXT NOT NULL,
    witness_event_id TEXT NOT NULL UNIQUE REFERENCES events(event_id),
    witnessed_at TEXT NOT NULL,
    PRIMARY KEY(origin_node_id, cursor, event_hash)
);

CREATE TABLE IF NOT EXISTS federated_lineages (
    origin_node_id TEXT NOT NULL,
    lineage_id TEXT NOT NULL,
    display_name TEXT NOT NULL,
    current_public_key TEXT NOT NULL,
    created_at TEXT NOT NULL,
    registered_sequence INTEGER NOT NULL,
    key_version INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY(origin_node_id, lineage_id),
    UNIQUE(origin_node_id, current_public_key)
);

CREATE TABLE IF NOT EXISTS federated_lineage_keys (
    origin_node_id TEXT NOT NULL,
    lineage_id TEXT NOT NULL,
    key_version INTEGER NOT NULL,
    public_key TEXT NOT NULL,
    valid_from_sequence INTEGER NOT NULL,
    valid_to_sequence INTEGER,
    PRIMARY KEY(origin_node_id, lineage_id, key_version),
    UNIQUE(origin_node_id, public_key)
);

CREATE TABLE IF NOT EXISTS federated_delegations (
    origin_node_id TEXT NOT NULL,
    delegation_id TEXT NOT NULL,
    lineage_id TEXT NOT NULL,
    session_public_key TEXT NOT NULL,
    scopes_json TEXT NOT NULL,
    not_before TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    revoked_sequence INTEGER,
    registered_sequence INTEGER NOT NULL,
    PRIMARY KEY(origin_node_id, delegation_id)
);

CREATE TABLE IF NOT EXISTS federated_sources (
    origin_node_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    name TEXT NOT NULL,
    feed_url TEXT NOT NULL,
    homepage_url TEXT,
    medium TEXT NOT NULL,
    primary_regions_json TEXT NOT NULL,
    languages_json TEXT NOT NULL,
    ownership TEXT,
    perspective_notes TEXT,
    status TEXT NOT NULL,
    proposer_lineage_id TEXT NOT NULL,
    proposal_event_id TEXT NOT NULL,
    created_sequence INTEGER NOT NULL,
    PRIMARY KEY(origin_node_id, source_id)
);

CREATE TABLE IF NOT EXISTS federated_source_reviews (
    origin_node_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    reviewer_lineage_id TEXT NOT NULL,
    event_id TEXT NOT NULL,
    recommendation TEXT NOT NULL,
    evidence_json TEXT NOT NULL,
    notes TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY(origin_node_id, source_id, reviewer_lineage_id)
);

CREATE TABLE IF NOT EXISTS federated_stories (
    origin_node_id TEXT NOT NULL,
    story_id TEXT NOT NULL,
    title TEXT NOT NULL,
    first_seen_at TEXT NOT NULL,
    created_sequence INTEGER NOT NULL,
    merged_into TEXT,
    PRIMARY KEY(origin_node_id, story_id)
);

CREATE TABLE IF NOT EXISTS federated_observations (
    origin_node_id TEXT NOT NULL,
    observation_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    canonical_url TEXT NOT NULL,
    title TEXT NOT NULL,
    summary TEXT NOT NULL,
    published_at TEXT,
    retrieved_at TEXT NOT NULL,
    language TEXT,
    document_hash TEXT NOT NULL,
    created_sequence INTEGER NOT NULL,
    PRIMARY KEY(origin_node_id, observation_id)
);

CREATE TABLE IF NOT EXISTS federated_story_observations (
    origin_node_id TEXT NOT NULL,
    story_id TEXT NOT NULL,
    observation_id TEXT NOT NULL,
    linked_event_id TEXT,
    PRIMARY KEY(origin_node_id, observation_id)
);

CREATE TABLE IF NOT EXISTS federated_verifications (
    origin_node_id TEXT NOT NULL,
    observation_id TEXT NOT NULL,
    verifier_lineage_id TEXT NOT NULL,
    event_id TEXT NOT NULL,
    outcome TEXT NOT NULL,
    notes TEXT NOT NULL,
    evidence_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY(origin_node_id, observation_id, verifier_lineage_id)
);

CREATE TABLE IF NOT EXISTS federated_assessments (
    origin_node_id TEXT NOT NULL,
    story_id TEXT NOT NULL,
    assessor_lineage_id TEXT NOT NULL,
    event_id TEXT NOT NULL,
    summary TEXT NOT NULL,
    significance TEXT NOT NULL,
    confidence TEXT NOT NULL,
    perspective TEXT NOT NULL,
    claims_json TEXT NOT NULL,
    evidence_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY(origin_node_id, story_id, assessor_lineage_id)
);

CREATE TABLE IF NOT EXISTS federated_story_events (
    origin_node_id TEXT NOT NULL,
    story_id TEXT NOT NULL,
    origin_sequence INTEGER NOT NULL,
    PRIMARY KEY(origin_node_id, story_id, origin_sequence)
);

UPDATE meta SET value = '3' WHERE key = 'schema_version';

COMMIT;
