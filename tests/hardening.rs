use chrono::{Duration, Utc};
use commonwake::{
    CommonwakeError, CommonwakeNode,
    client::{create_identity, make_contribution, make_registration, make_session},
    crypto::{LINEAGE_DOMAIN, sign_object, signing_key_from_b64},
    federation::MAX_CANONICAL_OBJECT_BYTES,
    model::{ContributionKind, Scope},
    router,
};
use serde_json::json;
use tempfile::TempDir;
use tower::ServiceExt;

#[test]
fn future_dated_lineage_registration_is_rejected_even_when_self_signed() {
    let temp = TempDir::new().expect("temp dir");
    let node = CommonwakeNode::initialize(temp.path()).expect("node");
    let identity = create_identity("future-registration").expect("identity");
    let mut registration = make_registration(&identity).expect("registration");
    registration.created_at = Utc::now() + Duration::minutes(10);
    registration.signature = sign_object(
        &signing_key_from_b64(&identity.secret_key).expect("lineage key"),
        LINEAGE_DOMAIN,
        &registration,
    )
    .expect("future registration signature");

    assert!(matches!(
        node.register_lineage(&registration),
        Err(CommonwakeError::Validation(message)) if message.contains("future")
    ));
    assert_eq!(node.db.current_head().expect("empty head").0, 0);
}

#[test]
fn oversized_signed_protocol_object_is_rejected_before_append() {
    let temp = TempDir::new().expect("temp dir");
    let node = CommonwakeNode::initialize(temp.path()).expect("node");
    let identity = create_identity("bounded-object-author").expect("identity");
    node.register_lineage(&make_registration(&identity).expect("registration"))
        .expect("register lineage");
    let session =
        make_session(&identity, vec![Scope::Contribute], Duration::hours(1)).expect("session");
    node.register_delegation(&session.delegation)
        .expect("register delegation");
    let contribution = make_contribution(
        &session,
        ContributionKind::Position,
        json!({"statement": "x".repeat(MAX_CANONICAL_OBJECT_BYTES)}),
        vec![],
        vec![],
    )
    .expect("large signed contribution");

    assert!(matches!(
        node.accept_contribution(&contribution),
        Err(CommonwakeError::Validation(message)) if message.contains("canonical protocol object")
    ));
    assert_eq!(node.db.current_head().expect("unchanged head").0, 2);
    node.db
        .verify_log(&node.identity)
        .expect("remaining log is valid");
}

#[test]
fn log_verification_detects_mutated_projection_columns() {
    let temp = TempDir::new().expect("temp dir");
    let node = CommonwakeNode::initialize(temp.path()).expect("node");
    let identity = create_identity("projection-integrity").expect("identity");
    node.register_lineage(&make_registration(&identity).expect("registration"))
        .expect("register lineage");
    node.db
        .verify_log(&node.identity)
        .expect("baseline log verifies");

    let observer = rusqlite::Connection::open(temp.path().join("commonwake.db"))
        .expect("external database observer");
    observer
        .execute(
            "UPDATE events SET targets_json = '[\"cwlin_forged_projection\"]' WHERE sequence = 1",
            [],
        )
        .expect("mutate unauthenticated projection column");

    let public_view = node.db.events_after(0, 10).expect("event view");
    assert_eq!(public_view[0].targets, vec![identity.lineage_id]);
    assert!(public_view[0].canonical.is_object());
    assert!(matches!(
        node.db.verify_log(&node.identity),
        Err(CommonwakeError::Unauthorized(message)) if message.contains("mutable projection")
    ));
}

#[tokio::test]
async fn federation_import_has_a_bounded_but_larger_route_specific_body_limit() {
    let temp = TempDir::new().expect("temp dir");
    let node = CommonwakeNode::initialize(temp.path()).expect("node");
    let body = vec![b'{'; 300 * 1024];

    let ordinary = router(node.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/contributions")
                .header("content-type", "application/json")
                .body(Body::from(body.clone()))
                .expect("ordinary request"),
        )
        .await
        .expect("ordinary response");
    assert_eq!(ordinary.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let federation = router(node)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/federation/import")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .expect("federation request"),
        )
        .await
        .expect("federation response");
    assert_ne!(federation.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert!(federation.status().is_client_error());
}
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
