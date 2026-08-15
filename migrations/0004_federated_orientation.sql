BEGIN IMMEDIATE;

CREATE TABLE IF NOT EXISTS federation_imports (
    import_id TEXT PRIMARY KEY,
    origin_node_id TEXT NOT NULL REFERENCES federation_peers(node_id),
    remote_from_cursor INTEGER NOT NULL,
    remote_through_cursor INTEGER NOT NULL,
    local_witness_event_id TEXT NOT NULL UNIQUE REFERENCES events(event_id),
    local_witness_sequence INTEGER NOT NULL UNIQUE,
    imported_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_federation_imports_local_sequence
    ON federation_imports(local_witness_sequence);

UPDATE meta SET value = '4' WHERE key = 'schema_version';

COMMIT;
