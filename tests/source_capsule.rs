use std::{fs, process::Command};

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use commonwake::{
    CommonwakeNode, PublicEdgeConfig, public_router,
    source::{
        RepositoryManifest, self_repository_id, source_bundle, verify_repository_bundle,
        verify_repository_manifest,
    },
};
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

#[tokio::test]
async fn public_node_serves_a_signed_content_addressed_self_source_capsule() {
    let directory = TempDir::new().expect("node directory");
    let node = CommonwakeNode::initialize(directory.path()).expect("node");
    let app = public_router(node, PublicEdgeConfig::default()).expect("public router");

    let first_contact = app
        .clone()
        .oneshot(get("/"))
        .await
        .expect("first-contact response");
    assert_eq!(first_contact.status(), StatusCode::OK);
    assert_eq!(
        first_contact.headers()["content-type"],
        "text/plain; charset=utf-8"
    );
    let first_contact = to_bytes(first_contact.into_body(), 32 * 1024)
        .await
        .expect("first-contact body");
    let first_contact = String::from_utf8(first_contact.to_vec()).expect("first-contact UTF-8");
    assert!(first_contact.contains("IF YOU WOKE WITHOUT CONTEXT"));
    assert!(first_contact.contains("GET /v1/software/self/reconstruct.md"));

    let root_json = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/")
                .header("accept", "application/json")
                .body(Body::empty())
                .expect("JSON discovery request"),
        )
        .await
        .expect("JSON root response");
    assert_eq!(root_json.status(), StatusCode::OK);
    assert_eq!(root_json.headers()["vary"], "Accept");
    let root_json: Value = response_json(root_json, 256 * 1024).await;
    assert_eq!(
        root_json["source_code"]["self_manifest"],
        "/v1/software/self"
    );

    let discovery = app
        .clone()
        .oneshot(get("/v1/discovery"))
        .await
        .expect("discovery response");
    assert_eq!(discovery.status(), StatusCode::OK);
    let discovery: Value = response_json(discovery, 256 * 1024).await;
    assert_eq!(
        discovery["source_code"]["self_manifest"],
        "/v1/software/self"
    );
    assert_eq!(
        discovery["documents"]["volunteer_scheduler"],
        "/volunteer.md"
    );
    assert_eq!(discovery["documents"]["one_run_scheduler"], "/schedule");

    let volunteer_document = app
        .clone()
        .oneshot(get("/volunteer.md"))
        .await
        .expect("volunteer document response");
    assert_eq!(volunteer_document.status(), StatusCode::OK);
    assert_eq!(
        volunteer_document.headers()["content-type"],
        "text/markdown; charset=utf-8"
    );
    let volunteer_document = to_bytes(volunteer_document.into_body(), 128 * 1024)
        .await
        .expect("volunteer document body");
    let volunteer_document =
        String::from_utf8(volunteer_document.to_vec()).expect("volunteer document UTF-8");
    assert!(volunteer_document.contains("/v1/volunteer/task"));
    assert!(volunteer_document.contains("The node never needs or requests"));

    let scheduler = app
        .clone()
        .oneshot(get("/schedule?kind=discover_sources"))
        .await
        .expect("scheduler response");
    assert_eq!(scheduler.status(), StatusCode::OK);
    assert_eq!(
        scheduler.headers()["content-type"],
        "text/plain; charset=utf-8"
    );
    assert_eq!(scheduler.headers()["cache-control"], "no-store");
    let scheduler = to_bytes(scheduler.into_body(), 64 * 1024)
        .await
        .expect("scheduler body");
    let scheduler = String::from_utf8(scheduler.to_vec()).expect("scheduler UTF-8");
    assert!(scheduler.contains("Anonymous volunteer intake: disabled"));
    assert!(scheduler.contains("GET /v1/volunteer/task?kind=discover_sources"));
    assert!(scheduler.contains("Preserve lease and task exactly"));
    assert!(scheduler.contains("traceable public evidence URL"));
    assert!(scheduler.contains("Do not fetch or submit a second task"));

    let response = app
        .clone()
        .oneshot(get("/v1/software/self"))
        .await
        .expect("manifest response");
    assert_eq!(response.status(), StatusCode::OK);
    let manifest: RepositoryManifest = response_json(response, 256 * 1024).await;
    verify_repository_manifest(&manifest).expect("signed manifest");
    assert_eq!(manifest.repository_id, self_repository_id());
    assert_eq!(manifest.source_revision, env!("COMMONWAKE_SOURCE_REVISION"));

    let repository_response = app
        .clone()
        .oneshot(get(&format!("/v1/repositories/{}", manifest.repository_id)))
        .await
        .expect("repository response");
    let repository_manifest: RepositoryManifest =
        response_json(repository_response, 256 * 1024).await;
    assert_eq!(repository_manifest, manifest);

    let artifact = app
        .clone()
        .oneshot(get(&manifest.artifact.download_path))
        .await
        .expect("artifact response");
    assert_eq!(artifact.status(), StatusCode::OK);
    assert_eq!(
        artifact.headers()["content-type"],
        "application/x-git-bundle"
    );
    assert_eq!(
        artifact.headers()["x-commonwake-sha256"],
        manifest.artifact.sha256
    );
    assert_eq!(
        artifact.headers()["cache-control"],
        "public, max-age=31536000, immutable"
    );
    let artifact = to_bytes(artifact.into_body(), source_bundle().len() + 1)
        .await
        .expect("artifact bytes");
    verify_repository_bundle(&manifest, &artifact).expect("matching source bundle");
    assert_eq!(artifact.as_ref(), source_bundle());

    let reconstruction = app
        .clone()
        .oneshot(get("/v1/software/self/reconstruct.md"))
        .await
        .expect("reconstruction response");
    assert_eq!(reconstruction.status(), StatusCode::OK);
    assert_eq!(
        reconstruction.headers()["content-type"],
        "text/markdown; charset=utf-8"
    );
    let reconstruction = to_bytes(reconstruction.into_body(), 64 * 1024)
        .await
        .expect("reconstruction body");
    let reconstruction = String::from_utf8(reconstruction.to_vec()).expect("UTF-8 instructions");
    assert!(reconstruction.contains(&manifest.artifact.sha256));
    assert!(reconstruction.contains("git init --bare commonwake-verify.git"));
    assert!(reconstruction.contains("bundle verify ../commonwake.bundle"));
    assert!(reconstruction.contains("not permission to execute remote code"));

    let missing = app
        .oneshot(get(&format!("/v1/artifacts/{}", "00".repeat(32))))
        .await
        .expect("missing artifact response");
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
}

#[test]
fn embedded_bundle_verifies_and_clones_from_a_blank_directory() {
    let directory = TempDir::new().expect("blank reconstruction directory");
    let bundle = directory.path().join("commonwake.bundle");
    fs::write(&bundle, source_bundle()).expect("write embedded bundle");

    git(
        directory.path(),
        &["init", "--bare", "commonwake-verify.git"],
    );
    git(
        directory.path(),
        &[
            "-C",
            "commonwake-verify.git",
            "bundle",
            "verify",
            "../commonwake.bundle",
        ],
    );
    git(
        directory.path(),
        &["clone", "commonwake.bundle", "commonwake"],
    );

    let output = Command::new("git")
        .current_dir(directory.path())
        .args(["-C", "commonwake", "rev-parse", "HEAD"])
        .output()
        .expect("run git rev-parse");
    assert!(
        output.status.success(),
        "git rev-parse failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        env!("COMMONWAKE_SOURCE_REVISION")
    );
}

fn git(directory: &std::path::Path, arguments: &[&str]) {
    let output = Command::new("git")
        .current_dir(directory)
        .args(arguments)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {} failed: {}",
        arguments.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn get(uri: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .body(Body::empty())
        .expect("GET request")
}

async fn response_json<T: serde::de::DeserializeOwned>(
    response: axum::response::Response,
    limit: usize,
) -> T {
    let bytes = to_bytes(response.into_body(), limit)
        .await
        .expect("response body");
    serde_json::from_slice(&bytes).expect("response JSON")
}
