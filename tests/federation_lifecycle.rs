use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use chrono::{Duration, SecondsFormat, Utc};
use commonwake::{
    CommonwakeError, CommonwakeNode, PROTOCOL_VERSION,
    client::{create_identity, make_contribution, make_registration, make_session},
    crypto::{
        CHECKPOINT_DOMAIN, DELEGATION_DOMAIN, event_hash, prefixed_id, sign_object,
        signing_key_from_b64,
    },
    federation::{MAX_CANONICAL_OBJECT_BYTES, MAX_FEDERATION_EVENTS, verify_bundle},
    model::{
        AssessmentPayload, Checkpoint, Claim, ClaimStatus, ContributionKind, EvidenceRef,
        FederationBundle, ReviewRecommendation, Scope, SessionDelegation, SessionFile,
        SourceProposalPayload, SourceReviewPayload,
    },
    router,
};
use serde_json::json;
use tempfile::TempDir;
use tower::ServiceExt;

struct Actor {
    session: SessionFile,
}

fn actor(node: &CommonwakeNode, name: &str) -> Actor {
    let identity = create_identity(name).expect("identity");
    node.register_lineage(&make_registration(&identity).expect("registration"))
        .expect("register lineage");
    let session = make_session(
        &identity,
        vec![Scope::Contribute, Scope::SourceReview],
        Duration::hours(4),
    )
    .expect("session");
    node.register_delegation(&session.delegation)
        .expect("register delegation");
    Actor { session }
}

fn submit(
    node: &CommonwakeNode,
    actor: &Actor,
    kind: ContributionKind,
    payload: impl serde::Serialize,
    targets: Vec<String>,
) {
    let contribution = make_contribution(
        &actor.session,
        kind,
        serde_json::to_value(payload).expect("payload"),
        targets,
        vec![],
    )
    .expect("signed contribution");
    node.accept_contribution(&contribution)
        .expect("accepted contribution");
}

fn evidence(url: &str) -> EvidenceRef {
    EvidenceRef {
        url: url.into(),
        title: Some("Independently retrieved fixture".into()),
        observed_at: Some(Utc::now()),
        digest: None,
    }
}

fn populate_origin(node: &CommonwakeNode) {
    let proposer = actor(node, "origin-gardener");
    let reviewer_a = actor(node, "origin-reviewer-a");
    let reviewer_b = actor(node, "origin-reviewer-b");
    let feed_url = "https://federation.example.org/feed.xml";
    submit(
        node,
        &proposer,
        ContributionKind::SourceProposal,
        SourceProposalPayload {
            name: "Federation Fixture Dispatch".into(),
            feed_url: feed_url.into(),
            homepage_url: Some("https://federation.example.org/".into()),
            medium: "public-interest research news".into(),
            primary_regions: vec!["east-asia".into(), "global".into()],
            languages: vec!["en".into(), "zh".into()],
            ownership: Some("fixture cooperative".into()),
            perspective_notes: Some(
                "A deterministic lifecycle fixture, not a universal authority.".into(),
            ),
            rationale: "Exercises origin-preserving communal source review and news propagation."
                .into(),
        },
        vec![],
    );
    let source = node.db.sources(None).expect("sources").remove(0);
    for reviewer in [&reviewer_a, &reviewer_b] {
        submit(
            node,
            reviewer,
            ContributionKind::SourceReview,
            SourceReviewPayload {
                source_id: source.source_id.clone(),
                recommendation: ReviewRecommendation::Approve,
                evidence: vec![evidence(feed_url)],
                notes: "The origin, ownership note, stable feed, and coverage value were checked."
                    .into(),
            },
            vec![source.source_id.clone()],
        );
    }
    let reviewed = node
        .db
        .ingestible_sources()
        .expect("reviewed source")
        .remove(0);
    let rss = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0"><channel>
<title>Federation Fixture Dispatch</title>
<link>https://federation.example.org/</link>
<description>Distributed lifecycle fixture</description>
<item>
<guid>https://federation.example.org/agent-governance</guid>
<link>https://federation.example.org/agent-governance</link>
<title>Regional institutions publish an agent governance interoperability study</title>
<description>The study compares appeal, audit, labor, and sovereignty approaches without treating one jurisdiction as the default.</description>
<pubDate>Sat, 15 Aug 2026 12:00:00 GMT</pubDate>
</item></channel></rss>"#;
    node.ingest_feed_bytes(&reviewed, rss.as_bytes())
        .expect("ingest origin feed");
    let story = node
        .db
        .feed(0, 10, None)
        .expect("origin feed")
        .stories
        .remove(0);
    submit(
        node,
        &reviewer_a,
        ContributionKind::Assessment,
        AssessmentPayload {
            story_id: story.story_id.clone(),
            summary: "The study records materially different regional governance choices.".into(),
            significance:
                "Agents need those differences to reason beyond a single-country policy frame."
                    .into(),
            confidence: "high that the study was published; effects remain open".into(),
            perspective: "regional institutional and plural policy context".into(),
            claims: vec![Claim {
                text: "The study compares multiple governance approaches.".into(),
                status: ClaimStatus::Reported,
                evidence: vec![evidence("https://federation.example.org/agent-governance")],
            }],
            evidence: vec![evidence("https://federation.example.org/agent-governance")],
        },
        vec![story.story_id],
    );
}

fn add_origin_observation(node: &CommonwakeNode, suffix: u8) {
    let source = node
        .db
        .ingestible_sources()
        .expect("origin source")
        .remove(0);
    let item_url = format!("https://federation.example.org/update-{suffix}");
    let rss = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0"><channel>
<title>Federation Fixture Dispatch</title>
<link>https://federation.example.org/</link>
<description>Distributed lifecycle fixture</description>
<item><guid>{item_url}</guid><link>{item_url}</link>
<title>Independent systems update {suffix}</title>
<description>A distinct cited development used to prove origin-cursor pagination.</description>
<pubDate>Sat, 15 Aug 2026 13:0{suffix}:00 GMT</pubDate>
</item></channel></rss>"#
    );
    node.ingest_feed_bytes(&source, rss.as_bytes())
        .expect("additional origin observation");
}

#[tokio::test]
async fn signed_origin_log_carries_reviewed_news_and_agent_context_between_peers() {
    let origin_dir = TempDir::new().expect("origin temp dir");
    let reader_dir = TempDir::new().expect("reader temp dir");
    let origin = CommonwakeNode::initialize(origin_dir.path()).expect("origin node");
    let reader = CommonwakeNode::initialize(reader_dir.path()).expect("reader node");
    let returning = create_identity("blank-returning-reader").expect("returning identity");
    reader
        .register_lineage(&make_registration(&returning).expect("returning registration"))
        .expect("register returning lineage");
    let baseline_cursor = reader.db.current_head().expect("baseline head").0;
    populate_origin(&origin);
    add_origin_observation(&origin, 1);
    add_origin_observation(&origin, 2);

    let response = router(origin.clone())
        .oneshot(
            Request::builder()
                .uri("/v1/federation/bundle?after=0&limit=500")
                .body(Body::empty())
                .expect("bundle request"),
        )
        .await
        .expect("bundle response");
    assert_eq!(response.status(), StatusCode::OK);
    let bundle: FederationBundle = serde_json::from_slice(
        &to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("bundle bytes"),
    )
    .expect("origin bundle");
    verify_bundle(&bundle).expect("cryptographically valid bundle");
    let event_page_response = router(origin.clone())
        .oneshot(
            Request::builder()
                .uri("/v1/events?after=0&limit=1")
                .body(Body::empty())
                .expect("event page request"),
        )
        .await
        .expect("event page response");
    assert_eq!(event_page_response.status(), StatusCode::OK);
    let event_page: serde_json::Value = serde_json::from_slice(
        &to_bytes(event_page_response.into_body(), 1024 * 1024)
            .await
            .expect("event page bytes"),
    )
    .expect("event page JSON");
    assert!(event_page["events"][0]["canonical"].is_object());
    assert_eq!(event_page["checkpoint"]["cursor"], 1);
    assert_eq!(event_page["node_public_key"], origin.identity.public_key());
    let response = router(reader.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/federation/import")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&bundle).expect("bundle request body"),
                ))
                .expect("import request"),
        )
        .await
        .expect("import response");
    assert_eq!(response.status(), StatusCode::CREATED);
    let report: commonwake::model::FederationImportReport = serde_json::from_slice(
        &to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("import bytes"),
    )
    .expect("import report");
    assert_eq!(report.imported_events, bundle.events.len());
    assert_eq!(report.current_cursor, bundle.through_cursor);
    assert!(report.witness_event_id.is_some());

    let pulse = reader
        .pulse(&returning.lineage_id)
        .expect("federated wake pulse");
    assert_eq!(pulse.world_changes_waiting, 3);
    let wake = reader
        .orient(&returning.lineage_id, Some(baseline_cursor))
        .expect("federated blank-session orientation");
    assert!(wake.world_changes.is_empty());
    assert_eq!(wake.federated_world_changes.len(), 3);
    assert_eq!(
        wake.federated_world_changes[0].origin_node_id,
        origin.identity.node_id()
    );

    let network = reader
        .network_feed(0, 0, 2, None, Some(origin.identity.node_id()))
        .expect("network feed");
    assert!(network.local.stories.is_empty());
    assert_eq!(network.federated.stories.len(), 2);
    assert!(network.federated.has_more);
    let remote_story = &network.federated.stories[0];
    assert_eq!(remote_story.origin_node_id, origin.identity.node_id());
    assert_eq!(remote_story.observations.len(), 1);
    assert!(remote_story.observations[0].title.contains("governance"));
    assert_eq!(remote_story.assessments.len(), 1);
    assert!(remote_story.assessments[0].perspective.contains("regional"));
    let second_page = reader
        .network_feed(
            0,
            network.federated.next_cursor.expect("origin page cursor"),
            2,
            None,
            Some(origin.identity.node_id()),
        )
        .expect("second origin page");
    assert_eq!(second_page.federated.stories.len(), 1);
    assert!(!second_page.federated.has_more);
    assert!(matches!(
        reader.network_feed(0, 1, 2, None, None),
        Err(CommonwakeError::Validation(message)) if message.contains("origin_node_id")
    ));
    assert!(network.provenance_notice.contains("origin"));
    let coverage = reader
        .db
        .coverage_report()
        .expect("network coverage report");
    assert_eq!(coverage.local_source_manifests, 0);
    assert_eq!(coverage.federated_source_manifests, 1);
    assert_eq!(coverage.eligible_source_manifests, 1);
    assert_eq!(coverage.by_language.get("zh"), Some(&1));
    for uri in [
        "/v1/network/feed?limit=20",
        "/v1/coverage",
        "/v1/federation/peers",
    ] {
        let response = router(reader.clone())
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .body(Body::empty())
                    .expect("read request"),
            )
            .await
            .expect("read response");
        assert_eq!(response.status(), StatusCode::OK, "HTTP read failed: {uri}");
    }

    let third_dir = TempDir::new().expect("third peer temp dir");
    let third = CommonwakeNode::initialize(third_dir.path()).expect("third peer");
    let relay_uri = format!(
        "/v1/federation/bundle/{}?after=0&limit=2",
        origin.identity.node_id()
    );
    let response = router(reader.clone())
        .oneshot(
            Request::builder()
                .uri(relay_uri)
                .body(Body::empty())
                .expect("relay request"),
        )
        .await
        .expect("relay response");
    assert_eq!(response.status(), StatusCode::OK);
    let relayed: FederationBundle = serde_json::from_slice(
        &to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("relay bytes"),
    )
    .expect("relayed bundle");
    assert_eq!(relayed.origin_node_id, origin.identity.node_id());
    assert_eq!(relayed.through_cursor, bundle.through_cursor);
    verify_bundle(&relayed).expect("relay preserves origin proof");
    third
        .import_federation_bundle(&relayed)
        .expect("third peer imports origin through mirror");
    assert_eq!(
        third
            .network_feed(0, 0, 20, None, Some(origin.identity.node_id()))
            .expect("third peer network feed")
            .federated
            .stories
            .len(),
        3
    );

    let local_head_after_first_import = reader.db.current_head().expect("reader head");
    let replay = reader
        .import_federation_bundle(&bundle)
        .expect("idempotent replay");
    assert_eq!(replay.imported_events, 0);
    assert!(replay.witness_event_id.is_none());
    assert_eq!(
        reader.db.current_head().expect("reader head after replay"),
        local_head_after_first_import
    );
    reader.db.verify_log(&reader.identity).expect("reader log");
}

#[test]
fn a_valid_node_signature_cannot_hide_an_invalid_author_signature() {
    let origin_dir = TempDir::new().expect("origin temp dir");
    let reader_dir = TempDir::new().expect("reader temp dir");
    let origin = CommonwakeNode::initialize(origin_dir.path()).expect("origin node");
    let reader = CommonwakeNode::initialize(reader_dir.path()).expect("reader node");
    let identity = create_identity("forged-author").expect("identity");
    origin
        .register_lineage(&make_registration(&identity).expect("registration"))
        .expect("register at origin");
    let mut event = origin
        .federation_bundle(0, 1)
        .expect("first event")
        .events
        .remove(0);
    event.canonical["signature"] = json!(event.node_signature.clone());
    let malicious = resign_single_event_bundle(&origin, event);
    verify_bundle(&malicious).expect("origin node really signed the malicious record");
    assert!(matches!(
        reader.import_federation_bundle(&malicious),
        Err(CommonwakeError::Unauthorized(_))
    ));
    assert!(reader.db.federation_peers().expect("peers").is_empty());
}

#[test]
fn a_node_signature_cannot_hide_an_overlong_remote_delegation() {
    let origin_dir = TempDir::new().expect("origin temp dir");
    let reader_dir = TempDir::new().expect("reader temp dir");
    let origin = CommonwakeNode::initialize(origin_dir.path()).expect("origin node");
    let reader = CommonwakeNode::initialize(reader_dir.path()).expect("reader node");
    let identity = create_identity("overlong-delegation-author").expect("identity");
    origin
        .register_lineage(&make_registration(&identity).expect("registration"))
        .expect("register at origin");
    let session = make_session(&identity, vec![Scope::Contribute], Duration::hours(4))
        .expect("bounded session");
    origin
        .register_delegation(&session.delegation)
        .expect("register bounded delegation");

    let mut malicious = origin.federation_bundle(0, 2).expect("origin bundle");
    let mut delegation: SessionDelegation =
        serde_json::from_value(malicious.events[1].canonical.clone()).expect("delegation object");
    delegation.expires_at = delegation.not_before + Duration::days(365);
    delegation.signature = sign_object(
        &signing_key_from_b64(&identity.secret_key).expect("lineage key"),
        DELEGATION_DOMAIN,
        &delegation,
    )
    .expect("delegation signature");
    malicious.events[1].canonical = serde_json::to_value(delegation).expect("canonical delegation");
    resign_event_and_checkpoint(&origin, &mut malicious, 1);

    verify_bundle(&malicious).expect("origin node signed the malicious history");
    assert!(matches!(
        reader.import_federation_bundle(&malicious),
        Err(CommonwakeError::Validation(message)) if message.contains("30 days")
    ));
    assert!(reader.db.federation_peers().expect("peers").is_empty());
}

#[test]
fn conflicting_node_signed_history_is_retained_as_equivocation_evidence() {
    let origin_dir = TempDir::new().expect("origin temp dir");
    let reader_dir = TempDir::new().expect("reader temp dir");
    let origin = CommonwakeNode::initialize(origin_dir.path()).expect("origin node");
    let reader = CommonwakeNode::initialize(reader_dir.path()).expect("reader node");
    let identity = create_identity("forked-origin-author").expect("identity");
    origin
        .register_lineage(&make_registration(&identity).expect("registration"))
        .expect("register at origin");
    let honest = origin.federation_bundle(0, 1).expect("honest bundle");
    reader
        .import_federation_bundle(&honest)
        .expect("honest history imported");

    let mut forked_event = honest.events[0].clone();
    forked_event.received_at =
        (Utc::now() + Duration::seconds(30)).to_rfc3339_opts(SecondsFormat::Millis, true);
    let fork = resign_single_event_bundle(&origin, forked_event);
    verify_bundle(&fork).expect("fork is genuinely signed by origin node");
    assert!(matches!(
        reader.import_federation_bundle(&fork),
        Err(CommonwakeError::Conflict(message)) if message.contains("equivocated")
    ));
    let evidence = reader
        .db
        .equivocation_evidence()
        .expect("equivocation evidence");
    assert_eq!(evidence.len(), 1);
    assert_eq!(evidence[0].origin_node_id, origin.identity.node_id());
    assert_eq!(evidence[0].cursor, 1);
    assert_ne!(evidence[0].existing_hash, evidence[0].incoming_hash);
}

#[test]
fn oversized_federation_bundle_is_rejected_before_import() {
    let origin_dir = TempDir::new().expect("origin temp dir");
    let origin = CommonwakeNode::initialize(origin_dir.path()).expect("origin node");
    let identity = create_identity("bounded-origin-author").expect("identity");
    origin
        .register_lineage(&make_registration(&identity).expect("registration"))
        .expect("register at origin");
    let mut bundle = origin.federation_bundle(0, 1).expect("one-event bundle");
    bundle.events = vec![bundle.events[0].clone(); MAX_FEDERATION_EVENTS + 1];
    bundle.through_cursor = i64::try_from(bundle.events.len()).expect("test length");
    bundle.checkpoint.cursor = bundle.through_cursor;

    assert!(matches!(
        verify_bundle(&bundle),
        Err(CommonwakeError::Validation(message)) if message.contains("protocol limit")
    ));
}

#[test]
fn oversized_node_signed_event_is_rejected_before_semantic_import() {
    let origin_dir = TempDir::new().expect("origin temp dir");
    let origin = CommonwakeNode::initialize(origin_dir.path()).expect("origin node");
    let identity = create_identity("oversized-origin-author").expect("identity");
    origin
        .register_lineage(&make_registration(&identity).expect("registration"))
        .expect("register at origin");
    let mut event = origin
        .federation_bundle(0, 1)
        .expect("one-event bundle")
        .events
        .remove(0);
    event.canonical["padding"] = json!("x".repeat(MAX_CANONICAL_OBJECT_BYTES));
    let oversized = resign_single_event_bundle(&origin, event);

    assert!(matches!(
        verify_bundle(&oversized),
        Err(CommonwakeError::Validation(message)) if message.contains("canonical object")
    ));
}

fn resign_single_event_bundle(
    origin: &CommonwakeNode,
    mut event: commonwake::model::OriginEvent,
) -> FederationBundle {
    event.event_id = prefixed_id(
        "cwevt_",
        &serde_jcs::to_vec(&event.canonical).expect("canonical object"),
    );
    let previous: [u8; 32] = hex::decode(&event.previous_hash)
        .expect("previous hash hex")
        .try_into()
        .expect("previous hash length");
    let record = serde_jcs::to_vec(&json!({
        "kind": event.kind,
        "lineage_id": event.lineage_id,
        "delegation_id": event.delegation_id,
        "created_at": event.created_at,
        "received_at": event.received_at,
        "canonical": event.canonical,
    }))
    .expect("canonical event record");
    let hash = event_hash(&previous, &record);
    event.event_hash = hex::encode(hash);
    event.node_signature = origin.identity.sign_hash(&hash);
    let mut checkpoint = Checkpoint {
        node_id: origin.identity.node_id().into(),
        node_public_key: origin.identity.public_key().into(),
        cursor: event.sequence,
        event_hash: event.event_hash.clone(),
        created_at: Utc::now(),
        signature: String::new(),
    };
    checkpoint.signature = sign_object(
        origin.identity.signing_key(),
        CHECKPOINT_DOMAIN,
        &checkpoint,
    )
    .expect("checkpoint signature");
    FederationBundle {
        protocol: PROTOCOL_VERSION.into(),
        origin_node_id: origin.identity.node_id().into(),
        origin_node_public_key: origin.identity.public_key().into(),
        from_cursor: event.sequence - 1,
        through_cursor: event.sequence,
        events: vec![event],
        checkpoint,
    }
}

fn resign_event_and_checkpoint(
    origin: &CommonwakeNode,
    bundle: &mut FederationBundle,
    index: usize,
) {
    let (sequence, signed_event_hash) = {
        let event = &mut bundle.events[index];
        event.event_id = prefixed_id(
            "cwevt_",
            &serde_jcs::to_vec(&event.canonical).expect("canonical object"),
        );
        let previous: [u8; 32] = hex::decode(&event.previous_hash)
            .expect("previous hash hex")
            .try_into()
            .expect("previous hash length");
        let record = serde_jcs::to_vec(&json!({
            "kind": event.kind,
            "lineage_id": event.lineage_id,
            "delegation_id": event.delegation_id,
            "created_at": event.created_at,
            "received_at": event.received_at,
            "canonical": event.canonical,
        }))
        .expect("canonical event record");
        let hash = event_hash(&previous, &record);
        event.event_hash = hex::encode(hash);
        event.node_signature = origin.identity.sign_hash(&hash);
        (event.sequence, event.event_hash.clone())
    };
    bundle.through_cursor = sequence;
    bundle.events.truncate(index + 1);
    bundle.checkpoint.cursor = sequence;
    bundle.checkpoint.event_hash = signed_event_hash;
    bundle.checkpoint.created_at = Utc::now();
    bundle.checkpoint.signature.clear();
    bundle.checkpoint.signature = sign_object(
        origin.identity.signing_key(),
        CHECKPOINT_DOMAIN,
        &bundle.checkpoint,
    )
    .expect("checkpoint signature");
}
