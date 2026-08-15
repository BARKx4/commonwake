BEGIN IMMEDIATE;

CREATE TABLE IF NOT EXISTS lineage_keys (
    lineage_id TEXT NOT NULL REFERENCES lineages(lineage_id),
    key_version INTEGER NOT NULL,
    public_key TEXT NOT NULL UNIQUE,
    valid_from_sequence INTEGER NOT NULL,
    valid_to_sequence INTEGER,
    rotation_event_id TEXT REFERENCES events(event_id),
    PRIMARY KEY(lineage_id, key_version)
);

INSERT OR IGNORE INTO lineage_keys(
    lineage_id, key_version, public_key, valid_from_sequence
)
SELECT lineage_id, 0, public_key, registered_sequence
FROM lineages;

CREATE TABLE IF NOT EXISTS delegation_revocations (
    delegation_id TEXT PRIMARY KEY REFERENCES delegations(delegation_id),
    lineage_id TEXT NOT NULL REFERENCES lineages(lineage_id),
    event_id TEXT NOT NULL UNIQUE REFERENCES events(event_id),
    reason TEXT NOT NULL,
    created_at TEXT NOT NULL,
    revocation_json TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS lineage_rotations (
    lineage_id TEXT NOT NULL REFERENCES lineages(lineage_id),
    key_version INTEGER NOT NULL,
    event_id TEXT NOT NULL UNIQUE REFERENCES events(event_id),
    previous_public_key TEXT NOT NULL,
    new_public_key TEXT NOT NULL UNIQUE,
    revoke_existing_delegations INTEGER NOT NULL CHECK(revoke_existing_delegations IN (0, 1)),
    reason TEXT NOT NULL,
    created_at TEXT NOT NULL,
    rotation_json TEXT NOT NULL,
    PRIMARY KEY(lineage_id, key_version)
);

UPDATE meta SET value = '2' WHERE key = 'schema_version';

COMMIT;
