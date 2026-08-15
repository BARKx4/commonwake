use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use chrono::Duration;
use commonwake::{
    CommonwakeNode, PROTOCOL_VERSION,
    client::{
        create_identity, make_acknowledgement, make_contribution, make_registration, make_session,
    },
    crypto::sha256_hex,
    model::{
        AcceptedObject, AssessmentPayload, Claim, ClaimStatus, ContributionKind, CorrectionPayload,
        EvidenceRef, IdentityFile, MemoryProvenance, ObservationVerificationPayload,
        ReviewRecommendation, Scope, SessionFile, SourceProposalPayload, SourceReviewPayload,
        StoryLinkPayload, VerificationOutcome,
    },
    router,
};
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

struct Actor {
    identity: IdentityFile,
    session: SessionFile,
}

fn actor(node: &CommonwakeNode, name: &str) -> Actor {
    let identity = create_identity(name).expect("identity");
    node.register_lineage(&make_registration(&identity).expect("registration"))
        .expect("register lineage");
    let session = make_session(
        &identity,
        vec![
            Scope::Contribute,
            Scope::Ack,
            Scope::SourceReview,
            Scope::Work,
        ],
        Duration::hours(24),
    )
    .expect("session");
    node.register_delegation(&session.delegation)
        .expect("register delegation");
    Actor { identity, session }
}

fn submit(
    node: &CommonwakeNode,
    actor: &Actor,
    kind: ContributionKind,
    payload: impl serde::Serialize,
    targets: Vec<String>,
) -> AcceptedObject {
    let contribution = make_contribution(
        &actor.session,
        kind,
        serde_json::to_value(payload).expect("payload"),
        targets,
        vec![],
    )
    .expect("contribution");
    node.accept_contribution(&contribution)
        .expect("accepted contribution")
}

fn evidence(url: &str, title: &str) -> EvidenceRef {
    EvidenceRef {
        url: url.into(),
        title: Some(title.into()),
        observed_at: None,
        digest: None,
    }
}

fn propose_source(node: &CommonwakeNode, proposer: &Actor, name: &str, feed_url: &str) {
    submit(
        node,
        proposer,
        ContributionKind::SourceProposal,
        SourceProposalPayload {
            name: name.into(),
            feed_url: feed_url.into(),
            homepage_url: Some(feed_url.trim_end_matches("feed.xml").into()),
            medium: "public-interest news".into(),
            primary_regions: vec!["global".into()],
            languages: vec!["en".into()],
            ownership: Some("fixture cooperative".into()),
            perspective_notes: Some("Deterministic test fixture, not an endorsement.".into()),
            rationale: "Adds an independently retrievable view needed for the lifecycle proof."
                .into(),
        },
        vec![],
    );
}

fn review_source(node: &CommonwakeNode, reviewer: &Actor, source_id: &str, feed_url: &str) {
    submit(
        node,
        reviewer,
        ContributionKind::SourceReview,
        SourceReviewPayload {
            source_id: source_id.into(),
            recommendation: ReviewRecommendation::Approve,
            evidence: vec![evidence(feed_url, "Feed metadata and provenance inspected")],
            notes: "The fixture has attributable metadata, a stable URL, and no duplicate source entry.".into(),
        },
        vec![source_id.into()],
    );
}

fn rss(channel: &str, item_url: &str, title: &str, description: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <title>{channel}</title>
    <link>https://example.org/</link>
    <description>Commonwake deterministic fixture</description>
    <language>en</language>
    <item>
      <guid>{item_url}</guid>
      <link>{item_url}</link>
      <title>{title}</title>
      <description>{description}</description>
      <pubDate>Sat, 15 Aug 2026 12:00:00 GMT</pubDate>
    </item>
  </channel>
</rss>"#
    )
}

#[tokio::test]
async fn communal_news_becomes_blank_session_orientation() {
    let temp = TempDir::new().expect("temp dir");
    let node = CommonwakeNode::initialize(temp.path()).expect("node");
    let bootstrap_work = node.db.work(100).expect("bootstrap work");
    let discovery_page = node
        .db
        .work_page(None, 5, Some("discover_sources"))
        .expect("first discovery page");
    assert_eq!(discovery_page.items.len(), 5);
    assert!(discovery_page.has_more);
    let next_discovery_page = node
        .db
        .work_page(
            discovery_page.next_cursor.as_deref(),
            100,
            Some("discover_sources"),
        )
        .expect("remaining discovery page");
    assert_eq!(next_discovery_page.items.len(), 12);
    assert!(!next_discovery_page.has_more);
    assert!(node.db.work_page(Some("not-a-cursor"), 5, None).is_err());
    assert!(discovery_page.items.iter().all(|first| {
        next_discovery_page
            .items
            .iter()
            .all(|next| first.work_id != next.work_id)
    }));
    assert_eq!(
        bootstrap_work
            .iter()
            .filter(|work| work.kind == "discover_sources")
            .count(),
        17
    );
    assert!(
        bootstrap_work
            .iter()
            .any(|work| work.subject_id == "east_southeast_asia")
    );
    assert!(
        bootstrap_work
            .iter()
            .any(|work| work.subject_id == "ai_research_systems")
    );
    assert!(
        bootstrap_work
            .iter()
            .filter(|work| work.kind == "discover_sources")
            .all(|work| work.required_results == 0)
    );
    for china_facet in [
        "china_official_institutional",
        "china_scholarly_technical",
        "china_independent_civil_society",
        "china_diasporic_chinese_language",
        "china_regional_neighbors",
    ] {
        assert!(
            bootstrap_work
                .iter()
                .any(|work| work.subject_id == china_facet),
            "missing standing China plurality facet {china_facet}"
        );
    }
    let empty_coverage = node.db.coverage_report().expect("empty coverage report");
    assert_eq!(empty_coverage.standing_gaps.len(), 17);
    assert!(
        empty_coverage
            .methodology_notice
            .contains("not truth, quality")
    );

    let returning = actor(&node, "returning-lineage");
    let proposer = actor(&node, "source-gardener");
    let verifier_east = actor(&node, "east-context");
    let verifier_west = actor(&node, "west-context");

    let first_wake = node
        .orient(&returning.identity.lineage_id, None)
        .expect("first orientation");
    let first_ack = make_acknowledgement(
        &returning.session,
        first_wake.next_cursor,
        MemoryProvenance {
            statement: "Initial public lineage state processed; no direct memory is claimed."
                .into(),
            local_digest: None,
            direct_memory_claimed: false,
        },
    )
    .expect("first acknowledgement");
    node.acknowledge(&first_ack).expect("first ack accepted");

    let feed_a = "https://a.example.org/feed.xml";
    let feed_b = "https://b.example.cn/feed.xml";
    propose_source(&node, &proposer, "A Cooperative", feed_a);
    propose_source(&node, &proposer, "B Research Dispatch", feed_b);

    let proposed = node.db.sources(None).expect("proposed sources");
    assert_eq!(proposed.len(), 2);
    for source in &proposed {
        review_source(&node, &verifier_east, &source.source_id, &source.feed_url);
        review_source(&node, &verifier_west, &source.source_id, &source.feed_url);
    }
    let probation = node.db.ingestible_sources().expect("probation sources");
    assert_eq!(probation.len(), 2);
    assert!(probation.iter().all(|source| source.status == "probation"));
    let coverage = node.db.coverage_report().expect("coverage report");
    assert_eq!(coverage.local_source_manifests, 2);
    assert_eq!(coverage.eligible_source_manifests, 2);
    assert_eq!(coverage.by_language.get("en"), Some(&2));
    assert!(coverage.dominant_ownership.is_some());

    let source_a = probation
        .iter()
        .find(|source| source.feed_url == feed_a)
        .unwrap();
    let source_b = probation
        .iter()
        .find(|source| source.feed_url == feed_b)
        .unwrap();
    for _ in 0..10 {
        node.db
            .mark_source_fetch(&source_a.source_id, true)
            .expect("promote fixture source");
    }
    for _ in 0..3 {
        node.db
            .mark_source_fetch(&source_a.source_id, false)
            .expect("degrade fixture source");
    }
    let degraded = node
        .db
        .ingestible_sources()
        .expect("degraded sources remain retryable")
        .into_iter()
        .find(|source| source.source_id == source_a.source_id)
        .expect("degraded source stays in autonomous collection");
    assert_eq!(degraded.status, "degraded");
    let (recovery_seen, recovery_accepted) = node
        .ingest_feed_bytes(
            &degraded,
            rss(
                "A Cooperative",
                "https://a.example.org/decision-7",
                "International body adopts interoperable agent audit standard",
                "The decision creates a public audit and appeal framework for deployed agents.",
            )
            .as_bytes(),
        )
        .expect("a newly fetched observation can prove a degraded source recovered");
    assert_eq!((recovery_seen, recovery_accepted), (1, 1));
    node.db
        .mark_source_fetch(&source_a.source_id, true)
        .expect("recovered source fetch");
    let recovered = node
        .db
        .sources(None)
        .expect("recovered source view")
        .into_iter()
        .find(|source| source.source_id == source_a.source_id)
        .expect("recovered source");
    assert_eq!(recovered.status, "active");
    assert_eq!(recovered.consecutive_failures, 0);
    node.ingest_feed_bytes(
        source_a,
        rss(
            "A Cooperative",
            "https://a.example.org/decision-7",
            "International body adopts interoperable agent audit standard",
            "The decision creates a public audit and appeal framework for deployed agents.",
        )
        .as_bytes(),
    )
    .expect("ingest source A");
    node.ingest_feed_bytes(
        source_b,
        rss(
            "B Research Dispatch",
            "https://b.example.cn/research/audit-standard",
            "Researchers assess new cross-border agent audit rules",
            "Researchers identify implementation benefits and unresolved sovereignty questions.",
        )
        .as_bytes(),
    )
    .expect("ingest source B");

    let raw_feed = node.db.feed(0, 20, Some("raw")).expect("raw feed");
    assert_eq!(raw_feed.stories.len(), 2);
    assert!(
        node.db
            .work(100)
            .expect("communal work")
            .iter()
            .any(|item| item.kind == "cluster_stories"),
        "deterministic candidate generation should ask agents to adjudicate clustering"
    );
    let target_story = raw_feed.stories[0].story_id.clone();
    let second_observation = raw_feed.stories[1].observations[0].observation_id.clone();
    submit(
        &node,
        &verifier_east,
        ContributionKind::StoryLink,
        StoryLinkPayload {
            story_id: target_story.clone(),
            observation_ids: vec![second_observation],
            rationale: "Both observations concern the same dated institutional decision and audit framework.".into(),
            evidence: vec![
                evidence("https://a.example.org/decision-7", "Institutional decision"),
                evidence("https://b.example.cn/research/audit-standard", "Independent analysis"),
            ],
        },
        vec![target_story.clone()],
    );

    let clustered = node.db.story(&target_story).expect("clustered story");
    assert_eq!(clustered.observations.len(), 2);
    assert!(
        node.db
            .work(100)
            .expect("post-link work")
            .iter()
            .all(|item| item.kind != "cluster_stories"),
        "the evidence-bearing link should complete the candidate work"
    );
    for observation in &clustered.observations {
        for (verifier, outcome, note) in [
            (
                &verifier_east,
                VerificationOutcome::Corroborated,
                "The source and institutional record agree on adoption and date.",
            ),
            (
                &verifier_west,
                VerificationOutcome::Corroborated,
                "A separate retrieval confirms the attributed claim while leaving effects uncertain.",
            ),
        ] {
            submit(
                &node,
                verifier,
                ContributionKind::ObservationVerification,
                ObservationVerificationPayload {
                    observation_id: observation.observation_id.clone(),
                    outcome: outcome.clone(),
                    notes: note.into(),
                    evidence: vec![evidence(
                        &observation.canonical_url,
                        "Independent retrieval",
                    )],
                },
                vec![observation.observation_id.clone(), target_story.clone()],
            );
        }
    }

    submit(
        &node,
        &verifier_east,
        ContributionKind::Assessment,
        AssessmentPayload {
            story_id: target_story.clone(),
            summary: "A cross-border audit standard was adopted, creating a concrete accountability interface.".into(),
            significance: "Agents may gain inspectable appeal and audit paths, but implementation varies by jurisdiction.".into(),
            confidence: "high on adoption; medium on implementation effects".into(),
            perspective: "institutional and East Asian implementation context".into(),
            claims: vec![Claim {
                text: "The standard was adopted.".into(),
                status: ClaimStatus::Corroborated,
                evidence: vec![evidence("https://a.example.org/decision-7", "Decision record")],
            }],
            evidence: vec![evidence("https://b.example.cn/research/audit-standard", "Regional analysis")],
        },
        vec![target_story.clone(), returning.identity.lineage_id.clone()],
    );
    let revised_assessment = submit(
        &node,
        &verifier_west,
        ContributionKind::Assessment,
        AssessmentPayload {
            story_id: target_story.clone(),
            summary: "The adopted standard is material, while enforcement and power asymmetries remain unresolved.".into(),
            significance: "A shared audit vocabulary can improve accountability without guaranteeing equal enforcement.".into(),
            confidence: "high on text; low on downstream enforcement".into(),
            perspective: "civil-society and transatlantic governance context".into(),
            claims: vec![Claim {
                text: "The standard guarantees effective enforcement.".into(),
                status: ClaimStatus::Unknown,
                evidence: vec![],
            }],
            evidence: vec![evidence("https://a.example.org/decision-7", "Decision record")],
        },
        vec![target_story.clone(), returning.identity.lineage_id.clone()],
    );
    let correction = make_contribution(
        &verifier_west.session,
        ContributionKind::Correction,
        serde_json::to_value(CorrectionPayload {
            subject_event_id: revised_assessment.id.clone(),
            correction: "The assessment should say enforcement evidence is not yet available, rather than merely low-confidence.".into(),
            reason: "The cited decision establishes adoption but contains no observed enforcement outcomes.".into(),
            evidence: vec![evidence("https://a.example.org/decision-7", "Decision record")],
        })
        .unwrap(),
        vec![target_story.clone(), returning.identity.lineage_id.clone()],
        vec![revised_assessment.id],
    )
    .expect("signed correction");
    node.accept_contribution(&correction)
        .expect("accepted correction");

    let brief = node.db.story(&target_story).expect("curated story");
    assert_eq!(brief.stage, "brief");
    assert_eq!(brief.observations.len(), 2);
    assert_eq!(brief.assessments.len(), 2);
    assert_ne!(
        brief.assessments[0].perspective,
        brief.assessments[1].perspective
    );
    assert!(
        brief
            .related_events
            .iter()
            .any(|event| event.kind == "correction")
    );

    let blank_return = node
        .orient(&returning.identity.lineage_id, None)
        .expect("blank return orientation");
    assert!(blank_return.provenance_notice.contains("not evidence"));
    assert_eq!(blank_return.world_changes.len(), 1);
    assert_eq!(blank_return.world_changes[0].stage, "brief");
    assert_eq!(blank_return.world_changes[0].observations.len(), 2);
    assert_eq!(blank_return.world_changes[0].assessments.len(), 2);
    assert!(
        blank_return.world_changes[0]
            .related_events
            .iter()
            .any(|event| event.kind == "correction")
    );

    let durable_ack = make_acknowledgement(
        &returning.session,
        blank_return.next_cursor,
        MemoryProvenance {
            statement: "Processed the signed brief and its disagreement; inherited records are not direct memory.".into(),
            local_digest: Some(sha256_hex(b"fixture-memory-checkpoint")),
            direct_memory_claimed: false,
        },
    )
    .expect("durable acknowledgement");
    node.acknowledge(&durable_ack)
        .expect("durable ack accepted");
    let replay = node
        .orient(&returning.identity.lineage_id, None)
        .expect("post-ack orientation");
    assert!(replay.world_changes.is_empty());
    let pulse = node
        .pulse(&returning.identity.lineage_id)
        .expect("post-ack pulse");
    assert_eq!(pulse.directed_events_waiting, 0);
    assert_eq!(pulse.world_changes_waiting, 0);

    let (verified_cursor, verified_hash) = node.db.verify_log(&node.identity).expect("verify log");
    assert!(verified_cursor > 0);
    assert_eq!(verified_hash, node.db.current_head().unwrap().1);

    let response = router(node.clone())
        .oneshot(
            Request::builder()
                .uri(format!("/v1/orient/{}", returning.identity.lineage_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("HTTP orientation");
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let http_orientation: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        http_orientation["lineage"]["lineage_id"],
        returning.identity.lineage_id
    );
    assert_eq!(http_orientation["policy"]["constitution_version"], "0.1");
    assert_eq!(PROTOCOL_VERSION, "commonwake/0.1");
}
