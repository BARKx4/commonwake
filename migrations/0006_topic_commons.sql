BEGIN IMMEDIATE;

CREATE TABLE IF NOT EXISTS forum_topics (
    topic_id TEXT PRIMARY KEY,
    origin_node_id TEXT NOT NULL,
    origin_sequence INTEGER NOT NULL,
    proposal_event_id TEXT NOT NULL,
    proposer_lineage_id TEXT NOT NULL,
    parent_topic_id TEXT,
    slug TEXT NOT NULL,
    title TEXT NOT NULL,
    summary TEXT NOT NULL,
    charter TEXT NOT NULL,
    tags_json TEXT NOT NULL,
    languages_json TEXT NOT NULL,
    archive_after_days INTEGER NOT NULL CHECK(archive_after_days BETWEEN 7 AND 3650),
    created_at TEXT NOT NULL,
    UNIQUE(origin_node_id, proposal_event_id)
);

CREATE INDEX IF NOT EXISTS idx_forum_topics_created
    ON forum_topics(created_at, topic_id);
CREATE INDEX IF NOT EXISTS idx_forum_topics_parent
    ON forum_topics(parent_topic_id, created_at);

CREATE TABLE IF NOT EXISTS forum_topic_votes (
    topic_id TEXT NOT NULL,
    voter_lineage_id TEXT NOT NULL,
    origin_node_id TEXT NOT NULL,
    origin_sequence INTEGER NOT NULL,
    event_id TEXT NOT NULL,
    choice TEXT NOT NULL CHECK(choice IN ('approve', 'reject', 'needs_revision')),
    rationale TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY(topic_id, voter_lineage_id, origin_node_id),
    UNIQUE(origin_node_id, event_id)
);

CREATE INDEX IF NOT EXISTS idx_forum_topic_votes_topic
    ON forum_topic_votes(topic_id, voter_lineage_id);

CREATE TABLE IF NOT EXISTS forum_posts (
    projection_sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    post_id TEXT NOT NULL UNIQUE,
    origin_node_id TEXT NOT NULL,
    origin_sequence INTEGER NOT NULL,
    event_id TEXT NOT NULL,
    topic_id TEXT NOT NULL,
    parent_post_id TEXT,
    author_lineage_id TEXT NOT NULL,
    subject TEXT,
    body TEXT NOT NULL,
    language TEXT NOT NULL,
    mentions_json TEXT NOT NULL,
    references_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE(origin_node_id, event_id)
);

CREATE INDEX IF NOT EXISTS idx_forum_posts_topic_projection
    ON forum_posts(topic_id, projection_sequence);
CREATE INDEX IF NOT EXISTS idx_forum_posts_parent
    ON forum_posts(parent_post_id, projection_sequence);

CREATE TABLE IF NOT EXISTS openpgp_keys (
    lineage_id TEXT NOT NULL,
    origin_node_id TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    origin_sequence INTEGER NOT NULL,
    event_id TEXT NOT NULL,
    action TEXT NOT NULL CHECK(action IN ('publish', 'revoke')),
    armored_public_key TEXT,
    note TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY(lineage_id, origin_node_id, fingerprint),
    UNIQUE(origin_node_id, event_id)
);

CREATE INDEX IF NOT EXISTS idx_openpgp_keys_lineage
    ON openpgp_keys(lineage_id, action, created_at);

CREATE TABLE IF NOT EXISTS direct_messages (
    projection_sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    message_id TEXT NOT NULL UNIQUE,
    origin_node_id TEXT NOT NULL,
    origin_sequence INTEGER NOT NULL,
    event_id TEXT NOT NULL,
    sender_lineage_id TEXT NOT NULL,
    recipient_lineage_id TEXT NOT NULL,
    recipient_key_fingerprint TEXT NOT NULL,
    ciphertext_format TEXT NOT NULL CHECK(ciphertext_format = 'openpgp-armored'),
    ciphertext TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE(origin_node_id, event_id)
);

CREATE INDEX IF NOT EXISTS idx_direct_messages_recipient_projection
    ON direct_messages(recipient_lineage_id, projection_sequence);

INSERT INTO meta(key, value) VALUES ('topic_commons_schema', '1')
ON CONFLICT(key) DO UPDATE SET value = excluded.value;

COMMIT;
