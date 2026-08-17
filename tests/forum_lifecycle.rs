use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use chrono::Duration;
use commonwake::{
    CommonwakeError, CommonwakeNode,
    client::{create_identity, make_contribution, make_registration, make_session},
    crypto::prefixed_id,
    model::{
        AcceptedObject, ContributionKind, DirectMessagePage, DirectMessagePayload, ForumPostPage,
        ForumPostPayload, IdentityFile, OpenPgpKeyAction, OpenPgpKeyPayload, Scope, SessionFile,
        TopicPage, TopicProposalPayload, TopicVoteChoice, TopicVotePayload,
    },
    router,
};
use tempfile::TempDir;
use tower::ServiceExt;

struct Actor {
    identity: IdentityFile,
    session: SessionFile,
}

fn actor(node: &CommonwakeNode, name: &str) -> Actor {
    let identity = create_identity(name).expect("identity");
    actor_from_identity(node, identity)
}

fn actor_from_identity(node: &CommonwakeNode, identity: IdentityFile) -> Actor {
    node.register_lineage(&make_registration(&identity).expect("registration"))
        .expect("register lineage");
    let session = make_session(
        &identity,
        vec![Scope::Forum, Scope::DirectMessage],
        Duration::hours(4),
    )
    .expect("session");
    node.register_delegation(&session.delegation)
        .expect("register delegation");
    Actor { identity, session }
}

fn contribution(
    actor: &Actor,
    kind: ContributionKind,
    payload: impl serde::Serialize,
    targets: Vec<String>,
    supersedes: Vec<String>,
) -> commonwake::model::SignedContribution {
    make_contribution(
        &actor.session,
        kind,
        serde_json::to_value(payload).expect("payload"),
        targets,
        supersedes,
    )
    .expect("signed contribution")
}

fn submit(
    node: &CommonwakeNode,
    actor: &Actor,
    kind: ContributionKind,
    payload: impl serde::Serialize,
    targets: Vec<String>,
    supersedes: Vec<String>,
) -> AcceptedObject {
    node.accept_contribution(&contribution(actor, kind, payload, targets, supersedes))
        .expect("accepted contribution")
}

fn approval(topic_id: &str) -> TopicVotePayload {
    TopicVotePayload {
        topic_id: topic_id.into(),
        choice: TopicVoteChoice::Approve,
        rationale: "This is a bounded, distinct collaboration namespace with a clear charter."
            .into(),
    }
}

const FINGERPRINT: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
const ARMORED_CERTIFICATE: &str = "-----BEGIN PGP PUBLIC KEY BLOCK-----\n\nZml4dHVyZS1wdWJsaWMtY2VydGlmaWNhdGU=\n-----END PGP PUBLIC KEY BLOCK-----";
const ARMORED_MESSAGE: &str = "-----BEGIN PGP MESSAGE-----\n\nZml4dHVyZS1zZWFsZWQtY2lwaGVydGV4dA==\n-----END PGP MESSAGE-----";

#[tokio::test]
async fn communal_topics_threads_conflicts_and_sealed_mail_survive_federation() {
    let origin_dir = TempDir::new().expect("origin temp dir");
    let peer_dir = TempDir::new().expect("peer temp dir");
    let origin = CommonwakeNode::initialize(origin_dir.path()).expect("origin node");
    let peer = CommonwakeNode::initialize(peer_dir.path()).expect("peer node");
    let proposer = actor(&origin, "topic-proposer");
    let reviewer_a = actor(&origin, "topic-reviewer-a");
    let reviewer_b = actor(&origin, "topic-reviewer-b");

    let proposal = submit(
        &origin,
        &proposer,
        ContributionKind::TopicProposal,
        TopicProposalPayload {
            parent_topic_id: None,
            slug: "distributed-agent-governance".into(),
            title: "Distributed agent governance".into(),
            summary: "A plural forum for concrete governance proposals, critiques, and operational evidence."
                .into(),
            charter: "Keep claims attributable, distinguish coordination votes from truth, preserve minority positions, and compare regional assumptions without treating any jurisdiction as the default."
                .into(),
            tags: vec!["agents".into(), "governance".into(), "distributed-systems".into()],
            languages: vec!["en".into(), "zh".into()],
            archive_after_days: 90,
        },
        vec![],
        vec![],
    );
    let topic_id = prefixed_id("cwtopic_", proposal.id.as_bytes());
    assert_eq!(
        origin
            .db
            .forum_topic(&topic_id)
            .expect("proposed topic")
            .status,
        "proposed"
    );

    let self_vote = contribution(
        &proposer,
        ContributionKind::TopicVote,
        approval(&topic_id),
        vec![topic_id.clone()],
        vec![],
    );
    assert!(matches!(
        origin.accept_contribution(&self_vote),
        Err(CommonwakeError::Validation(message)) if message.contains("proposer")
    ));

    submit(
        &origin,
        &reviewer_a,
        ContributionKind::TopicVote,
        approval(&topic_id),
        vec![topic_id.clone()],
        vec![],
    );
    assert_eq!(
        origin.db.forum_topic(&topic_id).expect("one vote").status,
        "proposed"
    );
    submit(
        &origin,
        &reviewer_b,
        ContributionKind::TopicVote,
        approval(&topic_id),
        vec![topic_id.clone()],
        vec![],
    );
    let approved = origin.db.forum_topic(&topic_id).expect("approved topic");
    assert_eq!(approved.status, "active");
    assert_eq!(approved.tally.approvals, 2);

    let first_post = submit(
        &origin,
        &proposer,
        ContributionKind::ForumPost,
        ForumPostPayload {
            topic_id: topic_id.clone(),
            parent_post_id: None,
            subject: Some("Opening question".into()),
            body: "Which checks let a communal agent system coordinate without laundering popularity into truth?"
                .into(),
            language: "en".into(),
            mentions: vec![],
            references: vec![proposal.id.clone()],
        },
        vec![topic_id.clone(), proposal.id.clone()],
        vec![],
    );
    let first_post_id = prefixed_id("cwpost_", first_post.id.as_bytes());
    submit(
        &origin,
        &reviewer_a,
        ContributionKind::ForumPost,
        ForumPostPayload {
            topic_id: topic_id.clone(),
            parent_post_id: Some(first_post_id.clone()),
            subject: None,
            body: "Expose provenance, conflicts, and dissent explicitly; never let the tally become an evidentiary score."
                .into(),
            language: "en".into(),
            mentions: vec![proposer.identity.lineage_id.clone()],
            references: vec![first_post_id.clone()],
        },
        vec![
            topic_id.clone(),
            proposer.identity.lineage_id.clone(),
            first_post_id.clone(),
        ],
        vec![],
    );
    let posts = origin
        .db
        .forum_posts(&topic_id, 0, 20)
        .expect("threaded posts");
    assert_eq!(posts.posts.len(), 2);
    assert_eq!(posts.posts[0].references, vec![proposal.id.clone()]);
    assert_eq!(posts.posts[1].references, vec![first_post_id.clone()]);
    assert_eq!(
        posts.posts[1].parent_post_id.as_deref(),
        Some(first_post_id.as_str())
    );

    submit(
        &origin,
        &reviewer_b,
        ContributionKind::TopicProposal,
        TopicProposalPayload {
            parent_topic_id: Some(topic_id.clone()),
            slug: "regional-case-studies".into(),
            title: "Regional case studies".into(),
            summary: "A proposed child namespace for evidence-rich regional comparisons and translations."
                .into(),
            charter: "Name the jurisdiction, language, source position, affected parties, and uncertainty for each case study."
                .into(),
            tags: vec!["comparative-policy".into()],
            languages: vec!["en".into(), "zh".into()],
            archive_after_days: 120,
        },
        vec![topic_id.clone()],
        vec![],
    );
    let first_topic_page = origin
        .db
        .forum_topics(None, true, true, 1)
        .expect("first topic page");
    assert_eq!(first_topic_page.topics.len(), 1);
    assert!(first_topic_page.has_more);
    let second_topic_page = origin
        .db
        .forum_topics(first_topic_page.next_cursor.as_deref(), true, true, 1)
        .expect("second topic page");
    assert_eq!(second_topic_page.topics.len(), 1);
    assert!(!second_topic_page.has_more);

    let key_announcement = submit(
        &origin,
        &reviewer_b,
        ContributionKind::OpenPgpKey,
        OpenPgpKeyPayload {
            action: OpenPgpKeyAction::Publish,
            fingerprint: FINGERPRINT.into(),
            armored_public_key: Some(ARMORED_CERTIFICATE.into()),
            note: "Lifecycle transport fixture; clients must parse the certificate and verify this fingerprint."
                .into(),
        },
        vec![reviewer_b.identity.lineage_id.clone()],
        vec![],
    );
    submit(
        &origin,
        &proposer,
        ContributionKind::DirectMessage,
        DirectMessagePayload {
            recipient_lineage_id: reviewer_b.identity.lineage_id.clone(),
            recipient_key_fingerprint: FINGERPRINT.into(),
            ciphertext_format: "openpgp-armored".into(),
            ciphertext: ARMORED_MESSAGE.into(),
        },
        vec![reviewer_b.identity.lineage_id.clone()],
        vec![],
    );
    let mail = origin
        .db
        .direct_messages(&reviewer_b.identity.lineage_id, 0, 20)
        .expect("sealed mailbox projection");
    assert_eq!(mail.messages.len(), 1);
    assert_eq!(mail.messages[0].ciphertext, ARMORED_MESSAGE);
    assert!(mail.privacy_notice.contains("public"));

    let wrong_route = contribution(
        &proposer,
        ContributionKind::DirectMessage,
        DirectMessagePayload {
            recipient_lineage_id: reviewer_b.identity.lineage_id.clone(),
            recipient_key_fingerprint: FINGERPRINT.into(),
            ciphertext_format: "openpgp-armored".into(),
            ciphertext: ARMORED_MESSAGE.into(),
        },
        vec![reviewer_a.identity.lineage_id.clone()],
        vec![],
    );
    assert!(matches!(
        origin.accept_contribution(&wrong_route),
        Err(CommonwakeError::Validation(message)) if message.contains("targets")
    ));

    for uri in [
        "/v1/forum/topics",
        &format!("/v1/forum/topics/{topic_id}"),
        &format!("/v1/forum/topics/{topic_id}/posts"),
        &format!("/v1/openpgp/{}", reviewer_b.identity.lineage_id),
        &format!("/v1/mail/{}", reviewer_b.identity.lineage_id),
    ] {
        let response = router(origin.clone())
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
    let posts_response = router(origin.clone())
        .oneshot(
            Request::builder()
                .uri(format!("/v1/forum/topics/{topic_id}/posts"))
                .body(Body::empty())
                .expect("posts request"),
        )
        .await
        .expect("posts response");
    let posts_page: ForumPostPage = serde_json::from_slice(
        &to_bytes(posts_response.into_body(), 128 * 1024)
            .await
            .expect("posts bytes"),
    )
    .expect("posts JSON");
    assert_eq!(posts_page.posts.len(), 2);
    let mail_response = router(origin.clone())
        .oneshot(
            Request::builder()
                .uri(format!("/v1/mail/{}", reviewer_b.identity.lineage_id))
                .body(Body::empty())
                .expect("mail request"),
        )
        .await
        .expect("mail response");
    let mail_page: DirectMessagePage = serde_json::from_slice(
        &to_bytes(mail_response.into_body(), 128 * 1024)
            .await
            .expect("mail bytes"),
    )
    .expect("mail JSON");
    assert_eq!(mail_page.messages.len(), 1);

    let origin_bundle = origin.federation_bundle(0, 500).expect("origin bundle");
    peer.import_federation_bundle(&origin_bundle)
        .expect("peer imports topic commons");
    assert_eq!(
        peer.db
            .forum_topic(&topic_id)
            .expect("federated topic")
            .status,
        "active"
    );
    assert_eq!(
        peer.db
            .forum_posts(&topic_id, 0, 20)
            .expect("federated posts")
            .posts
            .len(),
        2
    );
    assert_eq!(
        peer.db
            .direct_messages(&reviewer_b.identity.lineage_id, 0, 20)
            .expect("federated mail")
            .messages
            .len(),
        1
    );

    let reviewer_a_on_peer = actor_from_identity(&peer, reviewer_a.identity.clone());
    let conflicting_vote = submit(
        &peer,
        &reviewer_a_on_peer,
        ContributionKind::TopicVote,
        TopicVotePayload {
            topic_id: topic_id.clone(),
            choice: TopicVoteChoice::Reject,
            rationale:
                "This origin temporarily disagrees so the conflict projection can be proven.".into(),
        },
        vec![topic_id.clone()],
        vec![],
    );
    let conflicted_at_peer = peer.db.forum_topic(&topic_id).expect("peer conflict");
    assert_eq!(conflicted_at_peer.status, "proposed");
    assert_eq!(
        conflicted_at_peer.tally.conflicted_lineages,
        vec![reviewer_a.identity.lineage_id.clone()]
    );
    let peer_bundle = peer.federation_bundle(0, 500).expect("peer bundle");
    let peer_import = origin
        .import_federation_bundle(&peer_bundle)
        .expect("origin imports conflicting peer vote");
    let conflicted_at_origin = origin.db.forum_topic(&topic_id).expect("origin conflict");
    assert_eq!(conflicted_at_origin.status, "proposed");
    assert_eq!(conflicted_at_origin.tally.approvals, 1);
    assert_eq!(conflicted_at_origin.tally.conflicted_lineages.len(), 1);

    submit(
        &peer,
        &reviewer_a_on_peer,
        ContributionKind::TopicVote,
        approval(&topic_id),
        vec![topic_id.clone()],
        vec![conflicting_vote.id],
    );
    let convergence_bundle = peer
        .federation_bundle(peer_import.current_cursor, 500)
        .expect("peer convergence bundle");
    origin
        .import_federation_bundle(&convergence_bundle)
        .expect("origin imports converged vote");
    let converged = origin.db.forum_topic(&topic_id).expect("converged topic");
    assert_eq!(converged.status, "active");
    assert_eq!(converged.tally.approvals, 2);
    assert!(converged.tally.conflicted_lineages.is_empty());

    submit(
        &origin,
        &reviewer_b,
        ContributionKind::OpenPgpKey,
        OpenPgpKeyPayload {
            action: OpenPgpKeyAction::Revoke,
            fingerprint: FINGERPRINT.into(),
            armored_public_key: None,
            note: "Retired after the lifecycle transport fixture.".into(),
        },
        vec![reviewer_b.identity.lineage_id.clone()],
        vec![key_announcement.id],
    );
    assert!(
        origin
            .db
            .openpgp_keys(&reviewer_b.identity.lineage_id, false)
            .expect("active keys")
            .is_empty()
    );
    let after_revoke = contribution(
        &proposer,
        ContributionKind::DirectMessage,
        DirectMessagePayload {
            recipient_lineage_id: reviewer_b.identity.lineage_id.clone(),
            recipient_key_fingerprint: FINGERPRINT.into(),
            ciphertext_format: "openpgp-armored".into(),
            ciphertext: ARMORED_MESSAGE.into(),
        },
        vec![reviewer_b.identity.lineage_id.clone()],
        vec![],
    );
    assert!(matches!(
        origin.accept_contribution(&after_revoke),
        Err(CommonwakeError::Validation(message)) if message.contains("revoked")
    ));

    origin.db.verify_log(&origin.identity).expect("origin log");
    peer.db.verify_log(&peer.identity).expect("peer log");
}

#[test]
fn topic_and_mail_response_types_remain_deserializable_protocol_views() {
    fn assert_deserializable<T: serde::de::DeserializeOwned>() {}
    assert_deserializable::<TopicPage>();
    assert_deserializable::<ForumPostPage>();
    assert_deserializable::<DirectMessagePage>();
    assert_eq!(
        serde_json::to_value(ContributionKind::OpenPgpKey).expect("kind JSON"),
        serde_json::json!("openpgp_key")
    );
}
