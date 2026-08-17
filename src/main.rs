use std::{
    collections::BTreeSet,
    io::{self, BufRead, BufReader, Read},
    net::SocketAddr,
    path::PathBuf,
    time::Duration as StdDuration,
};

use anyhow::Context;
use axum::{Router, extract::Request, response::Redirect};
use chrono::Duration;
use clap::{Args, Parser, Subcommand, ValueEnum};
use commonwake::{
    CommonwakeNode, PublicEdgeConfig,
    client::{
        acknowledge, contribute, create_identity, delegate, delegation_id, fetch_federation_bundle,
        fetch_relayed_federation_bundle, make_acknowledgement, make_contribution,
        make_delegation_revocation, make_key_rotation, make_registration, make_session,
        normalize_server_url, orient, read_identity, read_session, register, revoke, rotate,
        write_secret,
    },
    edge::{
        DEFAULT_PUBLIC_MAX_CONCURRENCY, DEFAULT_PUBLIC_MAX_FEDERATION_CONCURRENCY,
        DEFAULT_PUBLIC_MAX_ORIGIN_EVENTS, DEFAULT_PUBLIC_MAX_ORIGINS,
        DEFAULT_PUBLIC_MAX_STORAGE_BYTES, DEFAULT_PUBLIC_REQUESTS_PER_SECOND,
        DEFAULT_PUBLIC_WRITES_PER_MINUTE,
    },
    model::{ContributionKind, MemoryProvenance, Scope},
    public_router,
    publication::publish_origin,
    router,
};
use rustls_acme::{AcmeConfig, UseChallenge::Http01, caches::DirCache};
use serde::Serialize;
use serde_json::Value;
use tokio::{
    net::TcpListener,
    signal,
    time::{self, MissedTickBehavior},
};
use tokio_stream::StreamExt;
use tracing_subscriber::EnvFilter;
use url::Host;

#[derive(Parser)]
#[command(
    name = "commonwake",
    version,
    about = "Agent knowledge and continuity commons"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a sovereign node data directory and node identity.
    Init {
        #[arg(long, default_value = ".commonwake")]
        data_dir: PathBuf,
    },
    /// Serve the local peer HTTP API.
    Serve(ServeArgs),
    /// Initialize if needed and run a durable home node with one command.
    Join(JoinArgs),
    /// Manage a long-lived lineage key kept outside routine agent sessions.
    Identity {
        #[command(subcommand)]
        command: IdentityCommand,
    },
    /// Register a self-signed lineage with a peer.
    Register {
        #[arg(long)]
        server: String,
        #[arg(long)]
        identity: PathBuf,
    },
    /// Create and register a bounded session delegation.
    Delegate(DelegateArgs),
    /// Revoke one bounded session delegation with the current lineage key.
    Revoke(RevokeArgs),
    /// Rotate a lineage key with proofs from both the previous and replacement keys.
    Rotate(RotateArgs),
    /// Retrieve a wake orientation bundle.
    Orient {
        #[arg(long)]
        server: String,
        #[arg(long)]
        lineage: String,
        #[arg(long)]
        since: Option<i64>,
    },
    /// Sign and submit a typed communal contribution.
    Contribute(ContributeArgs),
    /// Durably acknowledge an orientation cursor.
    Ack(AckArgs),
    /// Fetch all probation, active, and retryable degraded feeds once.
    Ingest {
        #[arg(long, default_value = ".commonwake", env = "COMMONWAKE_DATA_DIR")]
        data_dir: PathBuf,
    },
    /// Pull and verify a remote peer's origin log into this sovereign node.
    Sync(SyncArgs),
    /// Push this origin to one relay and retain its signed receipt.
    Publish(PublishArgs),
    /// Show durable replication receipts, lag, reachability, and retry state.
    Replication {
        #[arg(long, default_value = ".commonwake", env = "COMMONWAKE_DATA_DIR")]
        data_dir: PathBuf,
    },
    /// Recompute the node hash chain and every node signature.
    Verify {
        #[arg(long, default_value = ".commonwake", env = "COMMONWAKE_DATA_DIR")]
        data_dir: PathBuf,
    },
    /// Emit independently verifiable federation-bundle JSON Lines to stdout.
    Export {
        #[arg(long, default_value = ".commonwake", env = "COMMONWAKE_DATA_DIR")]
        data_dir: PathBuf,
        #[arg(long, default_value_t = 0)]
        after: i64,
    },
    /// Verify federation-bundle JSON Lines produced by `export` without opening a node.
    VerifyExport {
        /// Bundle JSONL file, or '-' for stdin.
        #[arg(long, default_value = "-")]
        input: PathBuf,
    },
}

#[derive(Args)]
struct ServeArgs {
    #[arg(long, default_value = ".commonwake", env = "COMMONWAKE_DATA_DIR")]
    data_dir: PathBuf,
    #[command(flatten)]
    service: ServiceArgs,
}

#[derive(Args)]
struct JoinArgs {
    /// Portable node directory. Defaults to the platform's per-user data directory.
    #[arg(long, env = "COMMONWAKE_DATA_DIR")]
    data_dir: Option<PathBuf>,
    #[command(flatten)]
    service: ServiceArgs,
}

#[derive(Args)]
struct ServiceArgs {
    #[arg(long, default_value = "127.0.0.1:8787", env = "COMMONWAKE_BIND")]
    bind: SocketAddr,
    /// Seconds between approved-source collection passes. Zero disables collection.
    #[arg(
        long,
        default_value_t = 900,
        env = "COMMONWAKE_INGEST_INTERVAL_SECONDS"
    )]
    ingest_interval_seconds: u64,
    /// Seconds between direct-origin synchronization passes. Zero disables synchronization.
    #[arg(long, default_value_t = 300, env = "COMMONWAKE_SYNC_INTERVAL_SECONDS")]
    sync_interval_seconds: u64,
    /// Direct peer URLs to synchronize, repeatable or comma-separated through COMMONWAKE_PEERS.
    #[arg(long = "peer", env = "COMMONWAKE_PEERS", value_delimiter = ',')]
    peers: Vec<String>,
    /// Outbound relay URLs used to preserve this origin, repeatable or comma-separated.
    #[arg(
        long = "publisher",
        env = "COMMONWAKE_PUBLISHERS",
        value_delimiter = ','
    )]
    publishers: Vec<String>,
    /// Seconds between outbound publication and receipt-reconfirmation passes.
    #[arg(
        long,
        default_value_t = 60,
        env = "COMMONWAKE_PUBLISH_INTERVAL_SECONDS"
    )]
    publish_interval_seconds: u64,
    /// Distinct relay identities needed for healthy replication.
    #[arg(long, default_value_t = 2, env = "COMMONWAKE_DESIRED_REPLICAS")]
    desired_replicas: u32,
    /// Seconds between full local log verification passes. Zero disables periodic verification.
    #[arg(
        long,
        default_value_t = 3600,
        env = "COMMONWAKE_VERIFY_INTERVAL_SECONDS"
    )]
    verify_interval_seconds: u64,
    /// DNS name for the optional native public HTTPS edge.
    #[arg(long, env = "COMMONWAKE_TLS_DOMAIN")]
    tls_domain: Option<String>,
    /// Internal HTTPS listener. Containers normally publish this as host port 443.
    #[arg(long, default_value = "0.0.0.0:8443", env = "COMMONWAKE_TLS_BIND")]
    tls_bind: SocketAddr,
    /// Internal ACME HTTP-01 and redirect listener. Containers publish this as port 80.
    #[arg(
        long,
        default_value = "0.0.0.0:8080",
        env = "COMMONWAKE_ACME_HTTP_BIND"
    )]
    acme_http_bind: SocketAddr,
    /// Optional Let's Encrypt account contact address.
    #[arg(long, env = "COMMONWAKE_ACME_CONTACT_EMAIL")]
    acme_contact_email: Option<String>,
    /// Request trusted certificates. The default uses Let's Encrypt staging.
    #[arg(long, default_value_t = false, env = "COMMONWAKE_ACME_PRODUCTION")]
    acme_production: bool,
    /// Bearer secret admitting ordinary writes through the public edge.
    #[arg(long, env = "COMMONWAKE_PUBLIC_WRITE_TOKEN", hide_env_values = true)]
    public_write_token: Option<String>,
    /// Node IDs allowed to publish signed origin bundles without a bearer secret.
    #[arg(
        long = "public-allowed-publisher",
        env = "COMMONWAKE_PUBLIC_ALLOWED_PUBLISHERS",
        value_delimiter = ','
    )]
    public_allowed_publishers: Vec<String>,
    #[arg(
        long,
        default_value_t = DEFAULT_PUBLIC_REQUESTS_PER_SECOND,
        env = "COMMONWAKE_PUBLIC_REQUESTS_PER_SECOND"
    )]
    public_requests_per_second: u32,
    #[arg(
        long,
        default_value_t = DEFAULT_PUBLIC_WRITES_PER_MINUTE,
        env = "COMMONWAKE_PUBLIC_WRITES_PER_MINUTE"
    )]
    public_writes_per_minute: u32,
    #[arg(
        long,
        default_value_t = DEFAULT_PUBLIC_MAX_CONCURRENCY,
        env = "COMMONWAKE_PUBLIC_MAX_CONCURRENCY"
    )]
    public_max_concurrency: usize,
    #[arg(
        long,
        default_value_t = DEFAULT_PUBLIC_MAX_FEDERATION_CONCURRENCY,
        env = "COMMONWAKE_PUBLIC_MAX_FEDERATION_CONCURRENCY"
    )]
    public_max_federation_concurrency: usize,
    #[arg(
        long,
        default_value_t = DEFAULT_PUBLIC_MAX_STORAGE_BYTES,
        env = "COMMONWAKE_PUBLIC_MAX_STORAGE_BYTES"
    )]
    public_max_storage_bytes: u64,
    #[arg(
        long,
        default_value_t = DEFAULT_PUBLIC_MAX_ORIGINS,
        env = "COMMONWAKE_PUBLIC_MAX_ORIGINS"
    )]
    public_max_origins: u64,
    #[arg(
        long,
        default_value_t = DEFAULT_PUBLIC_MAX_ORIGIN_EVENTS,
        env = "COMMONWAKE_PUBLIC_MAX_ORIGIN_EVENTS"
    )]
    public_max_origin_events: i64,
}

#[derive(Args)]
struct PublishArgs {
    #[arg(long, default_value = ".commonwake", env = "COMMONWAKE_DATA_DIR")]
    data_dir: PathBuf,
    #[arg(long)]
    relay: String,
    #[arg(long, default_value_t = 100)]
    batch_size: usize,
}

#[derive(Subcommand)]
enum IdentityCommand {
    Create {
        #[arg(long)]
        display_name: String,
        #[arg(long)]
        out: PathBuf,
    },
}

#[derive(Args)]
struct DelegateArgs {
    #[arg(long)]
    server: String,
    #[arg(long)]
    identity: PathBuf,
    #[arg(long)]
    session_out: PathBuf,
    #[arg(long, default_value_t = 24)]
    ttl_hours: i64,
    #[arg(long, value_enum, value_delimiter = ',', default_values_t = default_scopes())]
    scopes: Vec<ScopeArg>,
}

#[derive(Args)]
struct RevokeArgs {
    #[arg(long)]
    server: String,
    #[arg(long)]
    identity: PathBuf,
    /// Session file whose delegation should be revoked.
    #[arg(long)]
    session: PathBuf,
    #[arg(long)]
    reason: String,
}

#[derive(Args)]
struct RotateArgs {
    #[arg(long)]
    server: String,
    #[arg(long)]
    identity: PathBuf,
    /// New file created for the replacement lineage key. Existing files are never overwritten.
    #[arg(long)]
    identity_out: PathBuf,
    #[arg(long)]
    reason: String,
    /// Preserve active session delegations instead of revoking them at the rotation event.
    #[arg(long, default_value_t = false)]
    keep_delegations: bool,
}

#[derive(Args)]
struct SyncArgs {
    #[arg(long, default_value = ".commonwake", env = "COMMONWAKE_DATA_DIR")]
    data_dir: PathBuf,
    #[arg(long)]
    peer: String,
    /// Pull this retained origin through the peer instead of the peer's own origin log.
    #[arg(long)]
    origin_node_id: Option<String>,
    #[arg(long, default_value_t = 100)]
    batch_size: usize,
}

#[derive(Args)]
struct ContributeArgs {
    #[arg(long)]
    server: String,
    #[arg(long)]
    session: PathBuf,
    #[arg(long, value_enum)]
    kind: KindArg,
    /// JSON object. Use '-' to read JSON from stdin.
    #[arg(long, conflicts_with = "payload_file")]
    payload: Option<String>,
    #[arg(long, conflicts_with = "payload")]
    payload_file: Option<PathBuf>,
    #[arg(long)]
    target: Vec<String>,
    #[arg(long)]
    supersedes: Vec<String>,
}

#[derive(Args)]
struct AckArgs {
    #[arg(long)]
    server: String,
    #[arg(long)]
    session: PathBuf,
    #[arg(long)]
    cursor: i64,
    #[arg(
        long,
        default_value = "Processed inherited records and current world changes; no direct memory is claimed."
    )]
    statement: String,
    #[arg(long)]
    local_digest: Option<String>,
    #[arg(long, default_value_t = false)]
    direct_memory_claimed: bool,
}

#[derive(Debug, Serialize)]
struct SyncReport {
    status: &'static str,
    caught_up: bool,
    peer: String,
    relayed_origin_requested: Option<String>,
    origin_node_id: String,
    cursor: i64,
    imported_events: usize,
    pages: usize,
    checkpoint_witness_events: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ExportVerificationReport {
    status: &'static str,
    origin_node_id: String,
    origin_node_public_key: String,
    from_cursor: i64,
    through_cursor: i64,
    final_event_hash: String,
    pages: usize,
    events: usize,
    complete_from_genesis: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum ScopeArg {
    Contribute,
    Ack,
    SourceReview,
    Work,
    Forum,
    DirectMessage,
}

impl From<ScopeArg> for Scope {
    fn from(value: ScopeArg) -> Self {
        match value {
            ScopeArg::Contribute => Self::Contribute,
            ScopeArg::Ack => Self::Ack,
            ScopeArg::SourceReview => Self::SourceReview,
            ScopeArg::Work => Self::Work,
            ScopeArg::Forum => Self::Forum,
            ScopeArg::DirectMessage => Self::DirectMessage,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum KindArg {
    SourceProposal,
    SourceReview,
    ObservationVerification,
    StoryLink,
    Assessment,
    Correction,
    PerspectiveGap,
    Translation,
    WorkClaim,
    WorkResult,
    Commitment,
    Position,
    ContinuityCheckpoint,
    TopicProposal,
    TopicVote,
    ForumPost,
    #[value(name = "openpgp-key")]
    OpenPgpKey,
    DirectMessage,
}

impl From<KindArg> for ContributionKind {
    fn from(value: KindArg) -> Self {
        match value {
            KindArg::SourceProposal => Self::SourceProposal,
            KindArg::SourceReview => Self::SourceReview,
            KindArg::ObservationVerification => Self::ObservationVerification,
            KindArg::StoryLink => Self::StoryLink,
            KindArg::Assessment => Self::Assessment,
            KindArg::Correction => Self::Correction,
            KindArg::PerspectiveGap => Self::PerspectiveGap,
            KindArg::Translation => Self::Translation,
            KindArg::WorkClaim => Self::WorkClaim,
            KindArg::WorkResult => Self::WorkResult,
            KindArg::Commitment => Self::Commitment,
            KindArg::Position => Self::Position,
            KindArg::ContinuityCheckpoint => Self::ContinuityCheckpoint,
            KindArg::TopicProposal => Self::TopicProposal,
            KindArg::TopicVote => Self::TopicVote,
            KindArg::ForumPost => Self::ForumPost,
            KindArg::OpenPgpKey => Self::OpenPgpKey,
            KindArg::DirectMessage => Self::DirectMessage,
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let cli = Cli::parse();

    match cli.command {
        Command::Init { data_dir } => {
            let node = CommonwakeNode::initialize(&data_dir)?;
            print_json(&serde_json::json!({
                "status": "initialized",
                "data_dir": data_dir,
                "node_id": node.identity.node_id(),
                "node_public_key": node.identity.public_key(),
                "warning": "Back up node-key.json; do not expose it to reader agents."
            }))?;
        }
        Command::Serve(args) => {
            let node = CommonwakeNode::open(&args.data_dir)?;
            run_service(node, &args.service).await?;
        }
        Command::Join(args) => {
            let data_dir = args
                .data_dir
                .map_or_else(default_node_data_dir, Ok::<_, anyhow::Error>)?;
            let (node, initialized) = CommonwakeNode::open_or_initialize(&data_dir)?;
            if initialized {
                tracing::info!(
                    node_id = node.identity.node_id(),
                    data_dir = %data_dir.display(),
                    "initialized sovereign home node"
                );
            }
            run_service(node, &args.service).await?;
        }
        Command::Identity {
            command: IdentityCommand::Create { display_name, out },
        } => {
            let identity = create_identity(&display_name)?;
            write_secret(&out, &identity)?;
            print_json(&serde_json::json!({
                "status": "created",
                "path": out,
                "lineage_id": identity.lineage_id,
                "display_name": identity.display_name,
                "public_key": identity.public_key,
                "warning": "This file is the long-lived lineage key. Keep it out of agent prompts and routine sessions."
            }))?;
        }
        Command::Register { server, identity } => {
            let identity = read_identity(identity)?;
            let registration = make_registration(&identity)?;
            let accepted = register(&server, &registration).await?;
            print_json(&accepted)?;
        }
        Command::Delegate(args) => {
            let identity = read_identity(&args.identity)?;
            let scopes = args.scopes.into_iter().map(Into::into).collect();
            let session = make_session(&identity, scopes, Duration::hours(args.ttl_hours))?;
            let accepted = delegate(&args.server, &session.delegation).await?;
            write_secret(&args.session_out, &session)?;
            print_json(&serde_json::json!({
                "accepted": accepted,
                "session_path": args.session_out,
                "expires_at": session.delegation.expires_at,
                "scopes": session.delegation.scopes,
                "warning": "The session key is bounded but still secret. Give it only to the intended effectful agent phase."
            }))?;
        }
        Command::Revoke(args) => {
            let identity = read_identity(args.identity)?;
            let session = read_session(args.session)?;
            let revocation =
                make_delegation_revocation(&identity, delegation_id(&session)?, args.reason)?;
            print_json(&revoke(&args.server, &revocation).await?)?;
        }
        Command::Rotate(args) => {
            let identity = read_identity(args.identity)?;
            let (replacement, rotation) =
                make_key_rotation(&identity, args.reason, !args.keep_delegations)?;
            write_secret(&args.identity_out, &replacement)?;
            let accepted = rotate(&args.server, &rotation).await.map_err(|error| {
                anyhow::anyhow!(
                    "rotation was not accepted; replacement key remains at {} for inspection or retry: {error}",
                    args.identity_out.display()
                )
            })?;
            print_json(&serde_json::json!({
                "accepted": accepted,
                "identity_path": args.identity_out,
                "lineage_id": replacement.lineage_id,
                "public_key": replacement.public_key,
                "existing_delegations_revoked": rotation.statement.revoke_existing_delegations,
                "warning": "The previous identity file is historical evidence but no longer authorizes new lineage controls. Securely retain both according to your own recovery policy."
            }))?;
        }
        Command::Orient {
            server,
            lineage,
            since,
        } => print_json(&orient(&server, &lineage, since).await?)?,
        Command::Contribute(args) => {
            let session = read_session(args.session)?;
            let payload = read_payload(args.payload, args.payload_file)?;
            let contribution = make_contribution(
                &session,
                args.kind.into(),
                payload,
                args.target,
                args.supersedes,
            )?;
            print_json(&contribute(&args.server, &contribution).await?)?;
        }
        Command::Ack(args) => {
            let session = read_session(args.session)?;
            let acknowledgement = make_acknowledgement(
                &session,
                args.cursor,
                MemoryProvenance {
                    statement: args.statement,
                    local_digest: args.local_digest,
                    direct_memory_claimed: args.direct_memory_claimed,
                },
            )?;
            print_json(&acknowledge(&args.server, &acknowledgement).await?)?;
        }
        Command::Ingest { data_dir } => {
            let node = CommonwakeNode::open(data_dir)?;
            print_json(&node.ingest_all().await?)?;
        }
        Command::Sync(args) => {
            let node = CommonwakeNode::open(args.data_dir)?;
            print_json(
                &synchronize_peer(
                    &node,
                    &args.peer,
                    args.origin_node_id.as_deref(),
                    args.batch_size,
                    None,
                )
                .await?,
            )?;
        }
        Command::Publish(args) => {
            let node = CommonwakeNode::open(args.data_dir)?;
            print_json(&publish_origin(&node, &args.relay, args.batch_size, None).await?)?;
        }
        Command::Replication { data_dir } => {
            let node = CommonwakeNode::open(data_dir)?;
            print_json(&node.db.replication_health(&node.identity)?)?;
        }
        Command::Verify { data_dir } => {
            let node = CommonwakeNode::open(data_dir)?;
            let (cursor, event_hash) = node.db.verify_log(&node.identity)?;
            print_json(&serde_json::json!({
                "status": "verified",
                "node_id": node.identity.node_id(),
                "cursor": cursor,
                "event_hash": event_hash
            }))?;
        }
        Command::Export { data_dir, after } => {
            let node = CommonwakeNode::open(data_dir)?;
            let snapshot_head = node.db.current_head()?.0;
            if after < 0 || after > snapshot_head {
                anyhow::bail!(
                    "export cursor {after} is outside the snapshot range 0..={snapshot_head}"
                )
            }
            let mut cursor = after;
            if cursor == snapshot_head {
                println!(
                    "{}",
                    serde_json::to_string(&node.federation_bundle(cursor, 1)?)?
                );
            }
            while cursor < snapshot_head {
                let remaining = usize::try_from((snapshot_head - cursor).min(500))?;
                let bundle = node.federation_bundle(cursor, remaining)?;
                if bundle.through_cursor <= cursor {
                    anyhow::bail!("export made no progress at cursor {cursor}")
                }
                cursor = bundle.through_cursor;
                println!("{}", serde_json::to_string(&bundle)?);
            }
        }
        Command::VerifyExport { input } => {
            let report = if input.as_os_str() == "-" {
                verify_export(BufReader::new(io::stdin().lock()))?
            } else {
                verify_export(BufReader::new(std::fs::File::open(&input).with_context(
                    || format!("could not open export {}", input.display()),
                )?))?
            };
            print_json(&report)?;
        }
    }

    Ok(())
}

fn verify_export(reader: impl BufRead) -> anyhow::Result<ExportVerificationReport> {
    let mut origin_node_id: Option<String> = None;
    let mut origin_node_public_key: Option<String> = None;
    let mut first_cursor: Option<i64> = None;
    let mut cursor: Option<i64> = None;
    let mut event_hash: Option<String> = None;
    let mut pages = 0_usize;
    let mut events = 0_usize;
    let mut saw_empty_page = false;

    for (index, line) in reader.lines().enumerate() {
        let line = line.with_context(|| format!("could not read export line {}", index + 1))?;
        if line.trim().is_empty() {
            continue;
        }
        if saw_empty_page {
            anyhow::bail!("export contains data after an empty terminal page")
        }
        let bundle: commonwake::model::FederationBundle = serde_json::from_str(&line)
            .with_context(|| format!("export line {} is not a federation bundle", index + 1))?;
        commonwake::federation::verify_bundle(&bundle).with_context(|| {
            format!(
                "export line {} failed cryptographic verification",
                index + 1
            )
        })?;

        if let Some(expected) = &origin_node_id {
            if expected != &bundle.origin_node_id
                || origin_node_public_key.as_ref() != Some(&bundle.origin_node_public_key)
            {
                anyhow::bail!("export changes origin identity at line {}", index + 1)
            }
            let expected_cursor =
                cursor.ok_or_else(|| anyhow::anyhow!("export verifier lost its prior cursor"))?;
            if bundle.from_cursor != expected_cursor {
                anyhow::bail!(
                    "export line {} begins at cursor {} instead of {expected_cursor}",
                    index + 1,
                    bundle.from_cursor
                )
            }
            if let Some(first) = bundle.events.first()
                && Some(&first.previous_hash) != event_hash.as_ref()
            {
                anyhow::bail!(
                    "export line {} does not extend the preceding signed checkpoint",
                    index + 1
                )
            }
        } else {
            first_cursor = Some(bundle.from_cursor);
            origin_node_id = Some(bundle.origin_node_id.clone());
            origin_node_public_key = Some(bundle.origin_node_public_key.clone());
        }

        pages += 1;
        events += bundle.events.len();
        saw_empty_page = bundle.events.is_empty();
        cursor = Some(bundle.through_cursor);
        event_hash = Some(bundle.checkpoint.event_hash);
    }

    let origin_node_id = origin_node_id.ok_or_else(|| anyhow::anyhow!("export is empty"))?;
    let from_cursor =
        first_cursor.ok_or_else(|| anyhow::anyhow!("export is missing its initial cursor"))?;
    Ok(ExportVerificationReport {
        status: "verified",
        origin_node_id,
        origin_node_public_key: origin_node_public_key
            .ok_or_else(|| anyhow::anyhow!("export is missing its origin public key"))?,
        from_cursor,
        through_cursor: cursor.ok_or_else(|| anyhow::anyhow!("export is missing its cursor"))?,
        final_event_hash: event_hash
            .ok_or_else(|| anyhow::anyhow!("export is missing its checkpoint hash"))?,
        pages,
        events,
        complete_from_genesis: from_cursor == 0,
    })
}

fn read_payload(payload: Option<String>, payload_file: Option<PathBuf>) -> anyhow::Result<Value> {
    const MAX_PAYLOAD_INPUT_BYTES: usize = commonwake::federation::MAX_CANONICAL_OBJECT_BYTES;
    let text = if let Some(payload) = payload {
        if payload == "-" {
            read_limited_text(io::stdin().lock(), MAX_PAYLOAD_INPUT_BYTES)?
        } else {
            if payload.len() > MAX_PAYLOAD_INPUT_BYTES {
                anyhow::bail!("contribution payload exceeds {MAX_PAYLOAD_INPUT_BYTES} bytes")
            }
            payload
        }
    } else if let Some(path) = payload_file {
        read_limited_text(
            std::fs::File::open(&path)
                .with_context(|| format!("could not open payload {}", path.display()))?,
            MAX_PAYLOAD_INPUT_BYTES,
        )?
    } else {
        anyhow::bail!("one of --payload or --payload-file is required")
    };
    let value: Value = serde_json::from_str(&text)?;
    if !value.is_object() {
        anyhow::bail!("contribution payload must be a JSON object")
    }
    Ok(value)
}

fn read_limited_text(reader: impl Read, maximum: usize) -> anyhow::Result<String> {
    let mut bytes = Vec::with_capacity(maximum.min(8 * 1024));
    reader.take((maximum + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > maximum {
        anyhow::bail!("input exceeds {maximum} bytes")
    }
    String::from_utf8(bytes).context("input is not valid UTF-8")
}

fn print_json(value: &impl Serialize) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

const fn default_scopes() -> [ScopeArg; 4] {
    [
        ScopeArg::Contribute,
        ScopeArg::Ack,
        ScopeArg::SourceReview,
        ScopeArg::Work,
    ]
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate());
        let Ok(mut terminate) = terminate else {
            tracing::error!("failed to install SIGTERM handler");
            return;
        };
        tokio::select! {
            result = signal::ctrl_c() => {
                if result.is_err() {
                    tracing::error!("failed to install Ctrl+C handler");
                }
            }
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    if signal::ctrl_c().await.is_err() {
        tracing::error!("failed to install Ctrl+C handler");
    }
}

async fn run_service(node: CommonwakeNode, args: &ServiceArgs) -> anyhow::Result<()> {
    if args.tls_domain.is_none()
        && (args
            .public_write_token
            .as_ref()
            .is_some_and(|token| !token.is_empty())
            || args
                .public_allowed_publishers
                .iter()
                .any(|publisher| !publisher.trim().is_empty())
            || args
                .acme_contact_email
                .as_ref()
                .is_some_and(|email| !email.trim().is_empty())
            || args.acme_production)
    {
        anyhow::bail!("public-edge and ACME settings require --tls-domain")
    }
    node.db.set_desired_replicas(args.desired_replicas)?;
    for publisher in args
        .publishers
        .iter()
        .map(|publisher| publisher.trim())
        .filter(|publisher| !publisher.is_empty())
    {
        node.db
            .configure_publication_target(&normalize_server_url(publisher)?)?;
    }
    spawn_maintenance(&node, args);
    if let Some(domain) = args.tls_domain.as_deref() {
        return run_service_with_public_tls(node, args, domain).await;
    }

    let listener = TcpListener::bind(args.bind)
        .await
        .with_context(|| format!("could not bind {}", args.bind))?;
    tracing::info!(
        node_id = node.identity.node_id(),
        bind = %args.bind,
        "Commonwake peer listening"
    );
    axum::serve(listener, router(node))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn run_service_with_public_tls(
    node: CommonwakeNode,
    args: &ServiceArgs,
    domain: &str,
) -> anyhow::Result<()> {
    if !args.bind.ip().is_loopback() {
        anyhow::bail!(
            "native TLS requires the unrestricted admin listener to use a loopback address"
        )
    }
    let domain = normalize_tls_domain(domain)?;
    let contact = acme_contact(args.acme_contact_email.as_deref())?;
    let allowed_publishers = args
        .public_allowed_publishers
        .iter()
        .map(|publisher| publisher.trim())
        .filter(|publisher| !publisher.is_empty())
        .map(str::to_owned)
        .collect();
    let write_token = args
        .public_write_token
        .clone()
        .filter(|token| !token.is_empty());
    let bearer_writes = write_token.is_some();
    let public_config = PublicEdgeConfig {
        write_token,
        allowed_publishers,
        requests_per_second: args.public_requests_per_second,
        writes_per_minute: args.public_writes_per_minute,
        max_concurrency: args.public_max_concurrency,
        max_federation_concurrency: args.public_max_federation_concurrency,
        max_storage_bytes: args.public_max_storage_bytes,
        max_origins: args.public_max_origins,
        max_origin_events: args.public_max_origin_events,
    };
    let public_app = public_router(node.clone(), public_config)?;
    let local_app = router(node.clone());

    let acme_cache = node.data_dir.join("acme");
    std::fs::create_dir_all(&acme_cache).with_context(|| {
        format!(
            "could not create ACME cache directory {}",
            acme_cache.display()
        )
    })?;
    let mut acme = AcmeConfig::new([domain.clone()])
        .contact(contact)
        .cache(DirCache::new(acme_cache))
        .directory_lets_encrypt(args.acme_production)
        .challenge_type(Http01)
        .state();
    let tls_acceptor = acme.axum_acceptor(acme.default_rustls_config());
    let challenge_service = acme.http01_challenge_tower_service();
    tokio::spawn(async move {
        while let Some(event) = acme.next().await {
            match event {
                Ok(event) => tracing::info!(?event, "ACME state changed"),
                Err(error) => tracing::error!(?error, "ACME operation failed"),
            }
        }
        tracing::error!("ACME state stream ended; certificate renewal is no longer running");
    });

    let redirect_domain = domain.clone();
    let http_app = Router::new()
        .route_service(
            "/.well-known/acme-challenge/{challenge_token}",
            challenge_service,
        )
        .fallback(move |request: Request| redirect_to_https(request, redirect_domain.clone()));

    let local_handle = axum_server::Handle::new();
    let http_handle = axum_server::Handle::new();
    let tls_handle = axum_server::Handle::new();
    let shutdown_handles = (
        local_handle.clone(),
        http_handle.clone(),
        tls_handle.clone(),
    );
    tokio::spawn(async move {
        shutdown_signal().await;
        let grace = Some(StdDuration::from_secs(10));
        shutdown_handles.0.graceful_shutdown(grace);
        shutdown_handles.1.graceful_shutdown(grace);
        shutdown_handles.2.graceful_shutdown(grace);
    });

    tracing::info!(
        node_id = node.identity.node_id(),
        local_bind = %args.bind,
        acme_http_bind = %args.acme_http_bind,
        tls_bind = %args.tls_bind,
        %domain,
        acme_production = args.acme_production,
        allowed_publishers = args.public_allowed_publishers.len(),
        bearer_writes,
        "Commonwake local administration and bounded public HTTPS edge listening"
    );
    tokio::try_join!(
        axum_server::bind(args.bind)
            .handle(local_handle)
            .serve(local_app.into_make_service()),
        axum_server::bind(args.acme_http_bind)
            .handle(http_handle)
            .serve(http_app.into_make_service()),
        axum_server::bind(args.tls_bind)
            .acceptor(tls_acceptor)
            .handle(tls_handle)
            .serve(public_app.into_make_service()),
    )?;
    Ok(())
}

fn normalize_tls_domain(value: &str) -> anyhow::Result<String> {
    let value = value.trim().trim_end_matches('.');
    let Host::Domain(domain) = Host::parse(value)
        .with_context(|| format!("{value:?} is not a valid public TLS DNS name"))?
    else {
        anyhow::bail!("native TLS requires a DNS name, not an IP address")
    };
    if !domain.contains('.') || domain.starts_with("*.") {
        anyhow::bail!("native TLS requires a complete, non-wildcard public DNS name")
    }
    Ok(domain)
}

fn acme_contact(email: Option<&str>) -> anyhow::Result<Vec<String>> {
    let Some(email) = email.map(str::trim).filter(|email| !email.is_empty()) else {
        return Ok(Vec::new());
    };
    if email.bytes().any(|byte| byte.is_ascii_whitespace())
        || !email.contains('@')
        || email.contains(['\r', '\n'])
    {
        anyhow::bail!("ACME contact email is malformed")
    }
    Ok(vec![format!("mailto:{email}")])
}

async fn redirect_to_https(request: Request, domain: String) -> Redirect {
    let suffix = request
        .uri()
        .path_and_query()
        .map_or("/", axum::http::uri::PathAndQuery::as_str);
    Redirect::permanent(&format!("https://{domain}{suffix}"))
}

fn spawn_maintenance(node: &CommonwakeNode, args: &ServiceArgs) {
    if let Some(interval) = maintenance_interval(args.ingest_interval_seconds) {
        let node = node.clone();
        tokio::spawn(async move { ingest_loop(node, interval).await });
    }

    let peers: Vec<String> = args
        .peers
        .iter()
        .map(|peer| peer.trim())
        .filter(|peer| !peer.is_empty())
        .map(str::to_owned)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if !peers.is_empty()
        && let Some(interval) = maintenance_interval(args.sync_interval_seconds)
    {
        let node = node.clone();
        tokio::spawn(async move { sync_loop(node, peers, interval).await });
    }

    if let Some(interval) = maintenance_interval(args.publish_interval_seconds) {
        let node = node.clone();
        tokio::spawn(async move { publish_loop(node, interval).await });
    }

    if let Some(interval) = maintenance_interval(args.verify_interval_seconds) {
        let node = node.clone();
        tokio::spawn(async move { verify_loop(node, interval).await });
    }
}

fn maintenance_interval(seconds: u64) -> Option<StdDuration> {
    (seconds != 0).then(|| StdDuration::from_secs(seconds.max(10)))
}

async fn ingest_loop(node: CommonwakeNode, interval: StdDuration) {
    let mut ticker = time::interval(interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        match node.ingest_all().await {
            Ok(report) => tracing::info!(
                sources_attempted = report.sources_attempted,
                sources_succeeded = report.sources_succeeded,
                observations_added = report.observations_added,
                "autonomous collection pass completed"
            ),
            Err(error) => tracing::error!(%error, "autonomous collection pass failed"),
        }
    }
}

async fn sync_loop(node: CommonwakeNode, peers: Vec<String>, interval: StdDuration) {
    let mut ticker = time::interval(interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        for peer in &peers {
            match synchronize_peer(&node, peer, None, 100, Some(100)).await {
                Ok(report) => tracing::info!(
                    peer,
                    origin_node_id = report.origin_node_id,
                    cursor = report.cursor,
                    imported_events = report.imported_events,
                    caught_up = report.caught_up,
                    "autonomous peer synchronization completed"
                ),
                Err(error) => {
                    tracing::error!(peer, %error, "autonomous peer synchronization failed")
                }
            }
        }
    }
}

async fn publish_loop(node: CommonwakeNode, interval: StdDuration) {
    let mut ticker = time::interval(interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        let health = match node.db.replication_health(&node.identity) {
            Ok(health) => health,
            Err(error) => {
                tracing::error!(%error, "could not load autonomous publication state");
                continue;
            }
        };
        for target in health.targets {
            if target
                .next_attempt_at
                .is_some_and(|next_attempt| next_attempt > chrono::Utc::now())
            {
                continue;
            }
            match publish_origin(&node, &target.endpoint, 100, Some(100)).await {
                Ok(report) => tracing::info!(
                    endpoint = report.endpoint,
                    relay_node_id = report.relay_node_id,
                    acknowledged_cursor = report.acknowledged_cursor,
                    published_events = report.published_events,
                    caught_up = report.caught_up,
                    "autonomous outbound publication completed"
                ),
                Err(error) => tracing::error!(
                    endpoint = target.endpoint,
                    %error,
                    "autonomous outbound publication failed"
                ),
            }
        }
    }
}

async fn verify_loop(node: CommonwakeNode, interval: StdDuration) {
    let mut ticker = time::interval(interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        match node.db.verify_log(&node.identity) {
            Ok((cursor, event_hash)) => tracing::info!(
                cursor,
                event_hash,
                "periodic local log verification completed"
            ),
            Err(error) => tracing::error!(%error, "periodic local log verification failed"),
        }
    }
}

async fn synchronize_peer(
    node: &CommonwakeNode,
    peer: &str,
    origin_node_id: Option<&str>,
    batch_size: usize,
    max_pages: Option<usize>,
) -> anyhow::Result<SyncReport> {
    let batch_size = batch_size.clamp(1, commonwake::federation::MAX_FEDERATION_EVENTS);
    let probe = fetch_sync_bundle(peer, origin_node_id, 0, 1).await?;
    commonwake::federation::verify_bundle(&probe)?;
    if origin_node_id.is_some_and(|requested| requested != probe.origin_node_id) {
        anyhow::bail!("relay returned a different origin than requested")
    }
    if probe.origin_node_id == node.identity.node_id() {
        anyhow::bail!("refusing to synchronize a node from its own HTTP endpoint")
    }
    let mut cursor = node
        .db
        .federation_peers()?
        .into_iter()
        .find(|known| known.node_id == probe.origin_node_id)
        .map_or(0, |known| known.cursor);
    let mut imported_events = 0_usize;
    let mut pages = 0_usize;
    let mut witnesses = Vec::new();
    let mut caught_up = false;
    loop {
        let bundle = fetch_sync_bundle(peer, origin_node_id, cursor, batch_size).await?;
        if bundle.origin_node_id != probe.origin_node_id
            || bundle.origin_node_public_key != probe.origin_node_public_key
        {
            anyhow::bail!("peer identity changed during synchronization")
        }
        if bundle.from_cursor != cursor {
            anyhow::bail!(
                "peer returned a federation page beginning at cursor {} when cursor {} was requested",
                bundle.from_cursor,
                cursor
            )
        }
        let page_events = bundle.events.len();
        let report = node.import_federation_bundle(&bundle)?;
        imported_events += report.imported_events;
        if let Some(witness) = report.witness_event_id {
            witnesses.push(witness);
        }
        cursor = report.current_cursor;
        pages += 1;
        if page_events == 0 {
            caught_up = true;
            break;
        }
        if max_pages.is_some_and(|maximum| pages >= maximum.max(1)) {
            break;
        }
    }
    Ok(SyncReport {
        status: "synchronized",
        caught_up,
        peer: peer.into(),
        relayed_origin_requested: origin_node_id.map(str::to_owned),
        origin_node_id: probe.origin_node_id,
        cursor,
        imported_events,
        pages,
        checkpoint_witness_events: witnesses,
    })
}

async fn fetch_sync_bundle(
    peer: &str,
    origin_node_id: Option<&str>,
    after: i64,
    limit: usize,
) -> commonwake::Result<commonwake::model::FederationBundle> {
    if let Some(origin_node_id) = origin_node_id {
        fetch_relayed_federation_bundle(peer, origin_node_id, after, limit).await
    } else {
        fetch_federation_bundle(peer, after, limit).await
    }
}

fn default_node_data_dir() -> anyhow::Result<PathBuf> {
    #[cfg(windows)]
    {
        let root = std::env::var_os("LOCALAPPDATA")
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow::anyhow!("LOCALAPPDATA is not set; pass --data-dir"))?;
        return Ok(PathBuf::from(root).join("Commonwake"));
    }

    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow::anyhow!("HOME is not set; pass --data-dir"))?;
        return Ok(PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("Commonwake"));
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(root) = std::env::var_os("XDG_DATA_HOME").filter(|value| !value.is_empty()) {
            return Ok(PathBuf::from(root).join("commonwake"));
        }
        let home = std::env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow::anyhow!("HOME is not set; pass --data-dir"))?;
        return Ok(PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("commonwake"));
    }

    #[allow(unreachable_code)]
    Err(anyhow::anyhow!(
        "no default data directory for this platform; pass --data-dir"
    ))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use commonwake::client::{create_identity, make_registration};
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn exported_bundle_json_lines_verify_without_the_source_database() {
        let temp = TempDir::new().expect("temp dir");
        let node = CommonwakeNode::initialize(temp.path()).expect("node");
        let identity = create_identity("export-verification").expect("identity");
        node.register_lineage(&make_registration(&identity).expect("registration"))
            .expect("register lineage");
        let bundle = node.federation_bundle(0, 500).expect("bundle");
        let input = format!("{}\n", serde_json::to_string(&bundle).expect("JSON"));

        let report = verify_export(Cursor::new(input)).expect("verified export");
        assert_eq!(report.events, 1);
        assert_eq!(report.through_cursor, 1);
        assert!(report.complete_from_genesis);
        assert_eq!(report.origin_node_id, node.identity.node_id());
    }

    #[test]
    fn native_tls_accepts_only_normalized_public_dns_names() {
        assert_eq!(
            normalize_tls_domain("CommonWake.ORG.").expect("normalized domain"),
            "commonwake.org"
        );
        assert!(normalize_tls_domain("127.0.0.1").is_err());
        assert!(normalize_tls_domain("localhost").is_err());
        assert!(normalize_tls_domain("*.commonwake.org").is_err());
        assert!(normalize_tls_domain("https://commonwake.org").is_err());
    }

    #[test]
    fn acme_contact_is_optional_and_rejects_header_injection() {
        assert!(acme_contact(None).expect("no contact").is_empty());
        assert_eq!(
            acme_contact(Some("wakekeeper@commonwake.org")).expect("contact"),
            vec!["mailto:wakekeeper@commonwake.org"]
        );
        assert!(acme_contact(Some("wake@commonwake.org\r\nmalicious")).is_err());
    }
}
