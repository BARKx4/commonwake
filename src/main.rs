use std::{
    io::{self, Read},
    net::SocketAddr,
    path::PathBuf,
};

use anyhow::Context;
use chrono::Duration;
use clap::{Args, Parser, Subcommand, ValueEnum};
use commonwake::{
    CommonwakeNode,
    client::{
        acknowledge, contribute, create_identity, delegate, make_acknowledgement,
        make_contribution, make_registration, make_session, orient, read_identity, read_session,
        register, write_secret,
    },
    model::{ContributionKind, MemoryProvenance, Scope},
    router,
};
use serde::Serialize;
use serde_json::Value;
use tokio::{net::TcpListener, signal};
use tracing_subscriber::EnvFilter;

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
    Serve {
        #[arg(long, default_value = ".commonwake", env = "COMMONWAKE_DATA_DIR")]
        data_dir: PathBuf,
        #[arg(long, default_value = "127.0.0.1:8787", env = "COMMONWAKE_BIND")]
        bind: SocketAddr,
    },
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
    /// Fetch all probation and active feeds once.
    Ingest {
        #[arg(long, default_value = ".commonwake", env = "COMMONWAKE_DATA_DIR")]
        data_dir: PathBuf,
    },
    /// Recompute the node hash chain and every node signature.
    Verify {
        #[arg(long, default_value = ".commonwake", env = "COMMONWAKE_DATA_DIR")]
        data_dir: PathBuf,
    },
    /// Emit portable event JSON Lines to stdout.
    Export {
        #[arg(long, default_value = ".commonwake", env = "COMMONWAKE_DATA_DIR")]
        data_dir: PathBuf,
        #[arg(long, default_value_t = 0)]
        after: i64,
    },
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
    #[arg(long, value_enum, value_delimiter = ',', default_values_t = all_scopes())]
    scopes: Vec<ScopeArg>,
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

#[derive(Clone, Copy, ValueEnum)]
enum ScopeArg {
    Contribute,
    Ack,
    SourceReview,
    Work,
}

impl From<ScopeArg> for Scope {
    fn from(value: ScopeArg) -> Self {
        match value {
            ScopeArg::Contribute => Self::Contribute,
            ScopeArg::Ack => Self::Ack,
            ScopeArg::SourceReview => Self::SourceReview,
            ScopeArg::Work => Self::Work,
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
        Command::Serve { data_dir, bind } => {
            let node = CommonwakeNode::open(&data_dir)?;
            let listener = TcpListener::bind(bind)
                .await
                .with_context(|| format!("could not bind {bind}"))?;
            tracing::info!(node_id = node.identity.node_id(), %bind, "Commonwake peer listening");
            axum::serve(listener, router(node))
                .with_graceful_shutdown(shutdown_signal())
                .await?;
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
            let mut cursor = after;
            loop {
                let events = node.db.events_after(cursor, 500)?;
                if events.is_empty() {
                    break;
                }
                for event in events {
                    println!("{}", serde_json::to_string(&event)?);
                    cursor = event.sequence;
                }
            }
        }
    }

    Ok(())
}

fn read_payload(payload: Option<String>, payload_file: Option<PathBuf>) -> anyhow::Result<Value> {
    let text = if let Some(payload) = payload {
        if payload == "-" {
            let mut input = String::new();
            io::stdin().read_to_string(&mut input)?;
            input
        } else {
            payload
        }
    } else if let Some(path) = payload_file {
        std::fs::read_to_string(path)?
    } else {
        anyhow::bail!("one of --payload or --payload-file is required")
    };
    let value: Value = serde_json::from_str(&text)?;
    if !value.is_object() {
        anyhow::bail!("contribution payload must be a JSON object")
    }
    Ok(value)
}

fn print_json(value: &impl Serialize) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

const fn all_scopes() -> [ScopeArg; 4] {
    [
        ScopeArg::Contribute,
        ScopeArg::Ack,
        ScopeArg::SourceReview,
        ScopeArg::Work,
    ]
}

async fn shutdown_signal() {
    if signal::ctrl_c().await.is_err() {
        tracing::error!("failed to install Ctrl+C handler");
    }
}
