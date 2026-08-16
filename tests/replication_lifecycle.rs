use std::net::SocketAddr;

use chrono::{DateTime, Utc};
use commonwake::{
    CommonwakeError, CommonwakeNode, PROTOCOL_VERSION,
    client::{create_identity, fetch_relayed_federation_bundle, make_registration},
    crypto::{CHECKPOINT_DOMAIN, REPLICATION_RECEIPT_DOMAIN, encode, prefixed_id, sign_object},
    federation::{verify_checkpoint, verify_replication_receipt},
    model::{Checkpoint, ReplicationReceipt},
    publication::publish_origin,
    router,
};
use ed25519_dalek::SigningKey;
use tempfile::TempDir;
use tokio::{net::TcpListener, task::JoinHandle};

async fn test_peer(node: CommonwakeNode) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("ephemeral listener");
    let address: SocketAddr = listener.local_addr().expect("listener address");
    let task = tokio::spawn(async move {
        axum::serve(listener, router(node))
            .await
            .expect("test peer server");
    });
    (format!("http://{address}"), task)
}

fn add_origin_event(node: &CommonwakeNode) {
    let identity = create_identity("replicated-origin-author").expect("lineage identity");
    node.register_lineage(&make_registration(&identity).expect("registration"))
        .expect("origin event");
}

#[test]
fn replication_receipt_has_a_deterministic_signed_fixture() {
    let origin_key = SigningKey::from_bytes(&[7_u8; 32]);
    let relay_key = SigningKey::from_bytes(&[9_u8; 32]);
    let created_at = "2026-08-15T12:00:00Z"
        .parse::<DateTime<Utc>>()
        .expect("fixture timestamp");
    let retained_at = "2026-08-15T12:01:00Z"
        .parse::<DateTime<Utc>>()
        .expect("fixture timestamp");
    let mut checkpoint = Checkpoint {
        node_id: prefixed_id("cwnode_", &origin_key.verifying_key().to_bytes()),
        node_public_key: encode(origin_key.verifying_key().to_bytes()),
        cursor: 0,
        event_hash: "0000000000000000000000000000000000000000000000000000000000000000".into(),
        created_at,
        signature: String::new(),
    };
    checkpoint.signature = sign_object(&origin_key, CHECKPOINT_DOMAIN, &checkpoint).expect("sign");
    let mut receipt = ReplicationReceipt {
        protocol: PROTOCOL_VERSION.into(),
        relay_node_id: prefixed_id("cwnode_", &relay_key.verifying_key().to_bytes()),
        relay_node_public_key: encode(relay_key.verifying_key().to_bytes()),
        origin_checkpoint: checkpoint,
        retained_at,
        signature: String::new(),
    };
    receipt.signature =
        sign_object(&relay_key, REPLICATION_RECEIPT_DOMAIN, &receipt).expect("sign receipt");

    assert_eq!(
        receipt.signature,
        "2KDpqGCnIMQqDLjYEhuCu7hsQl_bddDMAl1GKkxbyvsdEKYXr45fgxpqyHKkdKqaK-WY-_ubgEYL2lYLqiciCg"
    );
    verify_checkpoint(&receipt.origin_checkpoint).expect("fixture checkpoint");
    verify_replication_receipt(&receipt).expect("fixture receipt");
    let mut tampered = receipt;
    tampered.retained_at += chrono::Duration::seconds(1);
    assert!(matches!(
        verify_replication_receipt(&tampered),
        Err(CommonwakeError::Unauthorized(_))
    ));
}

#[test]
fn join_initialization_is_idempotent_without_replacing_the_node_identity() {
    let node_dir = TempDir::new().expect("node dir");
    let (first, initialized) =
        CommonwakeNode::open_or_initialize(node_dir.path()).expect("first join");
    assert!(initialized);
    let node_id = first.identity.node_id().to_owned();
    drop(first);

    let (second, initialized) =
        CommonwakeNode::open_or_initialize(node_dir.path()).expect("second join");
    assert!(!initialized);
    assert_eq!(second.identity.node_id(), node_id);
}

#[tokio::test]
async fn outbound_receipts_survive_restart_and_relays_outlive_the_origin() {
    let origin_dir = TempDir::new().expect("origin dir");
    let relay_a_dir = TempDir::new().expect("relay A dir");
    let relay_b_dir = TempDir::new().expect("relay B dir");
    let reader_dir = TempDir::new().expect("reader dir");
    let origin = CommonwakeNode::initialize(origin_dir.path()).expect("origin");
    let relay_a = CommonwakeNode::initialize(relay_a_dir.path()).expect("relay A");
    let relay_b = CommonwakeNode::initialize(relay_b_dir.path()).expect("relay B");
    add_origin_event(&origin);
    origin.db.set_desired_replicas(2).expect("replica target");
    assert_eq!(
        origin
            .db
            .work_page(None, 10, Some("replicate_origin"))
            .expect("under-replication work")
            .items
            .len(),
        1
    );
    let origin_id = origin.identity.node_id().to_owned();
    let origin_head = origin.db.current_head().expect("origin head");

    let (endpoint_a, task_a) = test_peer(relay_a.clone()).await;
    let (endpoint_b, task_b) = test_peer(relay_b).await;
    let first = publish_origin(&origin, &endpoint_a, 100, None)
        .await
        .expect("publish A");
    let second = publish_origin(&origin, &endpoint_b, 100, None)
        .await
        .expect("publish B");
    assert!(first.caught_up && second.caught_up);
    let health = origin
        .db
        .replication_health(&origin.identity)
        .expect("replication health");
    assert_eq!(health.status, "replicated");
    assert_eq!(health.confirmed_current_replicas, 2);
    assert_eq!(health.recently_reconfirmed_current_replicas, 2);
    assert!(health.targets.iter().all(|target| target.receipt.is_some()));
    assert!(
        origin
            .db
            .work_page(None, 10, Some("replicate_origin"))
            .expect("completed replication work")
            .items
            .is_empty()
    );

    drop(origin);
    let restarted = CommonwakeNode::open(origin_dir.path()).expect("restarted origin");
    let after_restart = restarted
        .db
        .replication_health(&restarted.identity)
        .expect("persisted health");
    assert_eq!(after_restart.confirmed_current_replicas, 2);
    assert_eq!(after_restart.current_cursor, origin_head.0);
    assert_eq!(after_restart.current_event_hash, origin_head.1);
    drop(restarted);

    let reader = CommonwakeNode::initialize(reader_dir.path()).expect("reader");
    let mirrored = fetch_relayed_federation_bundle(&endpoint_a, &origin_id, 0, 100)
        .await
        .expect("origin recovered from relay while origin is offline");
    reader
        .import_federation_bundle(&mirrored)
        .expect("reader imports relayed origin");
    let retained = reader
        .db
        .federation_peers()
        .expect("reader origins")
        .into_iter()
        .find(|peer| peer.node_id == origin_id)
        .expect("origin retained by reader");
    assert_eq!(retained.cursor, origin_head.0);
    assert_eq!(retained.event_hash, origin_head.1);

    task_a.abort();
    task_b.abort();
}

#[tokio::test]
async fn two_urls_for_one_relay_count_once_and_endpoint_identity_is_pinned() {
    let origin_dir = TempDir::new().expect("origin dir");
    let relay_dir = TempDir::new().expect("relay dir");
    let replacement_dir = TempDir::new().expect("replacement dir");
    let origin = CommonwakeNode::initialize(origin_dir.path()).expect("origin");
    let relay = CommonwakeNode::initialize(relay_dir.path()).expect("relay");
    let replacement = CommonwakeNode::initialize(replacement_dir.path()).expect("replacement");
    add_origin_event(&origin);
    origin.db.set_desired_replicas(2).expect("replica target");

    let (endpoint_a, task_a) = test_peer(relay.clone()).await;
    let (endpoint_b, task_b) = test_peer(relay.clone()).await;
    publish_origin(&origin, &endpoint_a, 100, None)
        .await
        .expect("first relay URL");
    publish_origin(&origin, &endpoint_b, 100, None)
        .await
        .expect("second relay URL");
    let health = origin
        .db
        .replication_health(&origin.identity)
        .expect("health");
    assert_eq!(health.targets.len(), 2);
    assert_eq!(health.confirmed_current_replicas, 1);
    assert_eq!(health.status, "degraded");

    let replacement_receipt = replacement
        .make_replication_receipt(&origin.checkpoint().expect("checkpoint"))
        .expect("replacement receipt");
    assert!(matches!(
        origin.db.record_publication_success(
            &origin.identity,
            &format!("{endpoint_a}/"),
            &replacement_receipt
        ),
        Err(CommonwakeError::Conflict(message)) if message.contains("changed relay identity")
    ));

    task_a.abort();
    task_b.abort();
}

#[tokio::test]
async fn failed_publication_is_retained_with_bounded_backoff() {
    let origin_dir = TempDir::new().expect("origin dir");
    let origin = CommonwakeNode::initialize(origin_dir.path()).expect("origin");
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("unused address reservation");
    let endpoint = format!("http://{}", listener.local_addr().expect("address"));
    drop(listener);

    assert!(
        publish_origin(&origin, &endpoint, 100, Some(1))
            .await
            .is_err()
    );
    let target = origin
        .db
        .replication_health(&origin.identity)
        .expect("health after failure")
        .targets
        .remove(0);
    assert_eq!(target.consecutive_failures, 1);
    assert!(target.next_attempt_at.is_some_and(|next| next > Utc::now()));
    assert!(target.last_error.is_some());
}
