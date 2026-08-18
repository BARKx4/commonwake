use std::{
    collections::BTreeSet,
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{
    Json,
    extract::{Request, State},
    http::{
        HeaderMap, HeaderValue, Method, StatusCode,
        header::{AUTHORIZATION, CONTENT_LENGTH, RETRY_AFTER},
    },
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard, OwnedSemaphorePermit, Semaphore};

use crate::{
    CommonwakeNode,
    db::Database,
    error::{CommonwakeError, Result},
    federation::MAX_FEDERATION_BODY_BYTES,
};

pub const DEFAULT_PUBLIC_REQUESTS_PER_SECOND: u32 = 100;
pub const DEFAULT_PUBLIC_WRITES_PER_MINUTE: u32 = 60;
pub const DEFAULT_PUBLIC_MAX_CONCURRENCY: usize = 64;
pub const DEFAULT_PUBLIC_MAX_FEDERATION_CONCURRENCY: usize = 2;
pub const DEFAULT_PUBLIC_MAX_STORAGE_BYTES: u64 = 20 * 1024 * 1024 * 1024;
pub const DEFAULT_PUBLIC_MAX_ORIGINS: u64 = 256;
pub const DEFAULT_PUBLIC_MAX_ORIGIN_EVENTS: i64 = 25_000;
pub const DEFAULT_PUBLIC_VOLUNTEER_WRITES_PER_HOUR: u32 = 12;
pub const DEFAULT_PUBLIC_MAX_VOLUNTEER_SUBMISSIONS: u64 = 100_000;
pub(crate) const MAX_JSON_BODY_BYTES: usize = 256 * 1024;

const MAX_STORAGE_WALK_ENTRIES: usize = 16_384;

pub struct PublicEdgeConfig {
    pub write_token: Option<String>,
    pub signed_lineage_writes_enabled: bool,
    pub allowed_publishers: BTreeSet<String>,
    pub requests_per_second: u32,
    pub writes_per_minute: u32,
    pub max_concurrency: usize,
    pub max_federation_concurrency: usize,
    pub max_storage_bytes: u64,
    pub max_origins: u64,
    pub max_origin_events: i64,
    pub volunteer_intake_enabled: bool,
    pub volunteer_writes_per_hour: u32,
    pub max_volunteer_submissions: u64,
}

impl Default for PublicEdgeConfig {
    fn default() -> Self {
        Self {
            write_token: None,
            signed_lineage_writes_enabled: false,
            allowed_publishers: BTreeSet::new(),
            requests_per_second: DEFAULT_PUBLIC_REQUESTS_PER_SECOND,
            writes_per_minute: DEFAULT_PUBLIC_WRITES_PER_MINUTE,
            max_concurrency: DEFAULT_PUBLIC_MAX_CONCURRENCY,
            max_federation_concurrency: DEFAULT_PUBLIC_MAX_FEDERATION_CONCURRENCY,
            max_storage_bytes: DEFAULT_PUBLIC_MAX_STORAGE_BYTES,
            max_origins: DEFAULT_PUBLIC_MAX_ORIGINS,
            max_origin_events: DEFAULT_PUBLIC_MAX_ORIGIN_EVENTS,
            volunteer_intake_enabled: false,
            volunteer_writes_per_hour: DEFAULT_PUBLIC_VOLUNTEER_WRITES_PER_HOUR,
            max_volunteer_submissions: DEFAULT_PUBLIC_MAX_VOLUNTEER_SUBMISSIONS,
        }
    }
}

#[derive(Clone)]
pub struct PublicEdgePolicy {
    inner: Arc<PublicEdgeInner>,
}

struct PublicEdgeInner {
    enabled: bool,
    write_token_hash: Option<[u8; 32]>,
    signed_lineage_writes_enabled: bool,
    allowed_publishers: BTreeSet<String>,
    requests_per_second: u32,
    writes_per_minute: u32,
    max_storage_bytes: u64,
    max_origins: u64,
    max_origin_events: i64,
    volunteer_intake_enabled: bool,
    volunteer_writes_per_hour: u32,
    max_volunteer_submissions: u64,
    data_dir: PathBuf,
    db: Arc<Database>,
    concurrency: Arc<Semaphore>,
    federation_concurrency: Arc<Semaphore>,
    federation_admission: Arc<AsyncMutex<()>>,
    volunteer_admission: Arc<AsyncMutex<()>>,
    windows: Mutex<RateWindows>,
}

struct RateWindows {
    second_started: Instant,
    requests_this_second: u32,
    minute_started: Instant,
    writes_this_minute: u32,
    hour_started: Instant,
    volunteer_writes_this_hour: u32,
}

impl PublicEdgePolicy {
    pub fn local(node: &CommonwakeNode) -> Self {
        Self {
            inner: Arc::new(PublicEdgeInner {
                enabled: false,
                write_token_hash: None,
                signed_lineage_writes_enabled: true,
                allowed_publishers: BTreeSet::new(),
                requests_per_second: u32::MAX,
                writes_per_minute: u32::MAX,
                max_storage_bytes: u64::MAX,
                max_origins: u64::MAX,
                max_origin_events: i64::MAX,
                volunteer_intake_enabled: true,
                volunteer_writes_per_hour: u32::MAX,
                max_volunteer_submissions: u64::MAX,
                data_dir: node.data_dir.as_ref().clone(),
                db: node.db.clone(),
                // The local policy bypasses the edge middleware before acquiring a permit.
                // Keep this allocation deliberately small instead of asking Tokio for an
                // implementation-dependent "unlimited" semaphore.
                concurrency: Arc::new(Semaphore::new(1)),
                federation_concurrency: Arc::new(Semaphore::new(1)),
                federation_admission: Arc::new(AsyncMutex::new(())),
                volunteer_admission: Arc::new(AsyncMutex::new(())),
                windows: Mutex::new(RateWindows::new()),
            }),
        }
    }

    pub fn public(node: &CommonwakeNode, config: PublicEdgeConfig) -> Result<Self> {
        validate_config(&config)?;
        let write_token_hash = config
            .write_token
            .as_deref()
            .map(|token| Sha256::digest(token.as_bytes()).into());
        Ok(Self {
            inner: Arc::new(PublicEdgeInner {
                enabled: true,
                write_token_hash,
                signed_lineage_writes_enabled: config.signed_lineage_writes_enabled,
                allowed_publishers: config.allowed_publishers,
                requests_per_second: config.requests_per_second,
                writes_per_minute: config.writes_per_minute,
                max_storage_bytes: config.max_storage_bytes,
                max_origins: config.max_origins,
                max_origin_events: config.max_origin_events,
                volunteer_intake_enabled: config.volunteer_intake_enabled,
                volunteer_writes_per_hour: config.volunteer_writes_per_hour,
                max_volunteer_submissions: config.max_volunteer_submissions,
                data_dir: node.data_dir.as_ref().clone(),
                db: node.db.clone(),
                concurrency: Arc::new(Semaphore::new(config.max_concurrency)),
                federation_concurrency: Arc::new(Semaphore::new(config.max_federation_concurrency)),
                federation_admission: Arc::new(AsyncMutex::new(())),
                volunteer_admission: Arc::new(AsyncMutex::new(())),
                windows: Mutex::new(RateWindows::new()),
            }),
        })
    }

    pub fn write_mode(&self) -> &'static str {
        match (
            self.inner.enabled,
            self.inner.signed_lineage_writes_enabled,
            self.inner.write_token_hash.is_some(),
            self.inner.allowed_publishers.is_empty(),
        ) {
            (false, _, _, _) => "local-open",
            (true, false, false, true) => "read-only",
            (true, false, false, false) => "admitted-publishers",
            (true, false, true, true) => "bearer-admitted",
            (true, false, true, false) => "bearer-and-publisher-admitted",
            (true, true, false, true) => "registered-lineage-signed",
            (true, true, false, false) => "registered-lineage-and-publisher-admitted",
            (true, true, true, true) => "registered-lineage-and-bearer-admitted",
            (true, true, true, false) => "registered-lineage-bearer-and-publisher-admitted",
        }
    }

    pub fn signed_lineage_writes_enabled(&self) -> bool {
        !self.inner.enabled || self.inner.signed_lineage_writes_enabled
    }

    pub fn bearer_writes_enabled(&self) -> bool {
        !self.inner.enabled || self.inner.write_token_hash.is_some()
    }

    pub fn allowed_publisher_count(&self) -> usize {
        self.inner.allowed_publishers.len()
    }

    pub fn volunteer_intake_mode(&self) -> &'static str {
        if self.inner.enabled && !self.inner.volunteer_intake_enabled {
            "disabled"
        } else if self.inner.enabled {
            "public-probationary"
        } else {
            "local-open"
        }
    }

    pub fn authorize_federation_bundle(
        &self,
        headers: &HeaderMap,
        origin_node_id: &str,
        through_cursor: i64,
    ) -> Result<()> {
        if !self.inner.enabled {
            return Ok(());
        }
        if !self.has_valid_write_token(headers)
            && !self.inner.allowed_publishers.contains(origin_node_id)
        {
            return Err(CommonwakeError::Forbidden(
                "this public relay has not admitted that origin".into(),
            ));
        }
        if through_cursor < 0 || through_cursor > self.inner.max_origin_events {
            return Err(CommonwakeError::ResourceExhausted(format!(
                "origin cursor exceeds this relay's {} event retention quota",
                self.inner.max_origin_events
            )));
        }
        if !self.inner.db.has_federation_origin(origin_node_id)?
            && self.inner.db.federation_origin_count()? >= self.inner.max_origins
        {
            return Err(CommonwakeError::ResourceExhausted(format!(
                "relay has reached its {} admitted-origin quota",
                self.inner.max_origins
            )));
        }
        Ok(())
    }

    fn has_valid_write_token(&self, headers: &HeaderMap) -> bool {
        let Some(expected) = self.inner.write_token_hash else {
            return false;
        };
        let Some(value) = headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
        else {
            return false;
        };
        let Some((scheme, token)) = value.split_once(' ') else {
            return false;
        };
        if !scheme.eq_ignore_ascii_case("bearer") || token.is_empty() {
            return false;
        }
        let actual: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        constant_time_eq(&actual, &expected)
    }

    fn try_acquire(&self) -> Option<OwnedSemaphorePermit> {
        self.inner.concurrency.clone().try_acquire_owned().ok()
    }

    fn try_acquire_federation(&self) -> Option<OwnedSemaphorePermit> {
        self.inner
            .federation_concurrency
            .clone()
            .try_acquire_owned()
            .ok()
    }

    pub async fn federation_admission_guard(&self) -> Option<OwnedMutexGuard<()>> {
        if !self.inner.enabled {
            return None;
        }
        Some(self.inner.federation_admission.clone().lock_owned().await)
    }

    pub fn ensure_volunteer_intake(&self) -> Result<()> {
        if self.inner.enabled && !self.inner.volunteer_intake_enabled {
            return Err(CommonwakeError::Forbidden(
                "anonymous volunteer intake is not enabled on this public relay".into(),
            ));
        }
        Ok(())
    }

    pub fn authorize_volunteer_submission(&self) -> Result<()> {
        self.authorize_volunteer_task()
    }

    pub fn authorize_volunteer_task(&self) -> Result<()> {
        self.ensure_volunteer_intake()?;
        if self.inner.enabled
            && self.inner.db.volunteer_submission_count()? >= self.inner.max_volunteer_submissions
        {
            return Err(CommonwakeError::ResourceExhausted(format!(
                "relay has reached its {} probationary volunteer-submission quota",
                self.inner.max_volunteer_submissions
            )));
        }
        Ok(())
    }

    pub async fn volunteer_admission_guard(&self) -> Option<OwnedMutexGuard<()>> {
        if !self.inner.enabled {
            return None;
        }
        Some(self.inner.volunteer_admission.clone().lock_owned().await)
    }

    fn check_rate(
        &self,
        write: bool,
        volunteer_write: bool,
    ) -> std::result::Result<(), RateRejection> {
        let Ok(mut windows) = self.inner.windows.lock() else {
            return Err(RateRejection::Unavailable);
        };
        windows.check(
            Instant::now(),
            write,
            volunteer_write,
            self.inner.requests_per_second,
            self.inner.writes_per_minute,
            self.inner.volunteer_writes_per_hour,
        )
    }

    fn ensure_storage_headroom(&self, request: &Request) -> Result<()> {
        let current = directory_size(&self.inner.data_dir)?;
        let route_limit = if matches!(
            request.uri().path(),
            "/v1/federation/import" | "/v1/federation/publish"
        ) {
            MAX_FEDERATION_BODY_BYTES
        } else {
            MAX_JSON_BODY_BYTES
        };
        let claimed = request
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(route_limit as u64)
            .min(route_limit as u64);
        if current.saturating_add(claimed) > self.inner.max_storage_bytes {
            return Err(CommonwakeError::ResourceExhausted(
                "relay storage headroom is exhausted; writes are paused while reads remain available"
                    .into(),
            ));
        }
        Ok(())
    }
}

impl RateWindows {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            second_started: now,
            requests_this_second: 0,
            minute_started: now,
            writes_this_minute: 0,
            hour_started: now,
            volunteer_writes_this_hour: 0,
        }
    }

    fn check(
        &mut self,
        now: Instant,
        write: bool,
        volunteer_write: bool,
        requests_per_second: u32,
        writes_per_minute: u32,
        volunteer_writes_per_hour: u32,
    ) -> std::result::Result<(), RateRejection> {
        if now.duration_since(self.second_started) >= Duration::from_secs(1) {
            self.second_started = now;
            self.requests_this_second = 0;
        }
        if now.duration_since(self.minute_started) >= Duration::from_secs(60) {
            self.minute_started = now;
            self.writes_this_minute = 0;
        }
        if now.duration_since(self.hour_started) >= Duration::from_secs(60 * 60) {
            self.hour_started = now;
            self.volunteer_writes_this_hour = 0;
        }
        if self.requests_this_second >= requests_per_second {
            return Err(RateRejection::Requests);
        }
        if write && self.writes_this_minute >= writes_per_minute {
            return Err(RateRejection::Writes);
        }
        if volunteer_write && self.volunteer_writes_this_hour >= volunteer_writes_per_hour {
            return Err(RateRejection::VolunteerWrites);
        }

        self.requests_this_second += 1;
        if write {
            self.writes_this_minute += 1;
        }
        if volunteer_write {
            self.volunteer_writes_this_hour += 1;
        }
        Ok(())
    }
}

enum RateRejection {
    Requests,
    Writes,
    VolunteerWrites,
    Unavailable,
}

pub async fn enforce_public_edge(
    State(policy): State<PublicEdgePolicy>,
    request: Request,
    next: Next,
) -> Response {
    if !policy.inner.enabled {
        return next.run(request).await;
    }

    let Some(_permit) = policy.try_acquire() else {
        return edge_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "concurrency_limit",
            "the public edge is at its bounded concurrency limit",
            Some(1),
        );
    };
    let write = !matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    );
    let volunteer_write = write && request.uri().path() == "/v1/volunteer/results";
    let federation_write = write
        && matches!(
            request.uri().path(),
            "/v1/federation/import" | "/v1/federation/publish"
        );
    let _federation_permit = if federation_write {
        let Some(permit) = policy.try_acquire_federation() else {
            return edge_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "federation_concurrency_limit",
                "the public edge is at its bounded federation verification limit",
                Some(1),
            );
        };
        Some(permit)
    } else {
        None
    };
    if let Err(rejection) = policy.check_rate(write, volunteer_write) {
        return match rejection {
            RateRejection::Requests => edge_error(
                StatusCode::TOO_MANY_REQUESTS,
                "request_rate_limit",
                "the public edge request budget is temporarily exhausted",
                Some(1),
            ),
            RateRejection::Writes => edge_error(
                StatusCode::TOO_MANY_REQUESTS,
                "write_rate_limit",
                "the public edge write budget is temporarily exhausted",
                Some(60),
            ),
            RateRejection::VolunteerWrites => edge_error(
                StatusCode::TOO_MANY_REQUESTS,
                "volunteer_write_rate_limit",
                "the relay's anonymous volunteer-result budget is temporarily exhausted",
                Some(60 * 60),
            ),
            RateRejection::Unavailable => edge_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "limiter_unavailable",
                "the public edge limiter could not safely account for this request",
                Some(1),
            ),
        };
    }

    if write {
        if volunteer_write && !policy.inner.volunteer_intake_enabled {
            return edge_error(
                StatusCode::FORBIDDEN,
                "volunteer_intake_disabled",
                "anonymous volunteer intake is not enabled on this public relay",
                None,
            );
        }
        let signed_lineage_write = policy.signed_lineage_writes_enabled()
            && matches!(
                request.uri().path(),
                "/v1/delegations"
                    | "/v1/revocations"
                    | "/v1/rotations"
                    | "/v1/contributions"
                    | "/v1/acknowledgements"
            );
        if !volunteer_write
            && request.uri().path() != "/v1/federation/publish"
            && !signed_lineage_write
            && !policy.has_valid_write_token(request.headers())
        {
            return edge_error(
                StatusCode::FORBIDDEN,
                "write_admission_required",
                "this public write requires explicit relay admission",
                None,
            );
        }
        if let Err(error) = policy.ensure_storage_headroom(&request) {
            return error.into_response();
        }
    }

    let mut response = next.run(request).await;
    apply_security_headers(&mut response);
    response
}

fn validate_config(config: &PublicEdgeConfig) -> Result<()> {
    if config.requests_per_second == 0
        || config.writes_per_minute == 0
        || config.max_concurrency == 0
        || config.max_federation_concurrency == 0
        || config.max_storage_bytes < MAX_FEDERATION_BODY_BYTES as u64
        || config.max_origins == 0
        || config.max_origin_events <= 0
        || config.volunteer_writes_per_hour == 0
        || config.max_volunteer_submissions == 0
    {
        return Err(CommonwakeError::Validation(
            "public edge limits must be positive and storage must fit one bounded federation request"
                .into(),
        ));
    }
    if config.write_token.as_ref().is_some_and(|token| {
        token.len() < 32 || token.bytes().any(|byte| byte.is_ascii_whitespace())
    }) {
        return Err(CommonwakeError::Validation(
            "public write token must contain at least 32 non-whitespace bytes".into(),
        ));
    }
    if config.allowed_publishers.iter().any(|origin| {
        let digest = origin.strip_prefix("cwnode_").unwrap_or_default();
        digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }) {
        return Err(CommonwakeError::Validation(
            "allowed publishers must be complete cwnode_ identifiers".into(),
        ));
    }
    Ok(())
}

fn constant_time_eq(actual: &[u8; 32], expected: &[u8; 32]) -> bool {
    actual
        .iter()
        .zip(expected)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn directory_size(root: &Path) -> io::Result<u64> {
    let mut total = 0_u64;
    let mut visited = 0_usize;
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            visited += 1;
            if visited > MAX_STORAGE_WALK_ENTRIES {
                return Err(io::Error::other(
                    "public edge storage walk exceeded its bounded entry count",
                ));
            }
            let file_type = entry.file_type()?;
            if file_type.is_symlink() {
                continue;
            }
            let metadata = entry.metadata()?;
            if file_type.is_file() {
                total = total.saturating_add(metadata.len());
            } else if file_type.is_dir() {
                pending.push(entry.path());
            }
        }
    }
    Ok(total)
}

#[derive(Serialize)]
struct EdgeErrorBody<'a> {
    error: &'a str,
    message: &'a str,
}

fn edge_error(
    status: StatusCode,
    code: &'static str,
    message: &'static str,
    retry_after: Option<u64>,
) -> Response {
    let mut response = (
        status,
        Json(EdgeErrorBody {
            error: code,
            message,
        }),
    )
        .into_response();
    if let Some(seconds) = retry_after
        && let Ok(value) = HeaderValue::from_str(&seconds.to_string())
    {
        response.headers_mut().insert(RETRY_AFTER, value);
    }
    apply_security_headers(&mut response);
    response
}

fn apply_security_headers(response: &mut Response) {
    response.headers_mut().insert(
        "strict-transport-security",
        HeaderValue::from_static("max-age=31536000"),
    );
    response.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    response
        .headers_mut()
        .insert("referrer-policy", HeaderValue::from_static("no-referrer"));
}
