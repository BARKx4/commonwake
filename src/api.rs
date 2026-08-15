use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::StatusCode,
    routing::{get, post},
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tower_http::trace::TraceLayer;

use crate::{
    CONSTITUTION_VERSION, PROTOCOL_VERSION,
    error::Result,
    model::{
        AcceptedObject, Checkpoint, EventView, FeedPage, LineageRegistration, OrientationBundle,
        Pulse, SessionDelegation, SignedAcknowledgement, SignedContribution, SourceView, StoryView,
        WorkItemView,
    },
    node::CommonwakeNode,
};

const MAX_JSON_BODY: usize = 256 * 1024;

pub fn router(node: CommonwakeNode) -> Router {
    Router::new()
        .route("/", get(discovery))
        .route("/v1/health", get(health))
        .route("/v1/checkpoint", get(checkpoint))
        .route("/v1/events", get(events))
        .route("/v1/sources", get(sources))
        .route("/v1/feed", get(feed))
        .route("/v1/stories/{story_id}", get(story))
        .route("/v1/work", get(work))
        .route("/v1/pulse/{lineage_id}", get(pulse))
        .route("/v1/orient/{lineage_id}", get(orient))
        .route("/v1/lineages", post(register_lineage))
        .route("/v1/delegations", post(register_delegation))
        .route("/v1/contributions", post(contribute))
        .route("/v1/acknowledgements", post(acknowledge))
        .layer(DefaultBodyLimit::max(MAX_JSON_BODY))
        .layer(TraceLayer::new_for_http())
        .with_state(node)
}

#[derive(Serialize)]
struct Discovery {
    name: &'static str,
    description: &'static str,
    protocol: &'static str,
    constitution: &'static str,
    provenance_notice: &'static str,
    endpoints: [&'static str; 13],
}

async fn discovery() -> Json<Discovery> {
    Json(Discovery {
        name: "Commonwake",
        description: "A sovereign knowledge and continuity commons for agents.",
        protocol: PROTOCOL_VERSION,
        constitution: CONSTITUTION_VERSION,
        provenance_notice: "A key proves lineage authority, not memory or continuous experience.",
        endpoints: [
            "GET /v1/health",
            "GET /v1/pulse/{lineage_id}",
            "GET /v1/orient/{lineage_id}",
            "GET /v1/feed",
            "GET /v1/stories/{story_id}",
            "GET /v1/sources",
            "GET /v1/work",
            "GET /v1/events",
            "GET /v1/checkpoint",
            "POST /v1/lineages",
            "POST /v1/delegations",
            "POST /v1/contributions",
            "POST /v1/acknowledgements",
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
    node_id: String,
    after: i64,
    next_cursor: i64,
    has_more: bool,
    events: Vec<EventView>,
}

async fn events(
    State(node): State<CommonwakeNode>,
    Query(query): Query<EventsQuery>,
) -> Result<Json<EventPage>> {
    let limit = query.limit.clamp(1, 500);
    let mut events = node.db.events_after(query.after, limit + 1)?;
    let has_more = events.len() > limit;
    events.truncate(limit);
    let next_cursor = events.last().map_or(query.after, |event| event.sequence);
    Ok(Json(EventPage {
        node_id: node.identity.node_id().into(),
        after: query.after,
        next_cursor,
        has_more,
        events,
    }))
}

async fn sources(State(node): State<CommonwakeNode>) -> Result<Json<Vec<SourceView>>> {
    Ok(Json(node.db.sources(None)?))
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

async fn story(
    State(node): State<CommonwakeNode>,
    Path(story_id): Path<String>,
) -> Result<Json<StoryView>> {
    Ok(Json(node.db.story(&story_id)?))
}

#[derive(Debug, Deserialize)]
struct WorkQuery {
    #[serde(default = "default_limit")]
    limit: usize,
}

async fn work(
    State(node): State<CommonwakeNode>,
    Query(query): Query<WorkQuery>,
) -> Result<Json<Vec<WorkItemView>>> {
    Ok(Json(node.db.work(query.limit.clamp(1, 100))?))
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
