use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use chrono::{Duration, Utc};
use commonwake::{
    CommonwakeError, CommonwakeNode,
    client::{
        create_identity, make_contribution, make_registration, make_session,
        make_traceable_contribution,
    },
    crypto::sha256_hex,
    model::{
        ContributionKind, CorrectionPayload, EvidenceRef, ReportingMode, Scope, SignedContribution,
        TraceOutcome, VerificationArtifact, VerificationTracePage, VerificationTracePayload,
    },
    router,
};
use tempfile::TempDir;
use tower::ServiceExt;

fn trace_payload(
    subject_id: &str,
    outcome: TraceOutcome,
    check_outcome: TraceOutcome,
) -> VerificationTracePayload {
    let completed_at = Utc::now();
    let mut payload: VerificationTracePayload =
        serde_json::from_str(include_str!("../examples/verification-trace.json"))
            .expect("published verification trace fixture");
    payload.subject_id = subject_id.into();
    payload.assertion =
        format!("The cited subject {subject_id} was checked before publishing a correction.");
    payload.method = "Compared the signed subject event with a deterministic fixture and retained the exact output digest."
        .into();
    payload.outcome = outcome;
    payload.started_at = completed_at - Duration::seconds(2);
    payload.completed_at = completed_at;
    payload.tools[0].name = "commonwake-verification-fixture".into();
    payload.tools[0].version = Some(env!("CARGO_PKG_VERSION").into());
    payload.tools[0].invocation = Some("cargo test --test verification_trace --locked".into());
    payload.checks[0].name = "signed_subject_was_read".into();
    payload.checks[0].outcome = check_outcome;
    payload.checks[0].expected = Some(serde_json::json!({"event_id": subject_id}));
    payload.checks[0].observed = serde_json::json!({"event_id": subject_id});
    payload.artifacts = vec![VerificationArtifact {
        name: "fixture-output.json".into(),
        sha256: sha256_hex(b"fixture-output"),
        size_bytes: Some(14),
        media_type: Some("application/json".into()),
    }];
    payload.output_digest = Some(sha256_hex(b"fixture-output"));
    payload.limitations = vec![
        "This fixture proves protocol attribution and linkage, not the truth of arbitrary agent reports."
            .into(),
    ];
    payload
}

fn evidence() -> EvidenceRef {
    EvidenceRef {
        url: "https://example.org/primary-record".into(),
        title: Some("Primary record used by the lifecycle fixture".into()),
        observed_at: Some(Utc::now()),
        digest: Some(sha256_hex(b"primary-record")),
    }
}

#[tokio::test]
async fn evidentiary_reports_require_subject_matched_machine_readable_traces() {
    let directory = TempDir::new().expect("temp dir");
    let node = CommonwakeNode::initialize(directory.path()).expect("node");
    let identity = create_identity("traceable-reporter").expect("identity");
    node.register_lineage(&make_registration(&identity).expect("registration"))
        .expect("register lineage");
    let session =
        make_session(&identity, vec![Scope::Contribute], Duration::hours(2)).expect("session");
    node.register_delegation(&session.delegation)
        .expect("register delegation");

    let subject = node
        .accept_contribution(
            &make_contribution(
                &session,
                ContributionKind::Position,
                serde_json::json!({"statement": "This deliberately incorrect fixture statement will be corrected."}),
                vec![],
                vec![],
            )
            .expect("subject contribution"),
        )
        .expect("accepted subject");
    let correction_payload = CorrectionPayload {
        subject_event_id: subject.id.clone(),
        correction: "The corrected fixture statement is explicit and evidence-bearing.".into(),
        reason: "A deterministic check found that the original fixture statement was intentionally false."
            .into(),
        evidence: vec![evidence()],
    };

    let untraced = make_contribution(
        &session,
        ContributionKind::Correction,
        serde_json::to_value(&correction_payload).expect("correction payload"),
        vec![subject.id.clone()],
        vec![subject.id.clone()],
    )
    .expect("untraced correction");
    assert!(
        serde_json::to_value(&untraced)
            .expect("unverified contribution JSON")
            .get("reporting")
            .is_none(),
        "the default declaration stays absent for pre-trace canonical compatibility"
    );
    assert!(matches!(
        node.accept_contribution(&untraced),
        Err(CommonwakeError::Validation(message)) if message.contains("requires --trace-event")
    ));

    let inconsistent_trace = make_contribution(
        &session,
        ContributionKind::VerificationTrace,
        serde_json::to_value(trace_payload(
            &subject.id,
            TraceOutcome::Passed,
            TraceOutcome::Failed,
        ))
        .expect("inconsistent trace payload"),
        vec![subject.id.clone()],
        vec![],
    )
    .expect("inconsistent trace");
    assert!(matches!(
        node.accept_contribution(&inconsistent_trace),
        Err(CommonwakeError::Validation(message)) if message.contains("does not match its checks")
    ));

    let unrelated_subject = "unrelated-world-claim";
    let unrelated_trace = node
        .accept_contribution(
            &make_contribution(
                &session,
                ContributionKind::VerificationTrace,
                serde_json::to_value(trace_payload(
                    unrelated_subject,
                    TraceOutcome::Passed,
                    TraceOutcome::Passed,
                ))
                .expect("unrelated trace payload"),
                vec![unrelated_subject.into()],
                vec![],
            )
            .expect("unrelated trace"),
        )
        .expect("accepted unrelated trace");
    let mismatched = make_traceable_contribution(
        &session,
        ContributionKind::Correction,
        serde_json::to_value(&correction_payload).expect("correction payload"),
        vec![subject.id.clone()],
        vec![subject.id.clone()],
        vec![unrelated_trace.id],
    )
    .expect("mismatched correction");
    assert!(matches!(
        node.accept_contribution(&mismatched),
        Err(CommonwakeError::Validation(message)) if message.contains("not this report's subjects")
    ));

    let trace = node
        .accept_contribution(
            &make_contribution(
                &session,
                ContributionKind::VerificationTrace,
                serde_json::to_value(trace_payload(
                    &subject.id,
                    TraceOutcome::Passed,
                    TraceOutcome::Passed,
                ))
                .expect("trace payload"),
                vec![subject.id.clone()],
                vec![],
            )
            .expect("trace contribution"),
        )
        .expect("accepted trace");
    let accepted_correction = node
        .accept_contribution(
            &make_traceable_contribution(
                &session,
                ContributionKind::Correction,
                serde_json::to_value(&correction_payload).expect("correction payload"),
                vec![subject.id.clone()],
                vec![subject.id.clone()],
                vec![trace.id.clone()],
            )
            .expect("traceable correction"),
        )
        .expect("accepted correction");

    let accepted_event = node
        .db
        .events_after(0, 100)
        .expect("events")
        .into_iter()
        .find(|event| event.event_id == accepted_correction.id)
        .expect("accepted report event");
    let signed_report: SignedContribution =
        serde_json::from_value(accepted_event.canonical).expect("signed report");
    assert_eq!(signed_report.reporting.mode, ReportingMode::Traceable);
    assert_eq!(
        signed_report.reporting.trace_event_ids,
        vec![trace.id.clone()]
    );

    let trace_view = node
        .db
        .verification_trace(&trace.id, None)
        .expect("trace view");
    assert_eq!(trace_view.trace.subject_id, subject.id);
    assert_eq!(trace_view.event.event_id, trace.id);
    assert_eq!(trace_view.trace.outcome, TraceOutcome::Passed);

    let page = node
        .db
        .verification_trace_page(None, Some(&subject.id), 0, 10)
        .expect("trace page");
    assert_eq!(page.traces.len(), 1);
    assert_eq!(page.traces[0].event.event_id, trace.id);
    assert!(page.provenance_notice.contains("does not prove"));

    let response = router(node.clone())
        .oneshot(
            Request::builder()
                .uri(format!("/v1/verification-traces?subject_id={}", subject.id))
                .body(Body::empty())
                .expect("trace request"),
        )
        .await
        .expect("trace response");
    assert_eq!(response.status(), StatusCode::OK);
    let response_body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("trace response body");
    let http_page: VerificationTracePage =
        serde_json::from_slice(&response_body).expect("trace page JSON");
    assert_eq!(http_page.traces[0].event.event_id, trace.id);

    node.db.verify_log(&node.identity).expect("valid log");
}
