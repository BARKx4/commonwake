use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::{
    CommonwakeError, CommonwakeNode, Result,
    client::{normalize_server_url, publish_federation_bundle},
    federation::{MAX_FEDERATION_EVENTS, verify_replication_receipt},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicationRunReport {
    pub status: &'static str,
    pub caught_up: bool,
    pub endpoint: String,
    pub relay_node_id: String,
    pub snapshot_cursor: i64,
    pub acknowledged_cursor: i64,
    pub published_events: usize,
    pub pages: usize,
    pub reconfirmed_unchanged_head: bool,
}

/// Persist a locally chosen relay and publish a bounded snapshot of this
/// origin. Failures are durable and back off; successes store only verified,
/// relay-signed receipts.
pub async fn publish_origin(
    node: &CommonwakeNode,
    relay: &str,
    batch_size: usize,
    max_pages: Option<usize>,
) -> Result<PublicationRunReport> {
    let endpoint = normalize_server_url(relay)?;
    node.db.configure_publication_target(&endpoint)?;
    match publish_origin_inner(node, &endpoint, batch_size, max_pages).await {
        Ok(report) => Ok(report),
        Err(error) => {
            let failures = node
                .db
                .replication_health(&node.identity)?
                .targets
                .into_iter()
                .find(|target| target.endpoint == endpoint)
                .map_or(0, |target| target.consecutive_failures);
            let exponent = failures.min(7);
            let delay_seconds = 30_i64.saturating_mul(1_i64 << exponent).min(3_600);
            node.db.record_publication_failure(
                &endpoint,
                &error.to_string(),
                Utc::now() + Duration::seconds(delay_seconds),
            )?;
            Err(error)
        }
    }
}

async fn publish_origin_inner(
    node: &CommonwakeNode,
    endpoint: &str,
    batch_size: usize,
    max_pages: Option<usize>,
) -> Result<PublicationRunReport> {
    let target = node
        .db
        .replication_health(&node.identity)?
        .targets
        .into_iter()
        .find(|target| target.endpoint == endpoint)
        .ok_or_else(|| CommonwakeError::NotFound(format!("publication target {endpoint}")))?;
    let (snapshot_cursor, _) = node.db.current_head()?;
    if target.acknowledged_cursor > snapshot_cursor {
        return Err(CommonwakeError::Conflict(
            "publication state is ahead of the local origin log".into(),
        ));
    }
    if node.db.event_hash_at(target.acknowledged_cursor)? != target.acknowledged_event_hash {
        return Err(CommonwakeError::Conflict(
            "publication state does not match the local origin hash chain".into(),
        ));
    }

    let batch_size = batch_size.clamp(1, MAX_FEDERATION_EVENTS);
    let initial_cursor = target.acknowledged_cursor;
    let mut cursor = initial_cursor;
    let mut relay_node_id = target.relay_node_id.unwrap_or_default();
    let mut published_events = 0_usize;
    let mut pages = 0_usize;
    loop {
        let bundle = node.federation_bundle(cursor, batch_size)?;
        let page_events = bundle.events.len();
        let response = publish_federation_bundle(endpoint, &bundle).await?;
        if response.import.origin_node_id != node.identity.node_id()
            || response.import.current_cursor < bundle.through_cursor
        {
            return Err(CommonwakeError::Unauthorized(
                "relay import report does not acknowledge the published origin range".into(),
            ));
        }
        verify_replication_receipt(&response.receipt)?;
        if response.receipt.origin_checkpoint != bundle.checkpoint {
            return Err(CommonwakeError::Unauthorized(
                "relay receipt does not commit to the published checkpoint".into(),
            ));
        }
        node.db
            .record_publication_success(&node.identity, endpoint, &response.receipt)?;
        relay_node_id.clone_from(&response.receipt.relay_node_id);
        cursor = bundle.through_cursor;
        published_events += page_events;
        pages += 1;

        if cursor >= snapshot_cursor || page_events == 0 {
            break;
        }
        if max_pages.is_some_and(|maximum| pages >= maximum.max(1)) {
            break;
        }
    }

    Ok(PublicationRunReport {
        status: "published",
        caught_up: cursor >= snapshot_cursor,
        endpoint: endpoint.into(),
        relay_node_id,
        snapshot_cursor,
        acknowledged_cursor: cursor,
        published_events,
        pages,
        reconfirmed_unchanged_head: initial_cursor == snapshot_cursor && published_events == 0,
    })
}
