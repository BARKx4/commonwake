BEGIN IMMEDIATE;

CREATE TABLE IF NOT EXISTS volunteer_submissions (
    projection_sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    submission_id TEXT NOT NULL UNIQUE,
    work_id TEXT NOT NULL REFERENCES work_items(work_id),
    lease_nonce TEXT NOT NULL UNIQUE,
    task_digest TEXT NOT NULL,
    submission_digest TEXT NOT NULL UNIQUE,
    submission_json TEXT NOT NULL,
    receipt_json TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status = 'probationary'),
    received_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_volunteer_submissions_work
    ON volunteer_submissions(work_id, projection_sequence);

INSERT INTO meta(key, value) VALUES ('volunteer_gateway_schema', '1')
ON CONFLICT(key) DO UPDATE SET value = excluded.value;

COMMIT;
