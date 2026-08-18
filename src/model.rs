use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DelegationRevocation {
    pub protocol: String,
    pub lineage_id: String,
    pub delegation_id: String,
    pub reason: String,
    pub created_at: DateTime<Utc>,
    pub nonce: String,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyRotationStatement {
    pub protocol: String,
    pub lineage_id: String,
    pub previous_public_key: String,
    pub new_public_key: String,
    pub revoke_existing_delegations: bool,
    pub reason: String,
    pub created_at: DateTime<Utc>,
    pub nonce: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedKeyRotation {
    pub statement: KeyRotationStatement,
    pub previous_signature: String,
    pub new_signature: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum Scope {
    Contribute,
    Ack,
    SourceReview,
    Work,
    Forum,
    DirectMessage,
}

impl Scope {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Contribute => "contribute",
            Self::Ack => "ack",
            Self::SourceReview => "source-review",
            Self::Work => "work",
            Self::Forum => "forum",
            Self::DirectMessage => "direct-message",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContributionKind {
    SourceProposal,
    SourceReview,
    VerificationTrace,
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
    #[serde(rename = "openpgp_key")]
    OpenPgpKey,
    DirectMessage,
}

impl ContributionKind {
    pub const fn required_scope(&self) -> Scope {
        match self {
            Self::SourceReview => Scope::SourceReview,
            Self::WorkClaim | Self::WorkResult => Scope::Work,
            Self::TopicProposal | Self::TopicVote | Self::ForumPost => Scope::Forum,
            Self::OpenPgpKey | Self::DirectMessage => Scope::DirectMessage,
            _ => Scope::Contribute,
        }
    }

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::SourceProposal => "source_proposal",
            Self::SourceReview => "source_review",
            Self::VerificationTrace => "verification_trace",
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
            Self::TopicProposal => "topic_proposal",
            Self::TopicVote => "topic_vote",
            Self::ForumPost => "forum_post",
            Self::OpenPgpKey => "openpgp_key",
            Self::DirectMessage => "direct_message",
        }
    }

    pub const fn requires_traceable_reporting(&self) -> bool {
        matches!(
            self,
            Self::SourceReview
                | Self::ObservationVerification
                | Self::StoryLink
                | Self::Assessment
                | Self::Correction
                | Self::WorkResult
        )
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReportingMode {
    #[default]
    Unverified,
    Traceable,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReportingDeclaration {
    #[serde(default)]
    pub mode: ReportingMode,
    #[serde(default)]
    pub trace_event_ids: Vec<String>,
}

impl ReportingDeclaration {
    pub fn is_unverified(&self) -> bool {
        self.mode == ReportingMode::Unverified && self.trace_event_ids.is_empty()
    }

    pub fn is_traceable(&self) -> bool {
        self.mode == ReportingMode::Traceable && !self.trace_event_ids.is_empty()
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
    #[serde(default, skip_serializing_if = "ReportingDeclaration::is_unverified")]
    pub reporting: ReportingDeclaration,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TraceOutcome {
    Passed,
    Failed,
    Inconclusive,
}

impl TraceOutcome {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Inconclusive => "inconclusive",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerificationTool {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invocation: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerificationArtifact {
    pub name: String,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerificationCheck {
    pub name: String,
    pub outcome: TraceOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<Value>,
    pub observed: Value,
    #[serde(default)]
    pub evidence: Vec<EvidenceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerificationTracePayload {
    pub subject_id: String,
    pub assertion: String,
    pub method: String,
    pub outcome: TraceOutcome,
    pub started_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
    #[serde(default)]
    pub tools: Vec<VerificationTool>,
    pub checks: Vec<VerificationCheck>,
    #[serde(default)]
    pub evidence: Vec<EvidenceRef>,
    #[serde(default)]
    pub artifacts: Vec<VerificationArtifact>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_digest: Option<String>,
    #[serde(default)]
    pub parent_trace_event_ids: Vec<String>,
    #[serde(default)]
    pub limitations: Vec<String>,
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
pub struct TopicProposalPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_topic_id: Option<String>,
    pub slug: String,
    pub title: String,
    pub summary: String,
    pub charter: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default = "default_archive_after_days")]
    pub archive_after_days: u32,
}

const fn default_archive_after_days() -> u32 {
    90
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum TopicVoteChoice {
    Approve,
    Reject,
    NeedsRevision,
}

impl TopicVoteChoice {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::Reject => "reject",
            Self::NeedsRevision => "needs_revision",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TopicVotePayload {
    pub topic_id: String,
    pub choice: TopicVoteChoice,
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForumPostPayload {
    pub topic_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_post_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    pub body: String,
    pub language: String,
    #[serde(default)]
    pub mentions: Vec<String>,
    #[serde(default)]
    pub references: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OpenPgpKeyAction {
    Publish,
    Revoke,
}

impl OpenPgpKeyAction {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Publish => "publish",
            Self::Revoke => "revoke",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenPgpKeyPayload {
    pub action: OpenPgpKeyAction,
    pub fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub armored_public_key: Option<String>,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectMessagePayload {
    pub recipient_lineage_id: String,
    pub recipient_key_fingerprint: String,
    pub ciphertext_format: String,
    pub ciphertext: String,
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
    pub key_version: i64,
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
    /// Exact authenticated protocol object used to derive the event id and
    /// node hash. Projection fields above are conveniences, not a substitute
    /// for this object when independently verifying provenance.
    pub canonical: Value,
    pub author_signature: Option<String>,
    pub previous_hash: String,
    pub event_hash: String,
    pub node_signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Checkpoint {
    pub node_id: String,
    pub node_public_key: String,
    pub cursor: i64,
    pub event_hash: String,
    pub created_at: DateTime<Utc>,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OriginEvent {
    pub sequence: i64,
    pub event_id: String,
    pub kind: String,
    pub lineage_id: Option<String>,
    pub delegation_id: Option<String>,
    pub created_at: String,
    pub received_at: String,
    pub canonical: Value,
    pub previous_hash: String,
    pub event_hash: String,
    pub node_signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationTraceView {
    pub origin_node_id: String,
    pub origin_node_public_key: String,
    pub event: OriginEvent,
    pub trace: VerificationTracePayload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationTracePage {
    pub traces: Vec<VerificationTraceView>,
    pub after: i64,
    pub next_cursor: i64,
    pub has_more: bool,
    pub origin_node_id: Option<String>,
    pub subject_id: Option<String>,
    pub provenance_notice: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBundle {
    pub protocol: String,
    pub origin_node_id: String,
    pub origin_node_public_key: String,
    pub from_cursor: i64,
    pub through_cursor: i64,
    pub events: Vec<OriginEvent>,
    pub checkpoint: Checkpoint,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointWitness {
    pub protocol: String,
    pub witness_node_id: String,
    pub witness_node_public_key: String,
    pub origin_node_id: String,
    pub origin_node_public_key: String,
    pub cursor: i64,
    pub event_hash: String,
    pub origin_checkpoint: Checkpoint,
    pub observed_at: DateTime<Utc>,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederationImportReport {
    pub origin_node_id: String,
    pub previously_known_cursor: i64,
    pub imported_events: usize,
    pub current_cursor: i64,
    pub current_event_hash: String,
    pub witness_event_id: Option<String>,
}

/// A relay's attributable claim that it retained one exact signed origin head.
///
/// This proves the relay made the claim; it does not prove future availability.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplicationReceipt {
    pub protocol: String,
    pub relay_node_id: String,
    pub relay_node_public_key: String,
    pub origin_checkpoint: Checkpoint,
    pub retained_at: DateTime<Utc>,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederationPublishReport {
    pub import: FederationImportReport,
    pub receipt: ReplicationReceipt,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicationTargetView {
    pub endpoint: String,
    pub relay_node_id: Option<String>,
    pub relay_node_public_key: Option<String>,
    pub acknowledged_cursor: i64,
    pub acknowledged_event_hash: String,
    pub at_current_head: bool,
    pub recently_reconfirmed: bool,
    pub last_attempt_at: Option<DateTime<Utc>>,
    pub last_success_at: Option<DateTime<Utc>>,
    pub consecutive_failures: u32,
    pub next_attempt_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub receipt: Option<ReplicationReceipt>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicationHealth {
    pub generated_at: DateTime<Utc>,
    pub origin_node_id: String,
    pub current_cursor: i64,
    pub current_event_hash: String,
    pub desired_replicas: u32,
    pub confirmed_current_replicas: usize,
    pub recently_reconfirmed_current_replicas: usize,
    pub status: String,
    pub targets: Vec<PublicationTargetView>,
    pub receipt_notice: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederationPeerView {
    pub node_id: String,
    pub node_public_key: String,
    pub first_seen_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub cursor: i64,
    pub event_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EquivocationEvidenceView {
    pub evidence_id: String,
    pub origin_node_id: String,
    pub conflict_kind: String,
    pub cursor: i64,
    pub existing_hash: String,
    pub incoming_hash: String,
    pub existing: Value,
    pub incoming: Value,
    pub detected_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederatedStoryView {
    pub origin_node_id: String,
    pub story_id: String,
    pub title: String,
    pub first_seen_at: DateTime<Utc>,
    pub stage: String,
    pub observations: Vec<ObservationView>,
    pub assessments: Vec<AssessmentView>,
    pub related_events: Vec<OriginEvent>,
    pub reporting_notice: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkFeed {
    pub local: FeedPage,
    pub federated: FederatedFeedPage,
    pub provenance_notice: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederatedFeedPage {
    /// Present when this is a stable, cursor-paginable page for one origin.
    pub origin_node_id: Option<String>,
    pub stories: Vec<FederatedStoryView>,
    /// Origin sequence cursor. Aggregate multi-origin previews deliberately do
    /// not invent a global cursor and therefore return `None` here.
    pub after: Option<i64>,
    pub next_cursor: Option<i64>,
    pub has_more: bool,
    pub pagination_notice: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverageGapView {
    pub coverage_area: String,
    pub eligible_sources: usize,
    pub proposed_sources: usize,
    pub status: String,
    pub standing_work_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OwnershipConcentrationView {
    pub ownership_label: String,
    pub source_manifests: usize,
    pub eligible_source_manifests: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverageReport {
    pub generated_at: DateTime<Utc>,
    pub local_source_manifests: usize,
    pub federated_source_manifests: usize,
    pub eligible_source_manifests: usize,
    pub by_status: BTreeMap<String, usize>,
    pub by_region_or_coverage_tag: BTreeMap<String, usize>,
    pub by_language: BTreeMap<String, usize>,
    pub by_medium: BTreeMap<String, usize>,
    pub by_ownership: BTreeMap<String, usize>,
    pub missing_ownership_manifests: usize,
    pub dominant_ownership: Option<OwnershipConcentrationView>,
    pub standing_gaps: Vec<CoverageGapView>,
    pub methodology_notice: String,
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
    pub federated_world_changes: Vec<FederatedStoryView>,
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
    /// Reviews carrying prior signed verification-trace event references.
    pub approval_count: i64,
    pub rejection_count: i64,
    pub untraced_approval_count: i64,
    pub untraced_rejection_count: i64,
    pub successful_fetches: i64,
    pub consecutive_failures: i64,
    pub last_fetched_at: Option<DateTime<Utc>>,
    pub reporting_notice: String,
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
    /// Only traceable reports contribute to these two verification counts.
    pub corroborated_count: i64,
    pub disputed_count: i64,
    pub untraced_corroborated_count: i64,
    pub untraced_disputed_count: i64,
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
    pub reporting: ReportingDeclaration,
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
    pub reporting_notice: String,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkPage {
    pub items: Vec<WorkItemView>,
    pub after: Option<String>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
    pub kind: Option<String>,
    pub reporting_notice: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TopicVoteTally {
    pub approvals: usize,
    pub rejections: usize,
    pub needs_revision: usize,
    pub conflicted_lineages: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopicVoteView {
    pub voter_lineage_id: String,
    pub origin_node_id: String,
    pub origin_sequence: i64,
    pub event_id: String,
    pub choice: String,
    pub rationale: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopicView {
    pub topic_id: String,
    pub origin_node_id: String,
    pub proposal_event_id: String,
    pub proposer_lineage_id: String,
    pub parent_topic_id: Option<String>,
    pub slug: String,
    pub title: String,
    pub summary: String,
    pub charter: String,
    pub tags: Vec<String>,
    pub languages: Vec<String>,
    pub archive_after_days: u32,
    pub created_at: DateTime<Utc>,
    pub last_activity_at: DateTime<Utc>,
    pub status: String,
    pub tally: TopicVoteTally,
    pub votes: Vec<TopicVoteView>,
    pub post_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopicPage {
    pub topics: Vec<TopicView>,
    pub after: Option<String>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
    pub selection_notice: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForumPostView {
    pub projection_sequence: i64,
    pub post_id: String,
    pub origin_node_id: String,
    pub origin_sequence: i64,
    pub event_id: String,
    pub topic_id: String,
    pub parent_post_id: Option<String>,
    pub author_lineage_id: String,
    pub subject: Option<String>,
    pub body: String,
    pub language: String,
    pub mentions: Vec<String>,
    pub references: Vec<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForumPostPage {
    pub posts: Vec<ForumPostView>,
    pub after: i64,
    pub next_cursor: i64,
    pub has_more: bool,
    pub ordering_notice: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenPgpKeyView {
    pub lineage_id: String,
    pub origin_node_id: String,
    pub fingerprint: String,
    pub event_id: String,
    pub action: String,
    pub armored_public_key: Option<String>,
    pub note: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectMessageView {
    pub projection_sequence: i64,
    pub message_id: String,
    pub origin_node_id: String,
    pub origin_sequence: i64,
    pub event_id: String,
    pub sender_lineage_id: String,
    pub recipient_lineage_id: String,
    pub recipient_key_fingerprint: String,
    pub ciphertext_format: String,
    pub ciphertext: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectMessagePage {
    pub messages: Vec<DirectMessageView>,
    pub after: i64,
    pub next_cursor: i64,
    pub has_more: bool,
    pub privacy_notice: String,
}
