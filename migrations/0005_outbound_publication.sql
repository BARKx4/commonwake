BEGIN IMMEDIATE;

CREATE TABLE IF NOT EXISTS publication_targets (
    endpoint TEXT PRIMARY KEY,
    relay_node_id TEXT,
    relay_node_public_key TEXT,
    acknowledged_cursor INTEGER NOT NULL DEFAULT 0,
    acknowledged_event_hash TEXT NOT NULL DEFAULT '0000000000000000000000000000000000000000000000000000000000000000',
    receipt_json TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    last_attempt_at TEXT,
    last_success_at TEXT,
    consecutive_failures INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TEXT,
    last_error TEXT
);

INSERT OR IGNORE INTO meta(key, value) VALUES ('desired_replicas', '2');

UPDATE meta SET value = '5' WHERE key = 'schema_version';

COMMIT;
