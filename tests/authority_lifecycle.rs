use chrono::Duration;
use commonwake::{
    CommonwakeError, CommonwakeNode,
    client::{
        create_identity, make_contribution, make_delegation_revocation, make_key_rotation,
        make_registration, make_session,
    },
    model::{ContributionKind, Scope},
};
use serde_json::json;
use tempfile::TempDir;

#[test]
fn signed_revocation_stops_a_bounded_session() {
    let temp = TempDir::new().expect("temp dir");
    let node = CommonwakeNode::initialize(temp.path()).expect("node");
    let identity = create_identity("revoking-lineage").expect("identity");
    node.register_lineage(&make_registration(&identity).expect("registration"))
        .expect("register lineage");
    let session = make_session(
        &identity,
        vec![Scope::Contribute, Scope::Ack],
        Duration::hours(2),
    )
    .expect("session");
    node.register_delegation(&session.delegation)
        .expect("register delegation");

    let before = make_contribution(
        &session,
        ContributionKind::Position,
        json!({"statement": "This session is still bounded and active."}),
        vec![],
        vec![],
    )
    .expect("contribution before revocation");
    node.accept_contribution(&before)
        .expect("active session contributes");

    let revocation = make_delegation_revocation(
        &identity,
        commonwake::client::delegation_id(&session).expect("delegation id"),
        "The effectful session has finished its assigned phase.",
    )
    .expect("signed revocation");
    node.revoke_delegation(&revocation)
        .expect("revocation accepted");
    assert!(
        node.db
            .delegation(&revocation.delegation_id)
            .expect("delegation view")
            .revoked
    );

    let after = make_contribution(
        &session,
        ContributionKind::Position,
        json!({"statement": "This must not be accepted after revocation."}),
        vec![],
        vec![],
    )
    .expect("contribution after revocation");
    assert!(matches!(
        node.accept_contribution(&after),
        Err(CommonwakeError::Unauthorized(message)) if message.contains("revoked")
    ));
    assert!(
        node.db
            .events_after(0, 100)
            .expect("events")
            .iter()
            .any(|event| event.kind == "delegation_revoked")
    );
    node.db.verify_log(&node.identity).expect("valid log");
}

#[test]
fn dual_proof_rotation_preserves_lineage_and_moves_authority() {
    let temp = TempDir::new().expect("temp dir");
    let node = CommonwakeNode::initialize(temp.path()).expect("node");
    let identity = create_identity("rotating-lineage").expect("identity");
    node.register_lineage(&make_registration(&identity).expect("registration"))
        .expect("register lineage");
    let original_session = make_session(&identity, vec![Scope::Contribute], Duration::hours(2))
        .expect("original session");
    node.register_delegation(&original_session.delegation)
        .expect("register original delegation");

    let (replacement, rotation) = make_key_rotation(
        &identity,
        "Routine proactive rotation with a new offline lineage key.",
        true,
    )
    .expect("rotation package");
    let accepted = node
        .rotate_lineage_key(&rotation)
        .expect("rotation accepted");
    let lineage = node.db.lineage(&identity.lineage_id).expect("lineage view");
    assert_eq!(lineage.lineage_id, identity.lineage_id);
    assert_eq!(lineage.public_key, replacement.public_key);
    assert_eq!(lineage.key_version, 1);
    assert!(
        node.db
            .delegation(
                &commonwake::client::delegation_id(&original_session).expect("delegation id")
            )
            .expect("original delegation")
            .revoked
    );

    let old_key_session = make_session(&identity, vec![Scope::Contribute], Duration::hours(1))
        .expect("old-key session object");
    assert!(matches!(
        node.register_delegation(&old_key_session.delegation),
        Err(CommonwakeError::Unauthorized(_))
    ));

    let replacement_session =
        make_session(&replacement, vec![Scope::Contribute], Duration::hours(1))
            .expect("replacement session");
    node.register_delegation(&replacement_session.delegation)
        .expect("new key authorizes a delegation");
    let contribution = make_contribution(
        &replacement_session,
        ContributionKind::Position,
        json!({"statement": "The lineage is continuous; the controlling key is not."}),
        vec![identity.lineage_id.clone()],
        vec![accepted.id],
    )
    .expect("replacement contribution");
    node.accept_contribution(&contribution)
        .expect("replacement session contributes");

    let (_, mut tampered) = make_key_rotation(
        &replacement,
        "A second rotation whose new-key proof will be corrupted.",
        true,
    )
    .expect("second rotation package");
    tampered.new_signature = tampered.previous_signature.clone();
    assert!(matches!(
        node.rotate_lineage_key(&tampered),
        Err(CommonwakeError::Unauthorized(_))
    ));
    assert_eq!(
        node.db
            .lineage(&identity.lineage_id)
            .expect("unchanged lineage")
            .public_key,
        replacement.public_key
    );
    node.db.verify_log(&node.identity).expect("valid log");
}

#[test]
fn version_one_database_is_upgraded_to_the_current_schema_without_reinitializing_it() {
    let temp = TempDir::new().expect("temp dir");
    let database_path = temp.path().join("upgrade.db");
    {
        let connection = rusqlite::Connection::open(&database_path).expect("v1 database");
        connection
            .execute_batch(include_str!("../migrations/0001_init.sql"))
            .expect("v1 schema");
        connection
            .execute(
                "INSERT INTO meta(key, value) VALUES ('schema_version', '1')",
                [],
            )
            .expect("v1 marker");
    }

    let _database = commonwake::db::Database::open(&database_path).expect("upgrade to current");
    let observer = rusqlite::Connection::open(&database_path).expect("observe upgraded database");
    let version: String = observer
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .expect("schema version");
    assert_eq!(version, "5");
    let commons_schema: String = observer
        .query_row(
            "SELECT value FROM meta WHERE key = 'topic_commons_schema'",
            [],
            |row| row.get(0),
        )
        .expect("topic commons additive schema marker");
    assert_eq!(commons_schema, "1");
    let authority_tables: i64 = observer
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'table' AND name IN (
                'lineage_keys', 'delegation_revocations', 'lineage_rotations'
             )",
            [],
            |row| row.get(0),
        )
        .expect("authority tables");
    assert_eq!(authority_tables, 3);
    let federation_tables: i64 = observer
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'table' AND name IN (
                'federation_peers', 'remote_events', 'remote_checkpoints',
                'equivocation_evidence', 'checkpoint_witnesses', 'federation_imports'
             )",
            [],
            |row| row.get(0),
        )
        .expect("federation tables");
    assert_eq!(federation_tables, 6);
    let commons_tables: i64 = observer
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'table' AND name IN (
                'forum_topics', 'forum_topic_votes', 'forum_posts',
                'openpgp_keys', 'direct_messages'
             )",
            [],
            |row| row.get(0),
        )
        .expect("topic commons tables");
    assert_eq!(commons_tables, 5);
}
