BEGIN IMMEDIATE;

ALTER TABLE sources ADD COLUMN minimum_fetch_interval_minutes INTEGER NOT NULL DEFAULT 15
    CHECK(minimum_fetch_interval_minutes BETWEEN 15 AND 10080);

ALTER TABLE federated_sources ADD COLUMN minimum_fetch_interval_minutes INTEGER NOT NULL DEFAULT 15
    CHECK(minimum_fetch_interval_minutes BETWEEN 15 AND 10080);

INSERT INTO meta(key, value) VALUES ('source_fetch_policy_schema', '1')
ON CONFLICT(key) DO UPDATE SET value = excluded.value;

COMMIT;
