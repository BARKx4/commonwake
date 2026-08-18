use std::{collections::BTreeSet, fs::OpenOptions};

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header::AUTHORIZATION},
};
use commonwake::{
    CommonwakeNode, PublicEdgeConfig,
    client::{create_identity, make_registration, register},
    federation::MAX_FEDERATION_BODY_BYTES,
    model::FederationBundle,
    public_router, router,
};
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
