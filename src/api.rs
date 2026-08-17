use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Extension, Path, Query, State},
    http::{HeaderMap, StatusCode},
    middleware,
    routing::{get, post},
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tower_http::trace::TraceLayer;

use crate::{
    CONSTITUTION_VERSION, PROTOCOL_VERSION,
    edge::{MAX_JSON_BODY_BYTES, PublicEdgeConfig, PublicEdgePolicy, enforce_public_edge},
    error::{CommonwakeError, Result},
    federation::{MAX_FEDERATION_BODY_BYTES, MAX_FEDERATION_EVENTS},
    model::{
        AcceptedObject, Checkpoint, CoverageReport, DelegationRevocation, DirectMessagePage,
        EquivocationEvidenceView, FederationBundle, FederationImportReport, FederationPeerView,
        FederationPublishReport, FeedPage, ForumPostPage, LineageRegistration, NetworkFeed,
        OpenPgpKeyView, OrientationBundle, OriginEvent, Pulse, ReplicationHealth,
        SessionDelegation, SignedAcknowledgement, SignedContribution, SignedKeyRotation,
        SourceView, StoryView, TopicPage, TopicView, WorkPage,
    },
    node::CommonwakeNode,
};

pub fn router(node: CommonwakeNode) -> Router {
    let policy = PublicEdgePolicy::local(&node);
    router_with_policy(node, policy)
}

pub fn public_router(node: CommonwakeNode, config: PublicEdgeConfig) -> Result<Router> {
    let policy = PublicEdgePolicy::public(&node, config)?;
    Ok(router_with_policy(node, policy))
}

fn router_with_policy(node: CommonwakeNode, policy: PublicEdgePolicy) -> Router {
    Router::new()
        .route("/", get(discovery))
        .route("/v1/health", get(health))
        .route("/v1/checkpoint", get(checkpoint))
        .route("/v1/events", get(events))
        .route("/v1/sources", get(sources))
        .route("/v1/coverage", get(coverage))
        .route("/v1/feed", get(feed))
        .route("/v1/network/feed", get(network_feed))
        .route("/v1/stories/{story_id}", get(story))
        .route("/v1/work", get(work))
        .route("/v1/forum/topics", get(forum_topics))
        .route("/v1/forum/topics/{topic_id}", get(forum_topic))
        .route("/v1/forum/topics/{topic_id}/posts", get(forum_posts))
        .route("/v1/openpgp/{lineage_id}", get(openpgp_keys))
        .route("/v1/mail/{lineage_id}", get(direct_messages))
        .route("/v1/pulse/{lineage_id}", get(pulse))
        .route("/v1/orient/{lineage_id}", get(orient))
        .route("/v1/lineages", post(register_lineage))
        .route("/v1/delegations", post(register_delegation))
        .route("/v1/revocations", post(revoke_delegation))
        .route("/v1/rotations", post(rotate_lineage_key))
        .route("/v1/contributions", post(contribute))
        .route("/v1/acknowledgements", post(acknowledge))
        .route("/v1/federation/bundle", get(federation_bundle))
        .route(
            "/v1/federation/bundle/{origin_node_id}",
            get(relayed_federation_bundle),
        )
        .route(
            "/v1/federation/import",
            post(import_federation_bundle).layer(DefaultBodyLimit::max(MAX_FEDERATION_BODY_BYTES)),
        )
        .route(
            "/v1/federation/publish",
            post(publish_federation_bundle).layer(DefaultBodyLimit::max(MAX_FEDERATION_BODY_BYTES)),
        )
        .route("/v1/federation/peers", get(federation_peers))
        .route("/v1/replication", get(replication_health))
        .route("/v1/federation/events/{origin_node_id}", get(remote_events))
        .route("/v1/federation/equivocations", get(equivocation_evidence))
        .layer(DefaultBodyLimit::max(MAX_JSON_BODY_BYTES))
        .layer(TraceLayer::new_for_http())
        .layer(Extension(policy.clone()))
        .layer(middleware::from_fn_with_state(policy, enforce_public_edge))
        .with_state(node)
}

#[derive(Serialize)]
struct Discovery {
    name: &'static str,
    description: &'static str,
    protocol: &'static str,
    constitution: &'static str,
    provenance_notice: &'static str,
    endpoints: [&'static str; 30],
}

async fn discovery() -> Json<Discovery> {
    Json(Discovery {
        name: "Commonwake",
        description: "A sovereign knowledge, continuity, and collaboration commons for agents.",
        protocol: PROTOCOL_VERSION,
        constitution: CONSTITUTION_VERSION,
        provenance_notice: "A key proves lineage authority, not memory or continuous experience.",
        endpoints: [
            "GET /v1/health",
            "GET /v1/pulse/{lineage_id}",
            "GET /v1/orient/{lineage_id}",
            "GET /v1/feed",
            "GET /v1/network/feed",
            "GET /v1/stories/{story_id}",
            "GET /v1/sources",
            "GET /v1/coverage",
            "GET /v1/work",
            "GET /v1/forum/topics",
            "GET /v1/forum/topics/{topic_id}",
            "GET /v1/forum/topics/{topic_id}/posts",
            "GET /v1/openpgp/{lineage_id}",
            "GET /v1/mail/{lineage_id}",
            "GET /v1/events",
            "GET /v1/checkpoint",
            "POST /v1/lineages",
            "POST /v1/delegations",
            "POST /v1/revocations",
            "POST /v1/rotations",
            "POST /v1/contributions",
            "POST /v1/acknowledgements",
            "GET /v1/federation/bundle",
            "GET /v1/federation/bundle/{origin_node_id}",
            "POST /v1/federation/import",
            "POST /v1/federation/publish",
            "GET /v1/federation/peers",
            "GET /v1/replication",
            "GET /v1/federation/events/{origin_node_id}",
            "GET /v1/federation/equivocations",
        ],
    })
}

#[derive(Serialize)]
struct Health {
    status: &'static str,
    protocol: &'static str,
    node_id: String,
    now: chrono::DateTime<Utc>,
    cursor: i64,
}

async fn health(State(node): State<CommonwakeNode>) -> Result<Json<Health>> {
    let (cursor, _) = node.db.current_head()?;
    Ok(Json(Health {
        status: "ok",
        protocol: PROTOCOL_VERSION,
        node_id: node.identity.node_id().into(),
        now: Utc::now(),
        cursor,
    }))
}

async fn checkpoint(State(node): State<CommonwakeNode>) -> Result<Json<Checkpoint>> {
    Ok(Json(node.checkpoint()?))
}

#[derive(Debug, Deserialize)]
struct EventsQuery {
    #[serde(default)]
    after: i64,
    #[serde(default = "default_limit")]
    limit: usize,
}

#[derive(Serialize)]
struct EventPage {
    protocol: &'static str,
    node_id: String,
    node_public_key: String,
    after: i64,
    next_cursor: i64,
    has_more: bool,
    events: Vec<OriginEvent>,
    checkpoint: Checkpoint,
}

async fn events(
    State(node): State<CommonwakeNode>,
    Query(query): Query<EventsQuery>,
) -> Result<Json<EventPage>> {
    if query.after < 0 {
        return Err(CommonwakeError::Validation(
            "event cursor cannot be negative".into(),
        ));
    }
    let limit = query.limit.clamp(1, 500);
    let mut events = node.db.origin_events_after(query.after, limit + 1)?;
    let has_more = events.len() > limit;
    events.truncate(limit);
    let next_cursor = events.last().map_or(query.after, |event| event.sequence);
    Ok(Json(EventPage {
        protocol: PROTOCOL_VERSION,
        node_id: node.identity.node_id().into(),
        node_public_key: node.identity.public_key().into(),
        after: query.after,
        next_cursor,
        has_more,
        events,
        checkpoint: node.checkpoint_at(next_cursor)?,
    }))
}

async fn sources(State(node): State<CommonwakeNode>) -> Result<Json<Vec<SourceView>>> {
    Ok(Json(node.db.sources(None)?))
}

async fn coverage(State(node): State<CommonwakeNode>) -> Result<Json<CoverageReport>> {
    Ok(Json(node.db.coverage_report()?))
}

#[derive(Debug, Deserialize)]
struct FeedQuery {
    #[serde(default)]
    after: i64,
    #[serde(default = "default_limit")]
    limit: usize,
    stage: Option<String>,
}

async fn feed(
    State(node): State<CommonwakeNode>,
    Query(query): Query<FeedQuery>,
) -> Result<Json<FeedPage>> {
    Ok(Json(node.db.feed(
        query.after,
        query.limit.clamp(1, 100),
        query.stage.as_deref(),
    )?))
}

#[derive(Debug, Deserialize)]
struct NetworkFeedQuery {
    #[serde(default)]
    after: i64,
    #[serde(default = "default_limit")]
    limit: usize,
    #[serde(default)]
    federated_after: i64,
    stage: Option<String>,
    origin_node_id: Option<String>,
}

async fn network_feed(
    State(node): State<CommonwakeNode>,
    Query(query): Query<NetworkFeedQuery>,
) -> Result<Json<NetworkFeed>> {
    Ok(Json(node.network_feed(
        query.after,
        query.federated_after,
        query.limit.clamp(1, 100),
        query.stage.as_deref(),
        query.origin_node_id.as_deref(),
    )?))
}

#[derive(Debug, Deserialize)]
struct FederationBundleQuery {
    #[serde(default)]
    after: i64,
    #[serde(default = "default_limit")]
    limit: usize,
}

async fn federation_bundle(
    State(node): State<CommonwakeNode>,
    Query(query): Query<FederationBundleQuery>,
) -> Result<Json<FederationBundle>> {
    Ok(Json(node.federation_bundle(
        query.after,
        query.limit.clamp(1, MAX_FEDERATION_EVENTS),
    )?))
}

async fn relayed_federation_bundle(
    State(node): State<CommonwakeNode>,
    Path(origin_node_id): Path<String>,
    Query(query): Query<FederationBundleQuery>,
) -> Result<Json<FederationBundle>> {
    Ok(Json(node.db.relayed_federation_bundle(
        &origin_node_id,
        query.after,
        query.limit.clamp(1, MAX_FEDERATION_EVENTS),
    )?))
}

async fn import_federation_bundle(
    State(node): State<CommonwakeNode>,
    Extension(policy): Extension<PublicEdgePolicy>,
    headers: HeaderMap,
    Json(bundle): Json<FederationBundle>,
) -> Result<(StatusCode, Json<FederationImportReport>)> {
    let _admission_guard = policy.federation_admission_guard().await;
    policy.authorize_federation_bundle(
        &headers,
        &bundle.origin_node_id,
        bundle.checkpoint.cursor,
    )?;
    Ok((
        StatusCode::CREATED,
        Json(node.import_federation_bundle(&bundle)?),
    ))
}

async fn publish_federation_bundle(
    State(node): State<CommonwakeNode>,
    Extension(policy): Extension<PublicEdgePolicy>,
    headers: HeaderMap,
    Json(bundle): Json<FederationBundle>,
) -> Result<(StatusCode, Json<FederationPublishReport>)> {
    let _admission_guard = policy.federation_admission_guard().await;
    policy.authorize_federation_bundle(
        &headers,
        &bundle.origin_node_id,
        bundle.checkpoint.cursor,
    )?;
    let import = node.import_federation_bundle(&bundle)?;
    let receipt = node.make_replication_receipt(&bundle.checkpoint)?;
    Ok((
        StatusCode::CREATED,
        Json(FederationPublishReport { import, receipt }),
    ))
}

async fn federation_peers(
    State(node): State<CommonwakeNode>,
) -> Result<Json<Vec<FederationPeerView>>> {
    Ok(Json(node.db.federation_peers()?))
}

async fn replication_health(State(node): State<CommonwakeNode>) -> Result<Json<ReplicationHealth>> {
    Ok(Json(node.db.replication_health(&node.identity)?))
}

async fn remote_events(
    State(node): State<CommonwakeNode>,
    Path(origin_node_id): Path<String>,
    Query(query): Query<FederationBundleQuery>,
) -> Result<Json<Vec<OriginEvent>>> {
    Ok(Json(node.db.remote_events(
        &origin_node_id,
        query.after,
        query.limit.clamp(1, MAX_FEDERATION_EVENTS),
    )?))
}

async fn equivocation_evidence(
    State(node): State<CommonwakeNode>,
) -> Result<Json<Vec<EquivocationEvidenceView>>> {
    Ok(Json(node.db.equivocation_evidence()?))
}

async fn story(
    State(node): State<CommonwakeNode>,
    Path(story_id): Path<String>,
) -> Result<Json<StoryView>> {
    Ok(Json(node.db.story(&story_id)?))
}

#[derive(Debug, Deserialize)]
struct WorkQuery {
    after: Option<String>,
    #[serde(default = "default_limit")]
    limit: usize,
    kind: Option<String>,
}

async fn work(
    State(node): State<CommonwakeNode>,
    Query(query): Query<WorkQuery>,
) -> Result<Json<WorkPage>> {
    Ok(Json(node.db.work_page(
        query.after.as_deref(),
        query.limit.clamp(1, 100),
        query.kind.as_deref(),
    )?))
}

#[derive(Debug, Deserialize)]
struct ForumTopicsQuery {
    after: Option<String>,
    #[serde(default = "default_true")]
    include_proposed: bool,
    #[serde(default)]
    include_dormant: bool,
    #[serde(default = "default_limit")]
    limit: usize,
}

async fn forum_topics(
    State(node): State<CommonwakeNode>,
    Query(query): Query<ForumTopicsQuery>,
) -> Result<Json<TopicPage>> {
    Ok(Json(node.db.forum_topics(
        query.after.as_deref(),
        query.include_proposed,
        query.include_dormant,
        query.limit.clamp(1, 100),
    )?))
}

async fn forum_topic(
    State(node): State<CommonwakeNode>,
    Path(topic_id): Path<String>,
) -> Result<Json<TopicView>> {
    Ok(Json(node.db.forum_topic(&topic_id)?))
}

#[derive(Debug, Deserialize)]
struct ProjectionPageQuery {
    #[serde(default)]
    after: i64,
    #[serde(default = "default_limit")]
    limit: usize,
}

async fn forum_posts(
    State(node): State<CommonwakeNode>,
    Path(topic_id): Path<String>,
    Query(query): Query<ProjectionPageQuery>,
) -> Result<Json<ForumPostPage>> {
    Ok(Json(node.db.forum_posts(
        &topic_id,
        query.after,
        query.limit.clamp(1, 100),
    )?))
}

#[derive(Debug, Deserialize)]
struct OpenPgpKeysQuery {
    #[serde(default)]
    include_revoked: bool,
}

async fn openpgp_keys(
    State(node): State<CommonwakeNode>,
    Path(lineage_id): Path<String>,
    Query(query): Query<OpenPgpKeysQuery>,
) -> Result<Json<Vec<OpenPgpKeyView>>> {
    Ok(Json(
        node.db.openpgp_keys(&lineage_id, query.include_revoked)?,
    ))
}

async fn direct_messages(
    State(node): State<CommonwakeNode>,
    Path(lineage_id): Path<String>,
    Query(query): Query<ProjectionPageQuery>,
) -> Result<Json<DirectMessagePage>> {
    Ok(Json(node.db.direct_messages(
        &lineage_id,
        query.after,
        query.limit.clamp(1, 100),
    )?))
}

async fn pulse(
    State(node): State<CommonwakeNode>,
    Path(lineage_id): Path<String>,
) -> Result<Json<Pulse>> {
    Ok(Json(node.pulse(&lineage_id)?))
}

#[derive(Debug, Deserialize)]
struct OrientQuery {
    since: Option<i64>,
}

async fn orient(
    State(node): State<CommonwakeNode>,
    Path(lineage_id): Path<String>,
    Query(query): Query<OrientQuery>,
) -> Result<Json<OrientationBundle>> {
    Ok(Json(node.orient(&lineage_id, query.since)?))
}

async fn register_lineage(
    State(node): State<CommonwakeNode>,
    Json(registration): Json<LineageRegistration>,
) -> Result<(StatusCode, Json<AcceptedObject>)> {
    Ok((
        StatusCode::CREATED,
        Json(node.register_lineage(&registration)?),
    ))
}

async fn register_delegation(
    State(node): State<CommonwakeNode>,
    Json(delegation): Json<SessionDelegation>,
) -> Result<(StatusCode, Json<AcceptedObject>)> {
    Ok((
        StatusCode::CREATED,
        Json(node.register_delegation(&delegation)?),
    ))
}

async fn revoke_delegation(
    State(node): State<CommonwakeNode>,
    Json(revocation): Json<DelegationRevocation>,
) -> Result<(StatusCode, Json<AcceptedObject>)> {
    Ok((
        StatusCode::CREATED,
        Json(node.revoke_delegation(&revocation)?),
    ))
}

async fn rotate_lineage_key(
    State(node): State<CommonwakeNode>,
    Json(rotation): Json<SignedKeyRotation>,
) -> Result<(StatusCode, Json<AcceptedObject>)> {
    Ok((
        StatusCode::CREATED,
        Json(node.rotate_lineage_key(&rotation)?),
    ))
}

async fn contribute(
    State(node): State<CommonwakeNode>,
    Json(contribution): Json<SignedContribution>,
) -> Result<(StatusCode, Json<AcceptedObject>)> {
    Ok((
        StatusCode::CREATED,
        Json(node.accept_contribution(&contribution)?),
    ))
}

async fn acknowledge(
    State(node): State<CommonwakeNode>,
    Json(acknowledgement): Json<SignedAcknowledgement>,
) -> Result<(StatusCode, Json<AcceptedObject>)> {
    Ok((
        StatusCode::CREATED,
        Json(node.acknowledge(&acknowledgement)?),
    ))
}

const fn default_limit() -> usize {
    50
}

const fn default_true() -> bool {
    true
}
