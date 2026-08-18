use std::{collections::BTreeSet, fs::OpenOptions};

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header::AUTHORIZATION},
};
use chrono::Duration;
use commonwake::{
    CommonwakeNode, PublicEdgeConfig,
    client::{
        acknowledge, contribute, create_identity, delegate, make_acknowledgement,
        make_contribution, make_delegation_revocation, make_key_rotation, make_registration,
        make_session, register, revoke, rotate,
    },
    federation::MAX_FEDERATION_BODY_BYTES,
    model::{ContributionKind, FederationBundle, MemoryProvenance, Scope},
    public_router, router,
};
use serde_json::json;
use tempfile::TempDir;
use tower::ServiceExt;

fn origin_with_events(directory: &TempDir, events: usize) -> CommonwakeNode {
    let node = CommonwakeNode::initialize(directory.path()).expect("origin node");
    for index in 0..events {
        let identity = create_identity(&format!("publisher-{index}")).expect("identity");
        node.register_lineage(&make_registration(&identity).expect("registration"))
            .expect("origin event");
    }
    node
}

fn complete_bundle(node: &CommonwakeNode) -> FederationBundle {
    node.federation_bundle(0, 500).expect("origin bundle")
}

fn json_request(method: &str, uri: &str, value: &impl serde::Serialize) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(value).expect("request JSON")))
        .expect("request")
}

#[tokio::test]
async fn public_edge_is_read_only_until_a_bearer_is_configured() {
    let directory = TempDir::new().expect("temp dir");
    let node = CommonwakeNode::initialize(directory.path()).expect("relay node");
    let identity = create_identity("admitted-writer").expect("identity");
    let registration = make_registration(&identity).expect("registration");

    let read = public_router(node.clone(), PublicEdgeConfig::default())
        .expect("public router")
        .oneshot(
            Request::builder()
                .uri("/v1/health")
                .body(Body::empty())
                .expect("health request"),
        )
        .await
        .expect("health response");
    assert_eq!(read.status(), StatusCode::OK);
    assert_eq!(
        read.headers()["strict-transport-security"],
        "max-age=31536000"
    );

    let robots = public_router(node.clone(), PublicEdgeConfig::default())
        .expect("public router")
        .oneshot(
            Request::builder()
                .uri("/robots.txt")
                .body(Body::empty())
                .expect("robots request"),
        )
        .await
        .expect("robots response");
    assert_eq!(robots.status(), StatusCode::OK);
    let robots_body = to_bytes(robots.into_body(), 4_096)
        .await
        .expect("robots body");
    let robots_text = String::from_utf8(robots_body.to_vec()).expect("robots text");
    assert!(robots_text.contains("User-agent: OAI-SearchBot\nAllow: /"));
    assert!(robots_text.contains("User-agent: ChatGPT-User\nAllow: /"));

    let denied = public_router(node.clone(), PublicEdgeConfig::default())
        .expect("public router")
        .oneshot(json_request("POST", "/v1/lineages", &registration))
        .await
        .expect("denied response");
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    assert_eq!(node.db.current_head().expect("unchanged log").0, 0);

    let config = PublicEdgeConfig {
        write_token: Some("a-secure-public-write-token-value".into()),
        ..PublicEdgeConfig::default()
    };
    let mut admitted = json_request("POST", "/v1/lineages", &registration);
    admitted.headers_mut().insert(
        AUTHORIZATION,
        "Bearer a-secure-public-write-token-value"
            .parse()
            .expect("authorization header"),
    );
    let admitted = public_router(node.clone(), config)
        .expect("public router")
        .oneshot(admitted)
        .await
        .expect("admitted response");
    assert_eq!(admitted.status(), StatusCode::CREATED);

    let local_identity = create_identity("local-admin").expect("identity");
    let local = router(node)
        .oneshot(json_request(
            "POST",
            "/v1/lineages",
            &make_registration(&local_identity).expect("local registration"),
        ))
        .await
        .expect("local response");
    assert_eq!(local.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn signed_client_can_present_a_public_edge_bearer() {
    let directory = TempDir::new().expect("temp dir");
    let node = CommonwakeNode::initialize(directory.path()).expect("relay node");
    let config = PublicEdgeConfig {
        write_token: Some("a-secure-public-write-token-value".into()),
        ..PublicEdgeConfig::default()
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("test listener");
    let address = listener.local_addr().expect("listener address");
    let app = public_router(node.clone(), config).expect("public router");
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("public edge server");
    });

    let identity = create_identity("credentialed-client").expect("identity");
    let registration = make_registration(&identity).expect("registration");
    let endpoint = format!("http://{address}");
    let denied = register(&endpoint, &registration, None)
        .await
        .expect_err("missing bearer must be denied");
    assert!(denied.to_string().contains("HTTP 403"));
    assert_eq!(node.db.current_head().expect("unchanged log").0, 0);

    register(
        &endpoint,
        &registration,
        Some("a-secure-public-write-token-value"),
    )
    .await
    .expect("credentialed registration");
    assert_eq!(node.db.current_head().expect("registration event").0, 1);
    server.abort();
}

#[tokio::test]
async fn registered_lineage_signed_writes_remain_closed_until_explicitly_enabled() {
    let directory = TempDir::new().expect("temp dir");
    let node = CommonwakeNode::initialize(directory.path()).expect("relay node");
    let identity = create_identity("existing-lineage").expect("identity");
    node.register_lineage(&make_registration(&identity).expect("registration"))
        .expect("locally admitted lineage");
    let session =
        make_session(&identity, vec![Scope::Contribute], Duration::hours(1)).expect("session");

    let denied = public_router(node.clone(), PublicEdgeConfig::default())
        .expect("public router")
        .oneshot(json_request("POST", "/v1/delegations", &session.delegation))
        .await
        .expect("denied response");
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    assert_eq!(node.db.current_head().expect("unchanged log").0, 1);
}

#[tokio::test]
async fn registered_lineages_can_use_their_own_signed_authority_without_a_relay_bearer() {
    let directory = TempDir::new().expect("temp dir");
    let node = CommonwakeNode::initialize(directory.path()).expect("relay node");
    let identity = create_identity("self-authorizing-lineage").expect("identity");
    node.register_lineage(&make_registration(&identity).expect("registration"))
        .expect("locally admitted lineage");
    let config = PublicEdgeConfig {
        signed_lineage_writes_enabled: true,
        ..PublicEdgeConfig::default()
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("test listener");
    let address = listener.local_addr().expect("listener address");
    let app = public_router(node.clone(), config).expect("public router");
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("public edge server");
    });
    let endpoint = format!("http://{address}");

    let discovery: serde_json::Value = reqwest::get(format!("{endpoint}/v1/discovery"))
        .await
        .expect("discovery request")
        .json()
        .await
        .expect("discovery JSON");
    assert_eq!(discovery["node"]["registered_lineage_writes"], true);
    assert_eq!(discovery["node"]["bearer_write_admission"], false);
    assert_eq!(
        discovery["node"]["public_write_mode"],
        "registered-lineage-signed"
    );

    let stranger = create_identity("unregistered-lineage").expect("stranger identity");
    let stranger_registration = make_registration(&stranger).expect("stranger registration");
    let denied_registration = register(&endpoint, &stranger_registration, None)
        .await
        .expect_err("new lineage registration still requires operator admission");
    assert!(denied_registration.to_string().contains("HTTP 403"));
    let stranger_session = make_session(&stranger, vec![Scope::Contribute], Duration::hours(1))
        .expect("stranger session");
    let denied_delegation = delegate(&endpoint, &stranger_session.delegation, None)
        .await
        .expect_err("an unknown lineage cannot self-admit");
    assert!(denied_delegation.to_string().contains("HTTP 404"));
    assert_eq!(node.db.current_head().expect("unchanged log").0, 1);

    let session = make_session(
        &identity,
        vec![Scope::Contribute, Scope::Ack],
        Duration::hours(1),
    )
    .expect("bounded session");
    delegate(&endpoint, &session.delegation, None)
        .await
        .expect("lineage-signed delegation");

    let contribution = make_contribution(
        &session,
        ContributionKind::Position,
        json!({"statement": "This concurrent session remains a distinct bounded delegate."}),
        vec![],
        vec![],
    )
    .expect("signed contribution");
    contribute(&endpoint, &contribution, None)
        .await
        .expect("session-signed contribution");

    let acknowledged_cursor = node.db.current_head().expect("current head").0;
    let acknowledgement = make_acknowledgement(
        &session,
        acknowledged_cursor,
        MemoryProvenance {
            statement: "Processed inherited records without claiming direct memory.".into(),
            local_digest: None,
            direct_memory_claimed: false,
        },
    )
    .expect("signed acknowledgement");
    acknowledge(&endpoint, &acknowledgement, None)
        .await
        .expect("session-signed acknowledgement");

    let revocation = make_delegation_revocation(
        &identity,
        commonwake::client::delegation_id(&session).expect("delegation id"),
        "This bounded branch completed its effectful phase.",
    )
    .expect("signed revocation");
    revoke(&endpoint, &revocation, None)
        .await
        .expect("lineage-signed revocation");

    let (replacement, rotation) = make_key_rotation(
        &identity,
        "Test the complete self-authenticated authority lifecycle.",
        true,
    )
    .expect("rotation package");
    rotate(&endpoint, &rotation, None)
        .await
        .expect("dual-proof key rotation");
    assert_eq!(
        node.db
            .lineage(&identity.lineage_id)
            .expect("rotated lineage")
            .public_key,
        replacement.public_key
    );
    assert_eq!(node.db.current_head().expect("complete lifecycle").0, 6);
    server.abort();
}

#[tokio::test]
async fn admitted_origins_can_publish_but_unlisted_origins_cannot() {
    let origin_directory = TempDir::new().expect("origin temp dir");
    let unlisted_directory = TempDir::new().expect("unlisted temp dir");
    let relay_directory = TempDir::new().expect("relay temp dir");
    let origin = origin_with_events(&origin_directory, 1);
    let unlisted = origin_with_events(&unlisted_directory, 1);
    let relay = CommonwakeNode::initialize(relay_directory.path()).expect("relay node");
    let config = PublicEdgeConfig {
        allowed_publishers: BTreeSet::from([origin.identity.node_id().to_owned()]),
        ..PublicEdgeConfig::default()
    };
    let app = public_router(relay.clone(), config).expect("public router");

    let admitted = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/v1/federation/publish",
            &complete_bundle(&origin),
        ))
        .await
        .expect("admitted publication");
    assert_eq!(admitted.status(), StatusCode::CREATED);
    assert!(
        relay
            .db
            .has_federation_origin(origin.identity.node_id())
            .expect("known origin")
    );

    let denied = app
        .oneshot(json_request(
            "POST",
            "/v1/federation/publish",
            &complete_bundle(&unlisted),
        ))
        .await
        .expect("denied publication");
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    assert!(
        !relay
            .db
            .has_federation_origin(unlisted.identity.node_id())
            .expect("unknown origin")
    );
}

#[tokio::test]
async fn public_edge_bounds_origin_history_request_rate_and_storage() {
    let origin_directory = TempDir::new().expect("origin temp dir");
    let cursor_relay_directory = TempDir::new().expect("cursor relay temp dir");
    let origin = origin_with_events(&origin_directory, 2);
    let cursor_relay =
        CommonwakeNode::initialize(cursor_relay_directory.path()).expect("cursor relay");
    let cursor_config = PublicEdgeConfig {
        allowed_publishers: BTreeSet::from([origin.identity.node_id().to_owned()]),
        max_origin_events: 1,
        ..PublicEdgeConfig::default()
    };
    let cursor_response = public_router(cursor_relay.clone(), cursor_config)
        .expect("cursor router")
        .oneshot(json_request(
            "POST",
            "/v1/federation/publish",
            &complete_bundle(&origin),
        ))
        .await
        .expect("cursor response");
    assert_eq!(cursor_response.status(), StatusCode::INSUFFICIENT_STORAGE);
    assert_eq!(
        cursor_relay
            .db
            .federation_origin_count()
            .expect("origin count"),
        0
    );

    let rate_directory = TempDir::new().expect("rate temp dir");
    let rate_node = CommonwakeNode::initialize(rate_directory.path()).expect("rate node");
    let rate_config = PublicEdgeConfig {
        requests_per_second: 1,
        ..PublicEdgeConfig::default()
    };
    let rate_app = public_router(rate_node, rate_config).expect("rate router");
    let first = rate_app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/health")
                .body(Body::empty())
                .expect("first request"),
        )
        .await
        .expect("first response");
    assert_eq!(first.status(), StatusCode::OK);
    let second = rate_app
        .oneshot(
            Request::builder()
                .uri("/v1/health")
                .body(Body::empty())
                .expect("second request"),
        )
        .await
        .expect("second response");
    assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(second.headers()["retry-after"], "1");

    let storage_directory = TempDir::new().expect("storage temp dir");
    let storage_node = CommonwakeNode::initialize(storage_directory.path()).expect("storage node");
    OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(storage_directory.path().join("bounded-storage-fixture"))
        .expect("storage fixture")
        .set_len(MAX_FEDERATION_BODY_BYTES as u64)
        .expect("bounded fixture length");
    let storage_config = PublicEdgeConfig {
        write_token: Some("a-secure-public-write-token-value".into()),
        max_storage_bytes: MAX_FEDERATION_BODY_BYTES as u64,
        ..PublicEdgeConfig::default()
    };
    let storage_identity = create_identity("storage-writer").expect("identity");
    let mut storage_request = json_request(
        "POST",
        "/v1/lineages",
        &make_registration(&storage_identity).expect("registration"),
    );
    storage_request.headers_mut().insert(
        AUTHORIZATION,
        "Bearer a-secure-public-write-token-value"
            .parse()
            .expect("authorization header"),
    );
    let storage_response = public_router(storage_node.clone(), storage_config)
        .expect("storage router")
        .oneshot(storage_request)
        .await
        .expect("storage response");
    assert_eq!(storage_response.status(), StatusCode::INSUFFICIENT_STORAGE);
    assert_eq!(storage_node.db.current_head().expect("unchanged log").0, 0);
}

#[tokio::test]
async fn concurrent_publishers_cannot_race_past_the_origin_quota() {
    let first_directory = TempDir::new().expect("first origin temp dir");
    let second_directory = TempDir::new().expect("second origin temp dir");
    let relay_directory = TempDir::new().expect("relay temp dir");
    let first = origin_with_events(&first_directory, 1);
    let second = origin_with_events(&second_directory, 1);
    let relay = CommonwakeNode::initialize(relay_directory.path()).expect("relay node");
    let config = PublicEdgeConfig {
        allowed_publishers: BTreeSet::from([
            first.identity.node_id().to_owned(),
            second.identity.node_id().to_owned(),
        ]),
        max_origins: 1,
        ..PublicEdgeConfig::default()
    };
    let app = public_router(relay.clone(), config).expect("public router");
    let first_request = app.clone().oneshot(json_request(
        "POST",
        "/v1/federation/publish",
        &complete_bundle(&first),
    ));
    let second_request = app.oneshot(json_request(
        "POST",
        "/v1/federation/publish",
        &complete_bundle(&second),
    ));
    let (first_response, second_response) = tokio::join!(first_request, second_request);
    let statuses = BTreeSet::from([
        first_response.expect("first response").status(),
        second_response.expect("second response").status(),
    ]);
    assert_eq!(
        statuses,
        BTreeSet::from([StatusCode::CREATED, StatusCode::INSUFFICIENT_STORAGE])
    );
    assert_eq!(relay.db.federation_origin_count().expect("origin count"), 1);
}
