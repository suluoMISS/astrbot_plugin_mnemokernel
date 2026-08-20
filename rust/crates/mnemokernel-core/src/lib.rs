//! Pure, deterministic domain rules for MnemoKernel.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use thiserror::Error;

pub const PROTOCOL_VERSION: &str = "mnemokernel.v1";
pub const SCHEMA_VERSION: i64 = 4;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ValidationError {
    #[error("unsupported protocol: {0}")]
    UnsupportedProtocol(String),
    #[error("{field} must not be empty")]
    Empty { field: &'static str },
    #[error("{field} exceeds {maximum} characters")]
    TooLong { field: &'static str, maximum: usize },
    #[error("{field} exceeds {maximum} items")]
    TooMany { field: &'static str, maximum: usize },
    #[error("{field} must be between 0 and 1")]
    UnitInterval { field: &'static str },
    #[error("{field} must be a 64-character lowercase hexadecimal ID")]
    InvalidId { field: &'static str },
    #[error("{field} must be a JSON object")]
    ExpectedObject { field: &'static str },
    #[error("origin_kind and sender_is_bot are inconsistent")]
    InconsistentOrigin,
    #[error("timestamps must be non-negative")]
    NegativeTimestamp,
    #[error("at least one evidence event is required")]
    MissingEvidence,
    #[error("operation {operation:?} requires memory_id")]
    MissingMemoryId { operation: OperationKind },
    #[error("create operations cannot provide memory_id")]
    UnexpectedMemoryId,
    #[error("evidence event is outside the admitted evidence set: {0}")]
    EvidenceOutsideAdmission(String),
    #[error("invalid memory state transition: {from:?} -> {to:?}")]
    InvalidStateTransition { from: MemoryState, to: MemoryState },
    #[error("invalid journal date")]
    InvalidJournalDate,
    #[error("journal time range is invalid")]
    InvalidJournalRange,
    #[error("journal proposal scope does not match request scope")]
    JournalScopeMismatch,
}

fn require_text(field: &'static str, value: &str, maximum: usize) -> Result<(), ValidationError> {
    if value.trim().is_empty() {
        return Err(ValidationError::Empty { field });
    }
    if value.chars().count() > maximum {
        return Err(ValidationError::TooLong { field, maximum });
    }
    Ok(())
}

fn require_protocol(protocol: &str) -> Result<(), ValidationError> {
    if protocol == PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(ValidationError::UnsupportedProtocol(protocol.to_owned()))
    }
}

fn require_digest(field: &'static str, value: &str) -> Result<(), ValidationError> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(ValidationError::InvalidId { field })
    }
}

fn empty_object() -> Value {
    Value::Object(Default::default())
}

fn digest(parts: &[&[u8]]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part);
    }
    hex::encode(hasher.finalize())
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScopeKind {
    Private,
    Group,
}

impl ScopeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Private => "private",
            Self::Group => "group",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OriginKind {
    UserMessage,
    BotResponse,
}

impl OriginKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UserMessage => "user_message",
            Self::BotResponse => "bot_response",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OccurredTimeSource {
    Platform,
    ObservedFallback,
}

impl OccurredTimeSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Platform => "platform",
            Self::ObservedFallback => "observed_fallback",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlatformMessageIdState {
    Present,
    Missing,
}

impl PlatformMessageIdState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Present => "present",
            Self::Missing => "missing",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ScopeDescriptor {
    pub platform_id: String,
    pub bot_account_id: String,
    pub kind: ScopeKind,
    pub session_id: String,
    pub persona_id: String,
}

impl ScopeDescriptor {
    pub fn validate(&self) -> Result<(), ValidationError> {
        require_text("platform_id", &self.platform_id, 128)?;
        require_text("bot_account_id", &self.bot_account_id, 256)?;
        require_text("session_id", &self.session_id, 512)?;
        require_text("persona_id", &self.persona_id, 256)
    }

    pub fn id(&self) -> Result<String, ValidationError> {
        self.validate()?;
        Ok(digest(&[
            PROTOCOL_VERSION.as_bytes(),
            self.platform_id.as_bytes(),
            self.bot_account_id.as_bytes(),
            self.kind.as_str().as_bytes(),
            self.session_id.as_bytes(),
            self.persona_id.as_bytes(),
        ]))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawEventInput {
    pub protocol: String,
    pub origin_kind: OriginKind,
    pub occurred_time_source: OccurredTimeSource,
    pub platform_message_id_state: PlatformMessageIdState,
    pub source_key: String,
    pub scope: ScopeDescriptor,
    pub sender_account_id: String,
    pub sender_display_name: String,
    pub sender_is_bot: bool,
    pub observed_at_ms: i64,
    pub occurred_at_ms: i64,
    pub content: String,
    pub reply_to_source_key: Option<String>,
    pub mentions: Vec<String>,
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PurgeScopeRequest {
    pub protocol: String,
    pub scope: ScopeDescriptor,
    pub actor_id: String,
    pub reason: String,
}

impl PurgeScopeRequest {
    pub fn validate(&self) -> Result<(), ValidationError> {
        require_protocol(&self.protocol)?;
        self.scope.validate()?;
        require_text("actor_id", &self.actor_id, 512)?;
        require_text("reason", &self.reason, 2_000)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CapturePolicyAction {
    Pause,
    Resume,
}

impl CapturePolicyAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pause => "pause",
            Self::Resume => "resume",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapturePolicyRequest {
    pub protocol: String,
    pub scope: ScopeDescriptor,
    pub action: CapturePolicyAction,
    pub actor_id: String,
    pub reason: String,
}

impl CapturePolicyRequest {
    pub fn validate(&self) -> Result<(), ValidationError> {
        require_protocol(&self.protocol)?;
        self.scope.validate()?;
        require_text("actor_id", &self.actor_id, 512)?;
        require_text("reason", &self.reason, 2_000)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetentionRequest {
    pub protocol: String,
    pub scope: ScopeDescriptor,
    pub cutoff_at_ms: i64,
    pub actor_id: String,
    pub reason: String,
}

impl RetentionRequest {
    pub fn validate(&self) -> Result<(), ValidationError> {
        require_protocol(&self.protocol)?;
        self.scope.validate()?;
        if self.cutoff_at_ms < 0 {
            return Err(ValidationError::NegativeTimestamp);
        }
        require_text("actor_id", &self.actor_id, 512)?;
        require_text("reason", &self.reason, 2_000)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopeStatsRequest {
    pub protocol: String,
    pub scope: ScopeDescriptor,
}

impl ScopeStatsRequest {
    pub fn validate(&self) -> Result<(), ValidationError> {
        require_protocol(&self.protocol)?;
        self.scope.validate()
    }
}

pub fn privacy_operation_id(scope_id: &str, actor_id: &str, nonce: u128) -> String {
    digest(&[
        b"privacy_operation",
        scope_id.as_bytes(),
        actor_id.as_bytes(),
        &nonce.to_be_bytes(),
    ])
}

impl RawEventInput {
    pub fn validate(&self) -> Result<(), ValidationError> {
        require_protocol(&self.protocol)?;
        self.scope.validate()?;
        require_text("source_key", &self.source_key, 512)?;
        require_text("sender_account_id", &self.sender_account_id, 512)?;
        if self.sender_is_bot != (self.origin_kind == OriginKind::BotResponse) {
            return Err(ValidationError::InconsistentOrigin);
        }
        if self.sender_display_name.chars().count() > 512 {
            return Err(ValidationError::TooLong {
                field: "sender_display_name",
                maximum: 512,
            });
        }
        require_text("content", &self.content, 100_000)?;
        if self.observed_at_ms < 0 || self.occurred_at_ms < 0 {
            return Err(ValidationError::NegativeTimestamp);
        }
        if let Some(reply) = &self.reply_to_source_key
            && reply.chars().count() > 512
        {
            return Err(ValidationError::TooLong {
                field: "reply_to_source_key",
                maximum: 512,
            });
        }
        if self.mentions.len() > 64 {
            return Err(ValidationError::TooMany {
                field: "mentions",
                maximum: 64,
            });
        }
        if !self.metadata.is_object() {
            return Err(ValidationError::ExpectedObject { field: "metadata" });
        }
        Ok(())
    }

    pub fn scope_id(&self) -> Result<String, ValidationError> {
        self.scope.id()
    }

    pub fn event_id(&self) -> Result<String, ValidationError> {
        self.validate()?;
        let scope_id = self.scope_id()?;
        Ok(digest(&[scope_id.as_bytes(), self.source_key.as_bytes()]))
    }

    pub fn content_hash(&self) -> String {
        digest(&[self.content.as_bytes()])
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NeedType {
    Fact,
    Preference,
    Decision,
    Commitment,
    Relationship,
    Episode,
    Procedure,
}

impl NeedType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fact => "fact",
            Self::Preference => "preference",
            Self::Decision => "decision",
            Self::Commitment => "commitment",
            Self::Relationship => "relationship",
            Self::Episode => "episode",
            Self::Procedure => "procedure",
        }
    }
}

pub type MemoryKind = NeedType;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecallDepth {
    Glance,
    Focused,
    Deep,
}

impl RecallDepth {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Glance => "glance",
            Self::Focused => "focused",
            Self::Deep => "deep",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecallRequest {
    pub protocol: String,
    pub scope: ScopeDescriptor,
    pub need_type: NeedType,
    pub reason: String,
    pub cues: Vec<String>,
    pub depth: RecallDepth,
    pub time_hint: Option<String>,
}

impl RecallRequest {
    pub fn validate(&self) -> Result<(), ValidationError> {
        require_protocol(&self.protocol)?;
        self.scope.validate()?;
        require_text("reason", &self.reason, 2_000)?;
        if self.cues.is_empty() {
            return Err(ValidationError::Empty { field: "cues" });
        }
        if self.cues.len() > 24 {
            return Err(ValidationError::TooMany {
                field: "cues",
                maximum: 24,
            });
        }
        let mut unique = HashSet::new();
        for cue in &self.cues {
            require_text("cue", cue, 256)?;
            if !unique.insert(cue.trim()) {
                return Err(ValidationError::Empty {
                    field: "duplicate cue",
                });
            }
        }
        if let Some(hint) = &self.time_hint
            && hint.chars().count() > 256
        {
            return Err(ValidationError::TooLong {
                field: "time_hint",
                maximum: 256,
            });
        }
        Ok(())
    }
}

fn require_journal_date(value: &str) -> Result<(), ValidationError> {
    let bytes = value.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes
            .iter()
            .enumerate()
            .any(|(index, byte)| index != 4 && index != 7 && !byte.is_ascii_digit())
    {
        return Err(ValidationError::InvalidJournalDate);
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DailyJournalRequest {
    pub protocol: String,
    pub scope: ScopeDescriptor,
    pub journal_date: String,
    pub occurred_from_ms: i64,
    pub occurred_to_ms: i64,
}

impl DailyJournalRequest {
    pub fn validate(&self) -> Result<(), ValidationError> {
        require_protocol(&self.protocol)?;
        self.scope.validate()?;
        require_journal_date(&self.journal_date)?;
        if self.occurred_from_ms < 0
            || self.occurred_to_ms <= self.occurred_from_ms
            || self.occurred_to_ms - self.occurred_from_ms > 172_800_000
        {
            return Err(ValidationError::InvalidJournalRange);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JournalClaimKind {
    Fact,
    Preference,
    Decision,
    Commitment,
    Relationship,
    Procedure,
    OpenLoop,
}

impl JournalClaimKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fact => "fact",
            Self::Preference => "preference",
            Self::Decision => "decision",
            Self::Commitment => "commitment",
            Self::Relationship => "relationship",
            Self::Procedure => "procedure",
            Self::OpenLoop => "open_loop",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JournalEpistemicStatus {
    AssistantInferred,
    UserConfirmed,
    Disputed,
}

impl JournalEpistemicStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AssistantInferred => "assistant_inferred",
            Self::UserConfirmed => "user_confirmed",
            Self::Disputed => "disputed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalClaimProposal {
    pub claim_kind: JournalClaimKind,
    pub claim_text: String,
    pub epistemic_status: JournalEpistemicStatus,
    pub evidence_event_ids: Vec<String>,
}

impl JournalClaimProposal {
    fn validate(&self, admitted: &HashSet<String>) -> Result<(), ValidationError> {
        require_text("claim_text", &self.claim_text, 2_000)?;
        if self.evidence_event_ids.is_empty() {
            return Err(ValidationError::MissingEvidence);
        }
        if self.evidence_event_ids.len() > 16 {
            return Err(ValidationError::TooMany {
                field: "evidence_event_ids",
                maximum: 16,
            });
        }
        let mut unique = HashSet::new();
        for event_id in &self.evidence_event_ids {
            require_digest("evidence_event_id", event_id)?;
            if !unique.insert(event_id) {
                return Err(ValidationError::Empty {
                    field: "duplicate evidence_event_id",
                });
            }
            if !admitted.contains(event_id) {
                return Err(ValidationError::EvidenceOutsideAdmission(event_id.clone()));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DailyJournalProposal {
    pub protocol: String,
    pub proposal_id: String,
    pub scope_id: String,
    pub journal_date: String,
    pub occurred_from_ms: i64,
    pub occurred_to_ms: i64,
    pub title: String,
    pub summary: String,
    pub cues: Vec<String>,
    pub open_loops: Vec<String>,
    pub claims: Vec<JournalClaimProposal>,
}

impl DailyJournalProposal {
    pub fn validate_for(
        &self,
        request: &DailyJournalRequest,
        admitted: &HashSet<String>,
    ) -> Result<(), ValidationError> {
        require_protocol(&self.protocol)?;
        require_text("proposal_id", &self.proposal_id, 128)?;
        require_digest("scope_id", &self.scope_id)?;
        let request_scope_id = request.scope.id()?;
        if self.scope_id != request_scope_id {
            return Err(ValidationError::JournalScopeMismatch);
        }
        if self.journal_date != request.journal_date
            || self.occurred_from_ms != request.occurred_from_ms
            || self.occurred_to_ms != request.occurred_to_ms
        {
            return Err(ValidationError::InvalidJournalRange);
        }
        require_text("title", &self.title, 200)?;
        require_text("summary", &self.summary, 6_000)?;
        if self.cues.len() > 16 {
            return Err(ValidationError::TooMany {
                field: "cues",
                maximum: 16,
            });
        }
        for cue in &self.cues {
            require_text("cue", cue, 256)?;
        }
        if self.open_loops.len() > 16 {
            return Err(ValidationError::TooMany {
                field: "open_loops",
                maximum: 16,
            });
        }
        for open_loop in &self.open_loops {
            require_text("open_loop", open_loop, 500)?;
        }
        if self.claims.len() > 32 {
            return Err(ValidationError::TooMany {
                field: "claims",
                maximum: 32,
            });
        }
        for claim in &self.claims {
            claim.validate(admitted)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryState {
    Candidate,
    Active,
    Superseded,
    Archived,
}

impl MemoryState {
    pub fn allows(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Candidate, Self::Active)
                | (Self::Candidate, Self::Archived)
                | (Self::Active, Self::Superseded)
                | (Self::Active, Self::Archived)
                | (Self::Superseded, Self::Archived)
        ) || self == next
    }

    pub fn require_transition(self, next: Self) -> Result<(), ValidationError> {
        if self.allows(next) {
            Ok(())
        } else {
            Err(ValidationError::InvalidStateTransition {
                from: self,
                to: next,
            })
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    Create,
    Strengthen,
    Revise,
    Supersede,
    Archive,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposalOperation {
    pub op: OperationKind,
    pub memory_id: Option<String>,
    pub memory_kind: MemoryKind,
    pub statement: String,
    pub confidence: Option<f64>,
    pub importance: Option<f64>,
    pub evidence_event_ids: Vec<String>,
    pub supersedes_memory_id: Option<String>,
    #[serde(default = "empty_object")]
    pub qualifiers: Value,
}

impl ProposalOperation {
    pub fn validate(&self, admitted: &HashSet<String>) -> Result<(), ValidationError> {
        require_text("statement", &self.statement, 2_000)?;
        if self.op == OperationKind::Create && self.memory_id.is_some() {
            return Err(ValidationError::UnexpectedMemoryId);
        }
        if self.op != OperationKind::Create && self.memory_id.is_none() {
            return Err(ValidationError::MissingMemoryId { operation: self.op });
        }
        if let Some(memory_id) = &self.memory_id {
            require_digest("memory_id", memory_id)?;
        }
        if let Some(memory_id) = &self.supersedes_memory_id {
            require_digest("supersedes_memory_id", memory_id)?;
        }
        if !self.qualifiers.is_object() {
            return Err(ValidationError::ExpectedObject {
                field: "qualifiers",
            });
        }
        if self.evidence_event_ids.is_empty() {
            return Err(ValidationError::MissingEvidence);
        }
        if self.evidence_event_ids.len() > 64 {
            return Err(ValidationError::TooMany {
                field: "evidence_event_ids",
                maximum: 64,
            });
        }
        for evidence_id in &self.evidence_event_ids {
            require_digest("evidence_event_id", evidence_id)?;
            if !admitted.contains(evidence_id) {
                return Err(ValidationError::EvidenceOutsideAdmission(
                    evidence_id.clone(),
                ));
            }
        }
        for (field, value) in [
            ("confidence", self.confidence),
            ("importance", self.importance),
        ] {
            if value.is_some_and(|number| !(0.0..=1.0).contains(&number)) {
                return Err(ValidationError::UnitInterval { field });
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryProposal {
    pub protocol: String,
    pub proposal_id: String,
    pub scope_id: String,
    pub worker_id: String,
    pub operations: Vec<ProposalOperation>,
}

impl MemoryProposal {
    pub fn validate(&self, admitted: &HashSet<String>) -> Result<(), ValidationError> {
        require_protocol(&self.protocol)?;
        require_text("proposal_id", &self.proposal_id, 128)?;
        require_digest("scope_id", &self.scope_id)?;
        require_text("worker_id", &self.worker_id, 256)?;
        if self.operations.is_empty() {
            return Err(ValidationError::Empty {
                field: "operations",
            });
        }
        if self.operations.len() > 64 {
            return Err(ValidationError::TooMany {
                field: "operations",
                maximum: 64,
            });
        }
        for operation in &self.operations {
            operation.validate(admitted)?;
        }
        Ok(())
    }
}

pub fn recall_id(scope_id: &str, request_json: &str, nonce: u128) -> String {
    digest(&[
        scope_id.as_bytes(),
        request_json.as_bytes(),
        &nonce.to_be_bytes(),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope() -> ScopeDescriptor {
        ScopeDescriptor {
            platform_id: "test".into(),
            bot_account_id: "bot-1".into(),
            kind: ScopeKind::Private,
            session_id: "user-1".into(),
            persona_id: "default".into(),
        }
    }

    #[test]
    fn scope_ids_are_stable_and_separated() {
        let first = scope().id().unwrap();
        assert_eq!(first, scope().id().unwrap());
        let mut other = scope();
        other.persona_id = "other".into();
        assert_ne!(first, other.id().unwrap());
        assert_eq!(first.len(), 64);
    }

    #[test]
    fn terminal_states_cannot_be_reactivated() {
        assert!(
            MemoryState::Active
                .require_transition(MemoryState::Superseded)
                .is_ok()
        );
        assert!(
            MemoryState::Superseded
                .require_transition(MemoryState::Active)
                .is_err()
        );
        assert!(
            MemoryState::Archived
                .require_transition(MemoryState::Active)
                .is_err()
        );
    }

    #[test]
    fn proposal_cannot_invent_evidence() {
        let admitted = HashSet::from(["a".repeat(64)]);
        let operation = ProposalOperation {
            op: OperationKind::Create,
            memory_id: None,
            memory_kind: MemoryKind::Fact,
            statement: "The user prefers tea.".into(),
            confidence: Some(0.8),
            importance: Some(0.4),
            evidence_event_ids: vec!["b".repeat(64)],
            supersedes_memory_id: None,
            qualifiers: Value::Object(Default::default()),
        };
        assert!(matches!(
            operation.validate(&admitted),
            Err(ValidationError::EvidenceOutsideAdmission(_))
        ));
    }
}
