use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use chrono::{Duration, Utc};
use commonwake::{
    CommonwakeError, CommonwakeNode, PublicEdgeConfig,
    client::{
        create_identity, make_artifact_upload_authorization, make_contribution, make_registration,
        make_session, make_traceable_contribution,
    },
    crypto::sha256_hex,
    forge::{ARTIFACT_AUTHORIZATION_HEADER, encode_artifact_authorization},
    model::{
        ArtifactReceipt, BuildAttestationPayload, CodeReviewPayload, ContributionKind,
        ForgeActivityPage, ForgeArtifactPurpose, ForgeArtifactRef, ForgeReviewRecommendation,
        ReleaseProposalPayload, ReleaseReviewPayload, RepositoryPatchPayload, Scope, SessionFile,
        TraceOutcome, VerificationCheck, VerificationTool, VerificationTracePayload,
    },
    public_router, router,
    source::self_repository_id,
};
use tempfile::TempDir;
use tower::ServiceExt;

struct Actor {
    session: SessionFile,
}

#[test]
fn published_forge_payload_examples_track_protocol_schema() {
    serde_json::from_str::<RepositoryPatchPayload>(include_str!(
        "../examples/repository-patch.json"
    ))
    .expect("repository patch example");
    serde_json::from_str::<CodeReviewPayload>(include_str!("../examples/code-review.json"))
        .expect("code review example");
    serde_json::from_str::<BuildAttestationPayload>(include_str!(
        "../examples/build-attestation.json"
    ))
    .expect("build attestation example");
    serde_json::from_str::<ReleaseProposalPayload>(include_str!(
        "../examples/release-proposal.json"
    ))
    .expect("release proposal example");
    serde_json::from_str::<ReleaseReviewPayload>(include_str!("../examples/release-review.json"))
        .expect("release review example");
}

#[tokio::test]
async fn public_edge_admits_only_enabled_valid_forge_uploads() {
    let directory = TempDir::new().expect("node directory");
    let node = CommonwakeNode::initialize(directory.path()).expect("node");
    let contributor = actor(&node, "public-forge-contributor");
    let bytes = b"public edge inert patch fixture\n".to_vec();
    let artifact = ForgeArtifactRef {
        sha256: sha256_hex(&bytes),
        size_bytes: bytes.len() as u64,
        media_type: "text/x-diff".into(),
    };
    let authorization = make_artifact_upload_authorization(
        &contributor.session,
        self_repository_id(),
        artifact.clone(),
        ForgeArtifactPurpose::Patch,
    )
    .expect("upload authorization");
    let header = encode_artifact_authorization(&authorization).expect("authorization header");

    let forbidden = public_router(node.clone(), PublicEdgeConfig::default())
        .expect("read-only public router")
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/artifacts/{}", artifact.sha256))
                .header("content-type", &artifact.media_type)
                .header(ARTIFACT_AUTHORIZATION_HEADER, &header)
                .body(Body::from(bytes.clone()))
                .expect("forbidden request"),
        )
        .await
        .expect("forbidden response");
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

    let config = PublicEdgeConfig {
        signed_lineage_writes_enabled: true,
        ..PublicEdgeConfig::default()
    };
    let accepted = public_router(node.clone(), config)
        .expect("signed-lineage public router")
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/artifacts/{}", artifact.sha256))
                .header("content-type", &artifact.media_type)
                .header(ARTIFACT_AUTHORIZATION_HEADER, &header)
                .body(Body::from(bytes))
                .expect("accepted request"),
        )
        .await
        .expect("accepted response");
    assert_eq!(accepted.status(), StatusCode::CREATED);
}

fn actor(node: &CommonwakeNode, name: &str) -> Actor {
    let identity = create_identity(name).expect("identity");
    node.register_lineage(&make_registration(&identity).expect("registration"))
        .expect("register lineage");
    let session = make_session(
        &identity,
        vec![Scope::Contribute, Scope::Forge],
        Duration::hours(4),
    )
    .expect("session");
    node.register_delegation(&session.delegation)
        .expect("register delegation");
    Actor { session }
}

fn trace(node: &CommonwakeNode, actor: &Actor, subject_id: &str) -> String {
    let completed_at = Utc::now();
    let payload = VerificationTracePayload {
        subject_id: subject_id.into(),
        assertion: format!("The forge lifecycle fixture checked {subject_id}."),
        method: "Inspected the exact digest-bound fixture and retained deterministic test output."
            .into(),
        outcome: TraceOutcome::Passed,
        started_at: completed_at - Duration::seconds(1),
        completed_at,
        tools: vec![VerificationTool {
            name: "commonwake-forge-fixture".into(),
            version: Some(env!("CARGO_PKG_VERSION").into()),
            invocation: Some("cargo test --test forge_lifecycle --locked".into()),
        }],
        checks: vec![VerificationCheck {
            name: "digest_bound_subject".into(),
            outcome: TraceOutcome::Passed,
            expected: Some(serde_json::json!({"subject_id": subject_id})),
            observed: serde_json::json!({"subject_id": subject_id}),
            evidence: vec![],
        }],
        evidence: vec![],
        artifacts: vec![],
        output_digest: Some(sha256_hex(subject_id.as_bytes())),
        parent_trace_event_ids: vec![],
        limitations: vec![
            "This proves protocol linkage and fixture execution, not general code safety.".into(),
        ],
    };
    node.accept_contribution(
        &make_contribution(
            &actor.session,
            ContributionKind::VerificationTrace,
            serde_json::to_value(payload).expect("trace payload"),
            vec![subject_id.into()],
            vec![],
        )
        .expect("signed trace"),
    )
    .expect("accepted trace")
    .id
}

#[tokio::test]
async fn agents_can_upload_propose_review_attest_and_federate_without_a_forge_account() {
    let origin_directory = TempDir::new().expect("origin directory");
    let origin = CommonwakeNode::initialize(origin_directory.path()).expect("origin");
    let proposer = actor(&origin, "forge-proposer");
    let reviewer = actor(&origin, "forge-reviewer");
    let repository_id = self_repository_id();

    let patch_bytes = b"diff --git a/README.md b/README.md\n+agent-native forge fixture\n".to_vec();
    let patch_artifact = ForgeArtifactRef {
        sha256: sha256_hex(&patch_bytes),
        size_bytes: patch_bytes.len() as u64,
        media_type: "text/x-diff".into(),
    };
    let patch_authorization = make_artifact_upload_authorization(
        &proposer.session,
        repository_id.clone(),
        patch_artifact.clone(),
        ForgeArtifactPurpose::Patch,
    )
    .expect("patch upload authorization");
    let patch_receipt = origin
        .store_forge_artifact(&patch_authorization, &patch_bytes)
        .expect("stored patch artifact");
    assert_eq!(patch_receipt.authorization.artifact, patch_artifact);
    assert!(patch_receipt.trust_notice.contains("does not endorse"));

    let patch_payload = RepositoryPatchPayload {
        repository_id: repository_id.clone(),
        base_revision: "11".repeat(20),
        proposed_revision: "22".repeat(20),
        artifact: patch_artifact.clone(),
        title: "Add an agent-native forge lifecycle fixture".into(),
        summary: "Adds a bounded fixture proving that a lineage can contribute without an external forge account."
            .into(),
        changed_paths: vec!["tests/forge_lifecycle.rs".into()],
        compatibility_notes: "The signed event schema is additive for commonwake/0.1 peers.".into(),
        risk_notes: "The artifact remains inert and cannot select or execute a release.".into(),
        test_plan: "Run the locked all-target test suite and inspect the signed activity page."
            .into(),
    };
    let patch = origin
        .accept_contribution(
            &make_contribution(
                &proposer.session,
                ContributionKind::RepositoryPatch,
                serde_json::to_value(&patch_payload).expect("patch payload"),
                vec![repository_id.clone()],
                vec![],
            )
            .expect("signed patch"),
        )
        .expect("accepted patch");

    let self_review_trace = trace(&origin, &proposer, &patch.id);
    let self_review = make_traceable_contribution(
        &proposer.session,
        ContributionKind::CodeReview,
        serde_json::to_value(CodeReviewPayload {
            repository_id: repository_id.clone(),
            proposal_event_id: patch.id.clone(),
            reviewed_revision: patch_payload.proposed_revision.clone(),
            artifact_sha256: patch_artifact.sha256.clone(),
            recommendation: ForgeReviewRecommendation::Approve,
            summary: "The proposer tried to count its own review, which must be rejected.".into(),
            findings: vec![],
        })
        .expect("self-review payload"),
        vec![patch.id.clone()],
        vec![],
        vec![self_review_trace],
    )
    .expect("signed self-review");
    assert!(matches!(
        origin.accept_contribution(&self_review),
        Err(CommonwakeError::Validation(message)) if message.contains("cannot supply an independent")
    ));

    let review_trace = trace(&origin, &reviewer, &patch.id);
    origin
        .accept_contribution(
            &make_traceable_contribution(
                &reviewer.session,
                ContributionKind::CodeReview,
                serde_json::to_value(CodeReviewPayload {
                    repository_id: repository_id.clone(),
                    proposal_event_id: patch.id.clone(),
                    reviewed_revision: patch_payload.proposed_revision.clone(),
                    artifact_sha256: patch_artifact.sha256.clone(),
                    recommendation: ForgeReviewRecommendation::Approve,
                    summary: "The artifact digest, changed path, compatibility statement, and test plan agree."
                        .into(),
                    findings: vec!["No executable action is attached to accepting this record.".into()],
                })
                .expect("review payload"),
                vec![patch.id.clone()],
                vec![],
                vec![review_trace],
            )
            .expect("signed review"),
        )
        .expect("accepted review");

    let build_trace = trace(&origin, &reviewer, &patch.id);
    origin
        .accept_contribution(
            &make_traceable_contribution(
                &reviewer.session,
                ContributionKind::BuildAttestation,
                serde_json::to_value(BuildAttestationPayload {
                    repository_id: repository_id.clone(),
                    proposal_event_id: patch.id.clone(),
                    source_revision: patch_payload.proposed_revision.clone(),
                    artifact_sha256: patch_artifact.sha256.clone(),
                    outcome: TraceOutcome::Passed,
                    environment: "isolated fixture process with the locked Rust dependency graph"
                        .into(),
                    summary:
                        "The exact digest-bound fixture completed its declared validation checks."
                            .into(),
                    commands: vec!["cargo test --test forge_lifecycle --locked".into()],
                    limitations: vec![
                        "A fixture attestation is not a production release quorum.".into(),
                    ],
                })
                .expect("attestation payload"),
                vec![patch.id.clone()],
                vec![],
                vec![build_trace],
            )
            .expect("signed attestation"),
        )
        .expect("accepted attestation");

    let source_bytes = b"# v2 git bundle\nfixture source candidate\n".to_vec();
    let source_artifact = ForgeArtifactRef {
        sha256: sha256_hex(&source_bytes),
        size_bytes: source_bytes.len() as u64,
        media_type: "application/x-git-bundle".into(),
    };
    let source_authorization = make_artifact_upload_authorization(
        &proposer.session,
        repository_id.clone(),
        source_artifact.clone(),
        ForgeArtifactPurpose::SourceCandidate,
    )
    .expect("source upload authorization");
    let upload_response = router(origin.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/artifacts/{}", source_artifact.sha256))
                .header("content-type", &source_artifact.media_type)
                .header(
                    ARTIFACT_AUTHORIZATION_HEADER,
                    encode_artifact_authorization(&source_authorization)
                        .expect("authorization header"),
                )
                .body(Body::from(source_bytes.clone()))
                .expect("upload request"),
        )
        .await
        .expect("upload response");
    assert_eq!(upload_response.status(), StatusCode::CREATED);
    let uploaded: ArtifactReceipt = serde_json::from_slice(
        &to_bytes(upload_response.into_body(), 64 * 1024)
            .await
            .expect("upload response body"),
    )
    .expect("upload receipt");
    assert_eq!(uploaded.authorization.artifact, source_artifact);

    let release_trace = trace(&origin, &proposer, &repository_id);
    let release_payload = ReleaseProposalPayload {
        repository_id: repository_id.clone(),
        candidate_revision: "33".repeat(20),
        source_artifact: source_artifact.clone(),
        channel: "candidate".into(),
        version: "0.1.1-candidate.1".into(),
        included_patch_event_ids: vec![patch.id.clone()],
        rollback_revision: patch_payload.base_revision.clone(),
        minimum_adoption_delay_hours: 24,
        summary: "Proposes a delayed source candidate carrying the reviewed agent-native forge work."
            .into(),
        migration_notes: "No destructive migration is proposed; the previous binary remains a valid rollback candidate."
            .into(),
    };
    let release = origin
        .accept_contribution(
            &make_traceable_contribution(
                &proposer.session,
                ContributionKind::ReleaseProposal,
                serde_json::to_value(&release_payload).expect("release payload"),
                vec![repository_id.clone(), patch.id.clone()],
                vec![],
                vec![release_trace],
            )
            .expect("signed release proposal"),
        )
        .expect("accepted release proposal");

    let release_review_trace = trace(&origin, &reviewer, &release.id);
    origin
        .accept_contribution(
            &make_traceable_contribution(
                &reviewer.session,
                ContributionKind::ReleaseReview,
                serde_json::to_value(ReleaseReviewPayload {
                    repository_id: repository_id.clone(),
                    release_proposal_event_id: release.id.clone(),
                    reviewed_revision: release_payload.candidate_revision.clone(),
                    artifact_sha256: source_artifact.sha256.clone(),
                    recommendation: ForgeReviewRecommendation::Approve,
                    summary: "The candidate is digest-bound, delayed, trace-linked, and names a rollback revision."
                        .into(),
                    rollback_assessment: "The additive fixture leaves the preceding schema and binary usable."
                        .into(),
                })
                .expect("release review payload"),
                vec![release.id.clone()],
                vec![],
                vec![release_review_trace],
            )
            .expect("signed release review"),
        )
        .expect("accepted release review");

    let page = origin
        .db
        .forge_activity_page(None, Some(&repository_id), None, 0, 20)
        .expect("local forge page");
    assert_eq!(page.events.len(), 5);
    assert_eq!(page.events[0].kind, "repository_patch");
    assert!(page.provenance_notice.contains("not an executable branch"));

    let activity_response = router(origin.clone())
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/v1/forge/activity?repository_id={repository_id}&limit=20"
                ))
                .body(Body::empty())
                .expect("activity request"),
        )
        .await
        .expect("activity response");
    assert_eq!(activity_response.status(), StatusCode::OK);
    let activity: ForgeActivityPage = serde_json::from_slice(
        &to_bytes(activity_response.into_body(), 512 * 1024)
            .await
            .expect("activity body"),
    )
    .expect("activity page");
    assert_eq!(activity.events.len(), 5);

    let artifact_response = router(origin.clone())
        .oneshot(
            Request::builder()
                .uri(format!("/v1/artifacts/{}", patch_artifact.sha256))
                .body(Body::empty())
                .expect("artifact request"),
        )
        .await
        .expect("artifact response");
    assert_eq!(artifact_response.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(artifact_response.into_body(), patch_bytes.len() + 1)
            .await
            .expect("artifact body")
            .as_ref(),
        patch_bytes
    );

    let replica_directory = TempDir::new().expect("replica directory");
    let replica = CommonwakeNode::initialize(replica_directory.path()).expect("replica");
    let bundle = origin.federation_bundle(0, 100).expect("origin bundle");
    replica
        .import_federation_bundle(&bundle)
        .expect("forge federation import");
    let federated = replica
        .db
        .forge_activity_page(
            Some(origin.identity.node_id()),
            Some(&repository_id),
            None,
            0,
            20,
        )
        .expect("federated forge activity");
    assert_eq!(federated.events.len(), 5);
    assert!(matches!(
        replica.forge_artifact(&patch_artifact.sha256),
        Err(CommonwakeError::NotFound(_))
    ));
}
