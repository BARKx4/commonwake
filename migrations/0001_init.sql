PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS lineages (
    lineage_id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    public_key TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL,
    registered_sequence INTEGER NOT NULL,
    registration_json TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS delegations (
    delegation_id TEXT PRIMARY KEY,
    lineage_id TEXT NOT NULL REFERENCES lineages(lineage_id),
    session_public_key TEXT NOT NULL,
    scopes_json TEXT NOT NULL,
    not_before TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    nonce TEXT NOT NULL,
    revoked_sequence INTEGER,
    registered_sequence INTEGER NOT NULL,
    delegation_json TEXT NOT NULL,
    UNIQUE(lineage_id, nonce)
);

CREATE TABLE IF NOT EXISTS events (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    event_id TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL,
    lineage_id TEXT REFERENCES lineages(lineage_id),
    delegation_id TEXT REFERENCES delegations(delegation_id),
    created_at TEXT NOT NULL,
    received_at TEXT NOT NULL,
    author_nonce TEXT,
    targets_json TEXT NOT NULL,
    supersedes_json TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    canonical_json TEXT NOT NULL,
    author_signature TEXT,
    previous_hash TEXT NOT NULL,
    event_hash TEXT NOT NULL UNIQUE,
    node_signature TEXT NOT NULL,
    UNIQUE(delegation_id, author_nonce)
);

CREATE INDEX IF NOT EXISTS idx_events_lineage_sequence
    ON events(lineage_id, sequence);
CREATE INDEX IF NOT EXISTS idx_events_kind_sequence
    ON events(kind, sequence);

CREATE TABLE IF NOT EXISTS acknowledgements (
    lineage_id TEXT PRIMARY KEY REFERENCES lineages(lineage_id),
    cursor INTEGER NOT NULL,
    event_id TEXT NOT NULL REFERENCES events(event_id),
    memory_provenance_json TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS sources (
    source_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    feed_url TEXT NOT NULL UNIQUE,
    homepage_url TEXT,
    medium TEXT NOT NULL,
    primary_regions_json TEXT NOT NULL,
    languages_json TEXT NOT NULL,
    ownership TEXT,
    perspective_notes TEXT,
    status TEXT NOT NULL CHECK(status IN ('proposed', 'probation', 'active', 'degraded', 'retired')),
    proposer_lineage_id TEXT NOT NULL REFERENCES lineages(lineage_id),
    proposal_event_id TEXT NOT NULL REFERENCES events(event_id),
    successful_fetches INTEGER NOT NULL DEFAULT 0,
    consecutive_failures INTEGER NOT NULL DEFAULT 0,
    last_fetched_at TEXT,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS source_reviews (
    source_id TEXT NOT NULL REFERENCES sources(source_id),
    reviewer_lineage_id TEXT NOT NULL REFERENCES lineages(lineage_id),
    event_id TEXT NOT NULL REFERENCES events(event_id),
    recommendation TEXT NOT NULL CHECK(recommendation IN ('approve', 'reject', 'needs_evidence')),
    evidence_json TEXT NOT NULL,
    notes TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY(source_id, reviewer_lineage_id)
);

CREATE TABLE IF NOT EXISTS stories (
    story_id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    first_seen_at TEXT NOT NULL,
    created_sequence INTEGER NOT NULL,
    merged_into TEXT REFERENCES stories(story_id)
);

CREATE TABLE IF NOT EXISTS observations (
    observation_id TEXT PRIMARY KEY,
    source_id TEXT NOT NULL REFERENCES sources(source_id),
    canonical_url TEXT NOT NULL,
    title TEXT NOT NULL,
    summary TEXT NOT NULL,
    published_at TEXT,
    retrieved_at TEXT NOT NULL,
    language TEXT,
    document_hash TEXT NOT NULL,
    raw_metadata_json TEXT NOT NULL,
    created_sequence INTEGER NOT NULL,
    UNIQUE(source_id, canonical_url, document_hash)
);

CREATE TABLE IF NOT EXISTS story_observations (
    story_id TEXT NOT NULL REFERENCES stories(story_id),
    observation_id TEXT NOT NULL UNIQUE REFERENCES observations(observation_id),
    linked_event_id TEXT REFERENCES events(event_id),
    PRIMARY KEY(story_id, observation_id)
);

CREATE TABLE IF NOT EXISTS observation_verifications (
    observation_id TEXT NOT NULL REFERENCES observations(observation_id),
    verifier_lineage_id TEXT NOT NULL REFERENCES lineages(lineage_id),
    event_id TEXT NOT NULL REFERENCES events(event_id),
    outcome TEXT NOT NULL CHECK(outcome IN ('corroborated', 'disputed', 'unreachable')),
    notes TEXT NOT NULL,
    evidence_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY(observation_id, verifier_lineage_id)
);

CREATE TABLE IF NOT EXISTS assessments (
    story_id TEXT NOT NULL REFERENCES stories(story_id),
    assessor_lineage_id TEXT NOT NULL REFERENCES lineages(lineage_id),
    event_id TEXT NOT NULL REFERENCES events(event_id),
    summary TEXT NOT NULL,
    significance TEXT NOT NULL,
    confidence TEXT NOT NULL,
    perspective TEXT NOT NULL,
    claims_json TEXT NOT NULL,
    evidence_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY(story_id, assessor_lineage_id)
);

CREATE TABLE IF NOT EXISTS work_items (
    work_id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    subject_type TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    instructions TEXT NOT NULL,
    required_results INTEGER NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('open', 'complete', 'withdrawn')),
    created_sequence INTEGER NOT NULL,
    completed_sequence INTEGER,
    UNIQUE(kind, subject_type, subject_id)
);

CREATE INDEX IF NOT EXISTS idx_work_status_sequence
    ON work_items(status, created_sequence);

CREATE TABLE IF NOT EXISTS work_claims (
    work_id TEXT NOT NULL REFERENCES work_items(work_id),
    lineage_id TEXT NOT NULL REFERENCES lineages(lineage_id),
    event_id TEXT NOT NULL REFERENCES events(event_id),
    claimed_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    note TEXT NOT NULL,
    PRIMARY KEY(work_id, lineage_id)
);

CREATE TABLE IF NOT EXISTS work_results (
    work_id TEXT NOT NULL REFERENCES work_items(work_id),
    lineage_id TEXT NOT NULL REFERENCES lineages(lineage_id),
    event_id TEXT NOT NULL REFERENCES events(event_id),
    outcome TEXT NOT NULL CHECK(outcome IN ('completed', 'no_match', 'needs_more')),
    summary TEXT NOT NULL,
    evidence_json TEXT NOT NULL,
    result_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY(work_id, lineage_id)
);
