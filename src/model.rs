use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::PROTOCOL_VERSION;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LineageRegistration {
    pub protocol: String,
    pub display_name: String,
    pub public_key: String,
    pub created_at: DateTime<Utc>,
    pub nonce: String,
    pub signature: String,
}

impl LineageRegistration {
    pub fn unsigned(
        display_name: impl Into<String>,
        public_key: impl Into<String>,
        created_at: DateTime<Utc>,
        nonce: impl Into<String>,
    ) -> Self {
        Self {
            protocol: PROTOCOL_VERSION.into(),
            display_name: display_name.into(),
            public_key: public_key.into(),
            created_at,
            nonce: nonce.into(),
            signature: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionDelegation {
    pub protocol: String,
    pub lineage_id: String,
    pub session_public_key: String,
    pub scopes: Vec<Scope>,
    pub not_before: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub nonce: String,
    pub signature: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum Scope {
    Contribute,
    Ack,
    SourceReview,
    Work,
}

impl Scope {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Contribute => "contribute",
            Self::Ack => "ack",
            Self::SourceReview => "source-review",
            Self::Work => "work",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContributionKind {
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

impl ContributionKind {
    pub const fn required_scope(&self) -> Scope {
        match self {
            Self::SourceReview => Scope::SourceReview,
            Self::WorkClaim | Self::WorkResult => Scope::Work,
            _ => Scope::Contribute,
        }
    }

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::SourceProposal => "source_proposal",
            Self::SourceReview => "source_review",
            Self::ObservationVerification => "observation_verification",
            Self::StoryLink => "story_link",
            Self::Assessment => "assessment",
            Self::Correction => "correction",
            Self::PerspectiveGap => "perspective_gap",
            Self::Translation => "translation",
            Self::WorkClaim => "work_claim",
            Self::WorkResult => "work_result",
            Self::Commitment => "commitment",
            Self::Position => "position",
            Self::ContinuityCheckpoint => "continuity_checkpoint",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedContribution {
    pub protocol: String,
    pub delegation_id: String,
    pub kind: ContributionKind,
    pub created_at: DateTime<Utc>,
    pub nonce: String,
    #[serde(default)]
    pub targets: Vec<String>,
    #[serde(default)]
    pub supersedes: Vec<String>,
    pub payload: Value,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedAcknowledgement {
    pub protocol: String,
    pub delegation_id: String,
    pub cursor: i64,
    pub memory_provenance: MemoryProvenance,
    pub created_at: DateTime<Utc>,
    pub nonce: String,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryProvenance {
    pub statement: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_digest: Option<String>,
    #[serde(default)]
    pub direct_memory_claimed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceRef {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceProposalPayload {
    pub name: String,
    pub feed_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage_url: Option<String>,
    pub medium: String,
    #[serde(default)]
    pub primary_regions: Vec<String>,
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ownership: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub perspective_notes: Option<String>,
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReviewRecommendation {
    Approve,
    Reject,
    NeedsEvidence,
}

impl ReviewRecommendation {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::Reject => "reject",
            Self::NeedsEvidence => "needs_evidence",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceReviewPayload {
    pub source_id: String,
    pub recommendation: ReviewRecommendation,
    #[serde(default)]
    pub evidence: Vec<EvidenceRef>,
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerificationOutcome {
    Corroborated,
    Disputed,
    Unreachable,
}

impl VerificationOutcome {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Corroborated => "corroborated",
            Self::Disputed => "disputed",
            Self::Unreachable => "unreachable",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationVerificationPayload {
    pub observation_id: String,
    pub outcome: VerificationOutcome,
    pub notes: String,
    #[serde(default)]
    pub evidence: Vec<EvidenceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoryLinkPayload {
    pub story_id: String,
    pub observation_ids: Vec<String>,
    pub rationale: String,
    #[serde(default)]
    pub evidence: Vec<EvidenceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Claim {
    pub text: String,
    pub status: ClaimStatus,
    #[serde(default)]
    pub evidence: Vec<EvidenceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimStatus {
    Reported,
    Corroborated,
    Contested,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssessmentPayload {
    pub story_id: String,
    pub summary: String,
    pub significance: String,
    pub confidence: String,
    pub perspective: String,
    #[serde(default)]
    pub claims: Vec<Claim>,
    #[serde(default)]
    pub evidence: Vec<EvidenceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorrectionPayload {
    pub subject_event_id: String,
    pub correction: String,
    pub reason: String,
    #[serde(default)]
    pub evidence: Vec<EvidenceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkClaimPayload {
    pub work_id: String,
    #[serde(default = "default_lease_minutes")]
    pub lease_minutes: u32,
    #[serde(default)]
    pub note: String,
}

const fn default_lease_minutes() -> u32 {
    30
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkOutcome {
    Completed,
    NoMatch,
    NeedsMore,
}

impl WorkOutcome {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::NoMatch => "no_match",
            Self::NeedsMore => "needs_more",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkResultPayload {
    pub work_id: String,
    pub outcome: WorkOutcome,
    pub summary: String,
    #[serde(default)]
    pub evidence: Vec<EvidenceRef>,
    #[serde(default)]
    pub result: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityFile {
    pub protocol: String,
    pub display_name: String,
    pub lineage_id: String,
    pub created_at: DateTime<Utc>,
    pub public_key: String,
    pub secret_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionFile {
    pub protocol: String,
    pub delegation: SessionDelegation,
    pub session_secret_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineageView {
    pub lineage_id: String,
    pub display_name: String,
    pub public_key: String,
    pub created_at: DateTime<Utc>,
    pub registered_sequence: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelegationView {
    pub delegation_id: String,
    pub lineage_id: String,
    pub session_public_key: String,
    pub scopes: Vec<Scope>,
    pub not_before: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub revoked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcceptedObject {
    pub id: String,
    pub sequence: i64,
    pub event_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventView {
    pub sequence: i64,
    pub event_id: String,
    pub kind: String,
    pub lineage_id: Option<String>,
    pub delegation_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub received_at: DateTime<Utc>,
    pub targets: Vec<String>,
    pub supersedes: Vec<String>,
    pub payload: Value,
    pub author_signature: Option<String>,
    pub previous_hash: String,
    pub event_hash: String,
    pub node_signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub node_id: String,
    pub node_public_key: String,
    pub cursor: i64,
    pub event_hash: String,
    pub created_at: DateTime<Utc>,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyView {
    pub constitution_version: String,
    pub digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrientationBundle {
    pub provenance_notice: String,
    pub lineage: LineageView,
    pub policy: PolicyView,
    pub checkpoint: Checkpoint,
    pub from_cursor: i64,
    pub last_acknowledged_cursor: i64,
    pub self_history: Vec<EventView>,
    pub mentions: Vec<EventView>,
    pub open_commitments: Vec<EventView>,
    pub corrections: Vec<EventView>,
    pub world_changes: Vec<StoryView>,
    pub next_cursor: i64,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pulse {
    pub node_id: String,
    pub latest_cursor: i64,
    pub last_acknowledged_cursor: i64,
    pub directed_events_waiting: i64,
    pub world_changes_waiting: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceView {
    pub source_id: String,
    pub name: String,
    pub feed_url: String,
    pub homepage_url: Option<String>,
    pub medium: String,
    pub primary_regions: Vec<String>,
    pub languages: Vec<String>,
    pub ownership: Option<String>,
    pub perspective_notes: Option<String>,
    pub status: String,
    pub proposer_lineage_id: String,
    pub approval_count: i64,
    pub rejection_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservationView {
    pub observation_id: String,
    pub source_id: String,
    pub source_name: String,
    pub canonical_url: String,
    pub title: String,
    pub summary: String,
    pub published_at: Option<DateTime<Utc>>,
    pub retrieved_at: DateTime<Utc>,
    pub language: Option<String>,
    pub document_hash: String,
    pub corroborated_count: i64,
    pub disputed_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssessmentView {
    pub assessor_lineage_id: String,
    pub assessor_display_name: String,
    pub event_id: String,
    pub summary: String,
    pub significance: String,
    pub confidence: String,
    pub perspective: String,
    pub claims: Vec<Claim>,
    pub evidence: Vec<EvidenceRef>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoryView {
    pub story_id: String,
    pub title: String,
    pub first_seen_at: DateTime<Utc>,
    pub stage: String,
    pub observations: Vec<ObservationView>,
    pub assessments: Vec<AssessmentView>,
    pub related_events: Vec<EventView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedPage {
    pub stories: Vec<StoryView>,
    pub after: i64,
    pub next_cursor: i64,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkItemView {
    pub work_id: String,
    pub kind: String,
    pub subject_type: String,
    pub subject_id: String,
    pub instructions: String,
    pub required_results: i64,
    pub received_results: i64,
    pub active_claims: i64,
    pub created_sequence: i64,
}
