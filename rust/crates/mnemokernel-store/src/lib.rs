//! Transactional SQLite store. All official persistence crosses this boundary.

mod migrations;
mod redaction;

use mnemokernel_core::{
    CapturePolicyAction, CapturePolicyRequest, DailyJournalProposal, DailyJournalRequest, NeedType,
    PROTOCOL_VERSION, PurgeScopeRequest, RawEventInput, RecallDepth, RecallRequest,
    RetentionRequest, SCHEMA_VERSION, ScopeStatsRequest, ValidationError, privacy_operation_id,
    recall_id,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::Serialize;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;

use crate::migrations::MigrationError;
use crate::redaction::{redact, sanitize_metadata};

const JOURNAL_EVENT_LIMIT: usize = 96;
const JOURNAL_EVENT_CONTENT_LIMIT: usize = 2_000;
const JOURNAL_CONTENT_BUDGET: usize = 64_000;
const ACTIVATION_HALF_LIFE_MS: i64 = 30 * 24 * 60 * 60 * 1_000;
const ACTIVATION_FLOOR: f64 = 0.05;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error(transparent)]
    Validation(#[from] ValidationError),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Migration(#[from] MigrationError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("idempotency conflict for source event: {0}")]
    IdempotencyConflict(String),
    #[error("database lock was poisoned")]
    LockPoisoned,
    #[error("privacy purge left {0} readable payload rows")]
    ResidualPayloads(usize),
    #[error("journal requires at least one active source event")]
    JournalNoEvents,
}

#[derive(Debug, Serialize)]
pub struct KernelCapabilities {
    pub capture_ready: bool,
    pub episode_ready: bool,
    pub recall_ready: bool,
}

#[derive(Debug, Serialize)]
pub struct Health {
    pub status: &'static str,
    pub protocol: &'static str,
    pub schema_version: i64,
    pub capabilities: KernelCapabilities,
}

#[derive(Debug, Serialize)]
pub struct IngestOutcome {
    pub status: &'static str,
    pub event_id: String,
    pub scope_id: String,
    pub inserted: bool,
    pub redactions: usize,
}

#[derive(Debug, Serialize)]
pub struct RecallItem {
    pub memory_id: String,
    pub memory_kind: String,
    pub statement: String,
    pub confidence: f64,
    pub occurred_at_ms: i64,
    pub evidence_status: &'static str,
    pub evidence_event_ids: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct RecallOutcome {
    pub status: String,
    pub protocol: &'static str,
    pub recall_id: String,
    pub scope_id: String,
    pub reason: String,
    pub brief: Option<String>,
    pub summary: String,
    pub items: Vec<RecallItem>,
    pub uncertainties: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct PurgeOutcome {
    pub status: &'static str,
    pub operation_id: String,
    pub scope_id: String,
    pub purged_payloads: usize,
    pub purged_episodes: usize,
    pub invalidated_journals: usize,
    pub compacted: bool,
}

#[derive(Debug, Serialize)]
pub struct CapturePolicyOutcome {
    pub status: &'static str,
    pub scope_id: String,
    pub policy_state: String,
}

#[derive(Debug, Serialize)]
pub struct RetentionOutcome {
    pub status: &'static str,
    pub scope_id: String,
    pub run_id: String,
    pub cutoff_at_ms: i64,
    pub purged_payloads: usize,
    pub decayed_memories: usize,
    pub residual_scan: &'static str,
}

#[derive(Debug, Serialize)]
pub struct ScopeStatsOutcome {
    pub status: &'static str,
    pub scope_id: String,
    pub policy_state: String,
    pub raw_events: i64,
    pub active_payloads: i64,
    pub episodes: i64,
    pub active_memories: i64,
    pub superseded_memories: i64,
    pub archived_memories: i64,
    pub last_retention_cutoff_ms: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct JournalEvent {
    pub event_id: String,
    pub occurred_at_ms: i64,
    pub origin_kind: String,
    pub sender_display_name: String,
    pub reply_to_source_key: Option<String>,
    pub content: String,
    pub content_truncated: bool,
}

#[derive(Debug, Serialize)]
pub struct JournalContextOutcome {
    pub status: &'static str,
    pub scope_id: String,
    pub journal_date: String,
    pub context_truncated: bool,
    pub events: Vec<JournalEvent>,
}

#[derive(Debug, Serialize)]
pub struct JournalSaveOutcome {
    pub status: &'static str,
    pub scope_id: String,
    pub journal_date: String,
    pub episode_id: String,
    pub version: i64,
    pub event_count: usize,
    pub claim_count: usize,
}

#[derive(Debug, Serialize)]
pub struct JournalClaimView {
    pub claim_kind: String,
    pub claim_text: String,
    pub epistemic_status: String,
}

#[derive(Debug, Serialize)]
pub struct JournalReadOutcome {
    pub status: &'static str,
    pub scope_id: String,
    pub journal_date: String,
    pub episode_id: String,
    pub version: i64,
    pub title: String,
    pub summary: String,
    pub cues: Vec<String>,
    pub open_loops: Vec<String>,
    pub claims: Vec<JournalClaimView>,
}

fn unix_time() -> Duration {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
}

fn now_ms() -> i64 {
    i64::try_from(unix_time().as_millis()).unwrap_or(i64::MAX)
}

fn digest(parts: &[&[u8]]) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part);
    }
    hex::encode(hasher.finalize())
}

fn truncate_chars(value: &str, maximum: usize) -> (String, bool) {
    let mut chars = value.chars();
    let truncated: String = chars.by_ref().take(maximum).collect();
    let was_truncated = chars.next().is_some();
    (truncated, was_truncated)
}

pub struct KernelStore {
    connection: Mutex<Connection>,
}

mod inspector;

impl KernelStore {
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let connection = migrations::open_database(path)?;

        let store = Self {
            connection: Mutex::new(connection),
        };
        let version = store.schema_version()?;
        if version != SCHEMA_VERSION {
            return Err(StoreError::Sqlite(rusqlite::Error::InvalidQuery));
        }
        Ok(store)
    }

    pub fn schema_version(&self) -> Result<i64, StoreError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?;
        let version = connection.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )?;
        Ok(version)
    }

    pub fn health(&self) -> Result<Health, StoreError> {
        Ok(Health {
            status: "ok",
            protocol: PROTOCOL_VERSION,
            schema_version: self.schema_version()?,
            capabilities: KernelCapabilities {
                capture_ready: true,
                episode_ready: true,
                recall_ready: true,
            },
        })
    }

    pub fn ingest_event(&self, event: &RawEventInput) -> Result<IngestOutcome, StoreError> {
        event.validate()?;
        let scope_id = event.scope_id()?;
        let event_id = event.event_id()?;
        let content_hash = event.content_hash();
        let redaction = redact(&event.content);
        let redaction_map_json = serde_json::to_string(&redaction.spans)?;
        let mentions_json = serde_json::to_string(&event.mentions)?;
        let metadata_json = serde_json::to_string(&sanitize_metadata(&event.metadata))?;
        let ingested_at_ms = now_ms();

        let mut connection = self
            .connection
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT OR IGNORE INTO scopes(
                scope_id, platform_id, bot_account_id, conversation_kind,
                session_id, persona_id, created_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                scope_id,
                event.scope.platform_id,
                event.scope.bot_account_id,
                event.scope.kind.as_str(),
                event.scope.session_id,
                event.scope.persona_id,
                ingested_at_ms,
            ],
        )?;

        let opted_out = transaction
            .query_row(
                "SELECT 1 FROM scope_capture_policies
                 WHERE scope_id = ?1 AND policy_state = 'opted_out'",
                [&scope_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if opted_out {
            transaction.commit()?;
            return Ok(IngestOutcome {
                status: "opted_out",
                event_id,
                scope_id,
                inserted: false,
                redactions: 0,
            });
        }

        let existing: Option<(String, String)> = transaction
            .query_row(
                "SELECT event_id, content_hash FROM raw_events
                 WHERE scope_id = ?1 AND source_key = ?2",
                params![scope_id, event.source_key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;

        let inserted = if let Some((stored_event_id, stored_hash)) = existing {
            if stored_event_id != event_id || stored_hash != content_hash {
                return Err(StoreError::IdempotencyConflict(event.source_key.clone()));
            }
            false
        } else {
            transaction.execute(
                "INSERT INTO raw_events(
                    event_id, scope_id, source_key, sender_account_id,
                    sender_display_name, sender_is_bot, observed_at_ms,
                    occurred_at_ms, content, content_hash, reply_to_source_key,
                    mentions_json, metadata_json, ingested_at_ms, origin_kind,
                    occurred_time_source, platform_message_id_state
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                    ?15, ?16, ?17
                 )",
                params![
                    event_id,
                    scope_id,
                    event.source_key,
                    event.sender_account_id,
                    "",
                    if event.sender_is_bot { 1_i64 } else { 0_i64 },
                    event.observed_at_ms,
                    event.occurred_at_ms,
                    "",
                    content_hash,
                    event.reply_to_source_key,
                    "[]",
                    "{}",
                    ingested_at_ms,
                    event.origin_kind.as_str(),
                    event.occurred_time_source.as_str(),
                    event.platform_message_id_state.as_str(),
                ],
            )?;
            transaction.execute(
                "INSERT INTO raw_event_payloads(
                    event_id, payload_state, sender_display_name, content,
                    mentions_json, metadata_json, redaction_map_json,
                    created_at_ms, updated_at_ms, purged_at_ms
                 ) VALUES (?1, 'active', ?2, ?3, ?4, ?5, ?6, ?7, ?7, NULL)",
                params![
                    event_id,
                    event.sender_display_name,
                    redaction.content,
                    mentions_json,
                    metadata_json,
                    redaction_map_json,
                    ingested_at_ms,
                ],
            )?;
            true
        };
        transaction.commit()?;

        Ok(IngestOutcome {
            status: "ok",
            event_id,
            scope_id,
            inserted,
            redactions: redaction.spans.len(),
        })
    }

    pub fn journal_context(
        &self,
        request: &DailyJournalRequest,
    ) -> Result<JournalContextOutcome, StoreError> {
        request.validate()?;
        let scope_id = request.scope.id()?;
        let connection = self
            .connection
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?;
        let mut statement = connection.prepare(
            "SELECT event.event_id, event.occurred_at_ms, event.origin_kind,
                    payload.sender_display_name, event.reply_to_source_key, payload.content
             FROM raw_events AS event
             JOIN raw_event_payloads AS payload ON payload.event_id = event.event_id
             WHERE event.scope_id = ?1
               AND event.occurred_at_ms >= ?2
               AND event.occurred_at_ms < ?3
               AND payload.payload_state = 'active'
             ORDER BY event.occurred_at_ms ASC, event.event_id ASC
             LIMIT 97",
        )?;
        let rows = statement.query_map(
            params![scope_id, request.occurred_from_ms, request.occurred_to_ms],
            |row| {
                let content: String = row.get(5)?;
                let (content, content_truncated) =
                    truncate_chars(&content, JOURNAL_EVENT_CONTENT_LIMIT);
                Ok(JournalEvent {
                    event_id: row.get(0)?,
                    occurred_at_ms: row.get(1)?,
                    origin_kind: row.get(2)?,
                    sender_display_name: row.get(3)?,
                    reply_to_source_key: row.get(4)?,
                    content,
                    content_truncated,
                })
            },
        )?;
        let mut events = Vec::new();
        for row in rows {
            events.push(row?);
        }
        let window_truncated = events.len() > JOURNAL_EVENT_LIMIT;
        events.truncate(JOURNAL_EVENT_LIMIT);
        let per_event_budget = if events.is_empty() {
            0
        } else {
            (JOURNAL_CONTENT_BUDGET / events.len()).min(JOURNAL_EVENT_CONTENT_LIMIT)
        };
        let mut context_truncated = window_truncated;
        for event in &mut events {
            let (content, truncated) = truncate_chars(&event.content, per_event_budget);
            event.content = content;
            event.content_truncated |= truncated;
            context_truncated |= event.content_truncated;
        }
        let status = if events.is_empty() { "empty" } else { "ok" };
        Ok(JournalContextOutcome {
            status,
            scope_id,
            journal_date: request.journal_date.clone(),
            context_truncated,
            events,
        })
    }

    pub fn save_journal(
        &self,
        request: &DailyJournalRequest,
        proposal: &DailyJournalProposal,
        proposal_json: &str,
    ) -> Result<JournalSaveOutcome, StoreError> {
        request.validate()?;
        let scope_id = request.scope.id()?;
        let connection = self
            .connection
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?;
        let mut statement = connection.prepare(
            "SELECT event.event_id
             FROM raw_events AS event
             JOIN raw_event_payloads AS payload ON payload.event_id = event.event_id
             WHERE event.scope_id = ?1
               AND event.occurred_at_ms >= ?2
               AND event.occurred_at_ms < ?3
               AND payload.payload_state = 'active'",
        )?;
        let rows = statement.query_map(
            params![scope_id, request.occurred_from_ms, request.occurred_to_ms],
            |row| row.get::<_, String>(0),
        )?;
        let mut admitted = HashSet::new();
        for row in rows {
            admitted.insert(row?);
        }
        if admitted.is_empty() {
            return Err(StoreError::JournalNoEvents);
        }
        proposal.validate_for(request, &admitted)?;

        let proposal_digest = digest(&[proposal_json.as_bytes()]);
        let episode_id = digest(&[
            b"episode",
            scope_id.as_bytes(),
            request.journal_date.as_bytes(),
        ]);
        let job_id = digest(&[
            b"episode_job",
            scope_id.as_bytes(),
            request.journal_date.as_bytes(),
        ]);
        let completed_at_ms = now_ms();
        let transaction = connection.unchecked_transaction()?;

        transaction.execute(
            "INSERT OR IGNORE INTO scopes(
                scope_id, platform_id, bot_account_id, conversation_kind,
                session_id, persona_id, created_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                scope_id,
                request.scope.platform_id,
                request.scope.bot_account_id,
                request.scope.kind.as_str(),
                request.scope.session_id,
                request.scope.persona_id,
                completed_at_ms,
            ],
        )?;

        let existing_job: Option<(String, String)> = transaction
            .query_row(
                "SELECT job_id, state FROM episode_jobs
                 WHERE scope_id = ?1 AND idempotency_key = ?2",
                params![scope_id, request.journal_date],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let stored_job_id = existing_job
            .as_ref()
            .map(|(stored_id, _)| stored_id.clone())
            .unwrap_or_else(|| job_id.clone());
        if existing_job.is_none() {
            transaction.execute(
                "INSERT INTO episode_jobs(
                    job_id, scope_id, job_kind, idempotency_key, state, checkpoint_json,
                    lease_owner, lease_token, lease_expires_at_ms, attempt, max_attempts,
                    last_error_kind, last_error_text, proposal_digest, created_at_ms,
                    available_at_ms, updated_at_ms, completed_at_ms
                 ) VALUES (?1, ?2, 'episode_build', ?3, 'succeeded', ?4,
                           NULL, NULL, NULL, 0, 1, NULL, NULL, ?5, ?6, ?6, ?6, ?6)",
                params![
                    stored_job_id,
                    scope_id,
                    request.journal_date,
                    format!("{{\"journal_date\":\"{}\"}}", request.journal_date),
                    proposal_digest,
                    completed_at_ms,
                ],
            )?;
        } else {
            transaction.execute(
                "UPDATE episode_jobs
                 SET state = 'succeeded', checkpoint_json = ?2,
                     lease_owner = NULL, lease_token = NULL,
                     lease_expires_at_ms = NULL, last_error_kind = NULL,
                     last_error_text = NULL, proposal_digest = ?3,
                     updated_at_ms = ?4, completed_at_ms = ?4
                 WHERE job_id = ?1",
                params![
                    stored_job_id,
                    format!("{{\"journal_date\":\"{}\"}}", request.journal_date),
                    proposal_digest,
                    completed_at_ms,
                ],
            )?;
        }

        for event_id in &admitted {
            transaction.execute(
                "INSERT OR IGNORE INTO job_event_admissions(
                    job_id, scope_id, event_id, admitted_at_ms
                 ) VALUES (?1, ?2, ?3, ?4)",
                params![stored_job_id, scope_id, event_id, completed_at_ms],
            )?;
        }

        let current_version: Option<i64> = transaction
            .query_row(
                "SELECT current_version FROM episodes
                 WHERE episode_id = ?1 AND scope_id = ?2 AND state = 'active'",
                params![episode_id, scope_id],
                |row| row.get(0),
            )
            .optional()?;
        let (version, status) = if let Some(current_version) = current_version {
            let current_digest: String = transaction.query_row(
                "SELECT proposal_digest FROM episode_versions
                 WHERE episode_id = ?1 AND version = ?2",
                params![episode_id, current_version],
                |row| row.get(0),
            )?;
            if current_digest == proposal_digest {
                transaction.commit()?;
                return Ok(JournalSaveOutcome {
                    status: "unchanged",
                    scope_id,
                    journal_date: request.journal_date.clone(),
                    episode_id,
                    version: current_version,
                    event_count: admitted.len(),
                    claim_count: proposal.claims.len(),
                });
            }
            let next_version = current_version + 1;
            transaction.execute(
                "UPDATE episodes SET current_version = ?2, updated_at_ms = ?3
                 WHERE episode_id = ?1 AND scope_id = ?4",
                params![episode_id, next_version, completed_at_ms, scope_id],
            )?;
            (next_version, "updated")
        } else {
            transaction.execute(
                "INSERT INTO episodes(
                    episode_id, scope_id, state, current_version,
                    occurred_from_ms, occurred_to_ms, created_at_ms, updated_at_ms
                 ) VALUES (?1, ?2, 'active', 1, ?3, ?4, ?5, ?5)",
                params![
                    episode_id,
                    scope_id,
                    request.occurred_from_ms,
                    request.occurred_to_ms,
                    completed_at_ms,
                ],
            )?;
            (1, "stored")
        };

        let cues_json = serde_json::to_string(&proposal.cues)?;
        let open_loops_json = serde_json::to_string(&proposal.open_loops)?;
        transaction.execute(
            "INSERT INTO episode_versions(
                episode_id, version, title, summary, cues_json, open_loops_json,
                proposal_digest, job_id, epistemic_status, created_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'assistant_inferred', ?9)",
            params![
                episode_id,
                version,
                proposal.title,
                proposal.summary,
                cues_json,
                open_loops_json,
                proposal_digest,
                stored_job_id,
                completed_at_ms,
            ],
        )?;
        for (index, claim) in proposal.claims.iter().enumerate() {
            let version_bytes = version.to_be_bytes();
            let index_bytes = (index as u64).to_be_bytes();
            let claim_id = digest(&[
                b"journal_claim",
                episode_id.as_bytes(),
                &version_bytes,
                &index_bytes,
                claim.claim_text.as_bytes(),
            ]);
            transaction.execute(
                "INSERT INTO episode_claims(
                    claim_id, episode_id, episode_version, claim_kind,
                    claim_text, epistemic_status, created_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    claim_id,
                    episode_id,
                    version,
                    claim.claim_kind.as_str(),
                    claim.claim_text,
                    claim.epistemic_status.as_str(),
                    completed_at_ms,
                ],
            )?;
            for event_id in &claim.evidence_event_ids {
                transaction.execute(
                    "INSERT INTO episode_evidence(
                        claim_id, episode_id, episode_version, scope_id,
                        event_id, evidence_role
                     ) VALUES (?1, ?2, ?3, ?4, ?5, 'supports')",
                    params![claim_id, episode_id, version, scope_id, event_id],
                )?;
            }
            materialize_journal_claim(
                &transaction,
                &scope_id,
                &claim_id,
                claim.claim_kind.as_str(),
                &claim.claim_text,
                &claim.evidence_event_ids,
                completed_at_ms,
            )?;
        }
        transaction.commit()?;

        Ok(JournalSaveOutcome {
            status,
            scope_id,
            journal_date: request.journal_date.clone(),
            episode_id,
            version,
            event_count: admitted.len(),
            claim_count: proposal.claims.len(),
        })
    }

    pub fn read_journal(
        &self,
        request: &DailyJournalRequest,
    ) -> Result<JournalReadOutcome, StoreError> {
        request.validate()?;
        let scope_id = request.scope.id()?;
        let episode_id = digest(&[
            b"episode",
            scope_id.as_bytes(),
            request.journal_date.as_bytes(),
        ]);
        let connection = self
            .connection
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?;
        let entry: Option<(i64, String, String, String, String)> = connection
            .query_row(
                "SELECT episode.current_version, version.title, version.summary,
                        version.cues_json, version.open_loops_json
                 FROM episodes AS episode
                 JOIN episode_versions AS version
                   ON version.episode_id = episode.episode_id
                  AND version.version = episode.current_version
                 WHERE episode.episode_id = ?1
                   AND episode.scope_id = ?2
                   AND episode.state = 'active'",
                params![episode_id, scope_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .optional()?;
        let Some((version, title, summary, cues_json, open_loops_json)) = entry else {
            return Ok(JournalReadOutcome {
                status: "not_found",
                scope_id,
                journal_date: request.journal_date.clone(),
                episode_id,
                version: 0,
                title: String::new(),
                summary: String::new(),
                cues: Vec::new(),
                open_loops: Vec::new(),
                claims: Vec::new(),
            });
        };
        let mut statement = connection.prepare(
            "SELECT claim_kind, claim_text, epistemic_status
             FROM episode_claims
             WHERE episode_id = ?1 AND episode_version = ?2
             ORDER BY rowid ASC",
        )?;
        let rows = statement.query_map(params![episode_id, version], |row| {
            Ok(JournalClaimView {
                claim_kind: row.get(0)?,
                claim_text: row.get(1)?,
                epistemic_status: row.get(2)?,
            })
        })?;
        let mut claims = Vec::new();
        for row in rows {
            claims.push(row?);
        }
        Ok(JournalReadOutcome {
            status: "found",
            scope_id,
            journal_date: request.journal_date.clone(),
            episode_id,
            version,
            title,
            summary,
            cues: serde_json::from_str(&cues_json)?,
            open_loops: serde_json::from_str(&open_loops_json)?,
            claims,
        })
    }

    pub fn purge_scope(&self, request: &PurgeScopeRequest) -> Result<PurgeOutcome, StoreError> {
        request.validate()?;
        let scope_id = request.scope.id()?;
        let completed_at_ms = now_ms();
        let operation_id =
            privacy_operation_id(&scope_id, &request.actor_id, unix_time().as_nanos());

        let mut connection = self
            .connection
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT OR IGNORE INTO scopes(
                scope_id, platform_id, bot_account_id, conversation_kind,
                session_id, persona_id, created_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                scope_id,
                request.scope.platform_id,
                request.scope.bot_account_id,
                request.scope.kind.as_str(),
                request.scope.session_id,
                request.scope.persona_id,
                completed_at_ms,
            ],
        )?;

        let purged_payloads = transaction.execute(
            "UPDATE raw_event_payloads
             SET payload_state = 'purged', sender_display_name = NULL,
                 content = NULL, mentions_json = NULL, metadata_json = NULL,
                 redaction_map_json = '[]', updated_at_ms = ?2, purged_at_ms = ?2
             WHERE payload_state <> 'purged'
               AND event_id IN (SELECT event_id FROM raw_events WHERE scope_id = ?1)",
            params![scope_id, completed_at_ms],
        )?;

        transaction.execute(
            "UPDATE episode_jobs
             SET state = 'dead_letter', lease_token = NULL,
                 lease_expires_at_ms = NULL, last_error_kind = 'privacy_purge',
                 last_error_text = 'scope payloads were purged',
                 updated_at_ms = ?2, completed_at_ms = ?2
             WHERE scope_id = ?1
               AND state IN ('pending', 'leased', 'retry_wait')",
            params![scope_id, completed_at_ms],
        )?;

        transaction.execute(
            "INSERT INTO scope_capture_policies(
                scope_id, policy_state, actor_id, reason_kind, updated_at_ms
             ) VALUES (?1, 'opted_out', ?2, 'explicit_forgetme', ?3)
             ON CONFLICT(scope_id) DO UPDATE SET
                policy_state = 'opted_out',
                actor_id = excluded.actor_id,
                reason_kind = excluded.reason_kind,
                updated_at_ms = excluded.updated_at_ms",
            params![scope_id, request.actor_id, completed_at_ms],
        )?;

        let purged_episodes: usize = transaction.query_row(
            "SELECT COUNT(*) FROM episodes WHERE scope_id = ?1",
            [&scope_id],
            |row| row.get(0),
        )?;
        transaction.execute(
            "DELETE FROM episode_evidence WHERE scope_id = ?1",
            [&scope_id],
        )?;
        transaction.execute(
            "DELETE FROM episode_claims
             WHERE episode_id IN (SELECT episode_id FROM episodes WHERE scope_id = ?1)",
            [&scope_id],
        )?;
        transaction.execute(
            "DELETE FROM episode_versions
             WHERE episode_id IN (SELECT episode_id FROM episodes WHERE scope_id = ?1)",
            [&scope_id],
        )?;
        transaction.execute("DELETE FROM episodes WHERE scope_id = ?1", [&scope_id])?;
        transaction.execute(
            "DELETE FROM event_thread_links WHERE scope_id = ?1",
            [&scope_id],
        )?;
        transaction.execute("DELETE FROM threads WHERE scope_id = ?1", [&scope_id])?;
        transaction.execute(
            "DELETE FROM memory_evidence
             WHERE event_id IN (SELECT event_id FROM raw_events WHERE scope_id = ?1)",
            [&scope_id],
        )?;
        transaction.execute("DELETE FROM memory_edges WHERE scope_id = ?1", [&scope_id])?;
        transaction.execute(
            "DELETE FROM memory_versions
             WHERE memory_id IN (SELECT memory_id FROM memory_atoms WHERE scope_id = ?1)",
            [&scope_id],
        )?;
        transaction.execute("DELETE FROM memory_atoms WHERE scope_id = ?1", [&scope_id])?;
        transaction.execute(
            "DELETE FROM memory_operations WHERE scope_id = ?1",
            [&scope_id],
        )?;
        transaction.execute("DELETE FROM recall_audits WHERE scope_id = ?1", [&scope_id])?;

        let invalidated_journals = transaction.execute(
            "UPDATE journal_exports
             SET state = 'invalidated', invalidated_at_ms = ?2
             WHERE scope_id = ?1 AND state = 'current'",
            params![scope_id, completed_at_ms],
        )?;
        transaction.execute(
            "INSERT INTO privacy_operations(
                operation_id, scope_id, operation_kind, actor_kind, actor_id,
                reason, payload_count, episode_count, journal_count,
                residual_scan, created_at_ms, completed_at_ms
             ) VALUES (?1, ?2, 'purge_payloads', 'user', ?3, ?4, ?5, ?6, ?7,
                       'pending', ?8, ?8)",
            params![
                operation_id,
                scope_id,
                request.actor_id,
                request.reason,
                purged_payloads,
                purged_episodes,
                invalidated_journals,
                completed_at_ms,
            ],
        )?;
        transaction.commit()?;

        let _: (i64, i64, i64) =
            connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?;
        connection.execute_batch("VACUUM")?;
        let residual_payloads: usize = connection.query_row(
            "SELECT COUNT(*)
             FROM raw_event_payloads AS payload
             JOIN raw_events AS event ON event.event_id = payload.event_id
             WHERE event.scope_id = ?1
               AND (payload.payload_state <> 'purged'
                    OR payload.sender_display_name IS NOT NULL
                    OR payload.content IS NOT NULL
                    OR payload.mentions_json IS NOT NULL
                    OR payload.metadata_json IS NOT NULL)",
            [&scope_id],
            |row| row.get(0),
        )?;
        if residual_payloads != 0 {
            connection.execute(
                "UPDATE privacy_operations SET residual_scan = 'failed'
                 WHERE operation_id = ?1",
                [&operation_id],
            )?;
            return Err(StoreError::ResidualPayloads(residual_payloads));
        }
        connection.execute(
            "UPDATE privacy_operations SET residual_scan = 'passed'
             WHERE operation_id = ?1",
            [&operation_id],
        )?;

        Ok(PurgeOutcome {
            status: "ok",
            operation_id,
            scope_id,
            purged_payloads,
            purged_episodes,
            invalidated_journals,
            compacted: true,
        })
    }

    pub fn set_capture_policy(
        &self,
        request: &CapturePolicyRequest,
    ) -> Result<CapturePolicyOutcome, StoreError> {
        request.validate()?;
        let scope_id = request.scope.id()?;
        let updated_at_ms = now_ms();
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT OR IGNORE INTO scopes(
                scope_id, platform_id, bot_account_id, conversation_kind,
                session_id, persona_id, created_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                scope_id,
                request.scope.platform_id,
                request.scope.bot_account_id,
                request.scope.kind.as_str(),
                request.scope.session_id,
                request.scope.persona_id,
                updated_at_ms,
            ],
        )?;
        let prior_reason_kind: Option<String> = transaction
            .query_row(
                "SELECT reason_kind FROM scope_capture_policies WHERE scope_id = ?1",
                [&scope_id],
                |row| row.get(0),
            )
            .optional()?;
        if prior_reason_kind.as_deref() == Some("explicit_forgetme") {
            transaction.commit()?;
            return Ok(CapturePolicyOutcome {
                status: "blocked",
                scope_id,
                policy_state: "opted_out".to_string(),
            });
        }
        let (policy_state, reason_kind) = match request.action {
            CapturePolicyAction::Pause => ("opted_out", "administrator_policy"),
            CapturePolicyAction::Resume => ("active", "explicit_enable"),
        };
        transaction.execute(
            "INSERT INTO scope_capture_policies(
                scope_id, policy_state, actor_id, reason_kind, updated_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(scope_id) DO UPDATE SET
                policy_state = excluded.policy_state,
                actor_id = excluded.actor_id,
                reason_kind = excluded.reason_kind,
                updated_at_ms = excluded.updated_at_ms",
            params![
                scope_id,
                policy_state,
                request.actor_id,
                reason_kind,
                updated_at_ms,
            ],
        )?;
        transaction.commit()?;
        Ok(CapturePolicyOutcome {
            status: "ok",
            scope_id,
            policy_state: policy_state.to_string(),
        })
    }

    pub fn retain_payloads(
        &self,
        request: &RetentionRequest,
    ) -> Result<RetentionOutcome, StoreError> {
        request.validate()?;
        let scope_id = request.scope.id()?;
        let completed_at_ms = now_ms();
        let run_id = privacy_operation_id(&scope_id, &request.actor_id, unix_time().as_nanos());
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?;
        let transaction = connection.transaction()?;
        let purged_payloads = transaction.execute(
            "UPDATE raw_event_payloads
             SET payload_state = 'purged', sender_display_name = NULL,
                 content = NULL, mentions_json = NULL, metadata_json = NULL,
                 redaction_map_json = '[]', updated_at_ms = ?2, purged_at_ms = ?2
             WHERE payload_state <> 'purged'
               AND event_id IN (
                   SELECT event_id FROM raw_events
                   WHERE scope_id = ?1 AND occurred_at_ms < ?3
               )",
            params![scope_id, completed_at_ms, request.cutoff_at_ms],
        )?;
        let decayed_memories = decay_active_memories(&transaction, &scope_id, completed_at_ms)?;
        transaction.execute(
            "INSERT INTO payload_retention_runs(
                run_id, cutoff_at_ms, payload_count, residual_scan,
                started_at_ms, completed_at_ms
             ) VALUES (?1, ?2, ?3, 'pending', ?4, ?4)",
            params![
                run_id,
                request.cutoff_at_ms,
                purged_payloads,
                completed_at_ms,
            ],
        )?;
        transaction.commit()?;
        let residual_payloads: i64 = connection.query_row(
            "SELECT COUNT(*)
             FROM raw_event_payloads AS payload
             JOIN raw_events AS event ON event.event_id = payload.event_id
             WHERE event.scope_id = ?1
               AND event.occurred_at_ms < ?2
               AND (payload.payload_state <> 'purged'
                    OR payload.sender_display_name IS NOT NULL
                    OR payload.content IS NOT NULL
                    OR payload.mentions_json IS NOT NULL
                    OR payload.metadata_json IS NOT NULL)",
            params![scope_id, request.cutoff_at_ms],
            |row| row.get(0),
        )?;
        let residual_scan = if residual_payloads == 0 {
            "passed"
        } else {
            "failed"
        };
        connection.execute(
            "UPDATE payload_retention_runs
             SET residual_scan = ?, completed_at_ms = ?
             WHERE run_id = ?",
            params![residual_scan, completed_at_ms, run_id],
        )?;
        if residual_payloads != 0 {
            return Err(StoreError::ResidualPayloads(residual_payloads as usize));
        }
        Ok(RetentionOutcome {
            status: "ok",
            scope_id,
            run_id,
            cutoff_at_ms: request.cutoff_at_ms,
            purged_payloads,
            decayed_memories,
            residual_scan,
        })
    }

    pub fn scope_stats(
        &self,
        request: &ScopeStatsRequest,
    ) -> Result<ScopeStatsOutcome, StoreError> {
        request.validate()?;
        let scope_id = request.scope.id()?;
        let connection = self
            .connection
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?;
        let count = |sql: &str| -> Result<i64, rusqlite::Error> {
            connection.query_row(sql, [&scope_id], |row| row.get(0))
        };
        let policy_state: String = connection
            .query_row(
                "SELECT policy_state FROM scope_capture_policies WHERE scope_id = ?1",
                [&scope_id],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or_else(|| "active".to_string());
        let last_retention_cutoff_ms: Option<i64> = connection
            .query_row(
                "SELECT MAX(event.occurred_at_ms)
                 FROM raw_events AS event
                 JOIN raw_event_payloads AS payload ON payload.event_id = event.event_id
                 WHERE event.scope_id = ?1 AND payload.payload_state = 'purged'",
                [&scope_id],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        Ok(ScopeStatsOutcome {
            status: "ok",
            scope_id: scope_id.clone(),
            policy_state,
            raw_events: count("SELECT COUNT(*) FROM raw_events WHERE scope_id = ?1")?,
            active_payloads: connection.query_row(
                "SELECT COUNT(*) FROM raw_event_payloads AS payload
                 JOIN raw_events AS event ON event.event_id = payload.event_id
                 WHERE event.scope_id = ?1 AND payload.payload_state = 'active'",
                [&scope_id],
                |row| row.get(0),
            )?,
            episodes: count("SELECT COUNT(*) FROM episodes WHERE scope_id = ?1")?,
            active_memories: count(
                "SELECT COUNT(*) FROM memory_atoms WHERE scope_id = ?1 AND state = 'active'",
            )?,
            superseded_memories: count(
                "SELECT COUNT(*) FROM memory_atoms WHERE scope_id = ?1 AND state = 'superseded'",
            )?,
            archived_memories: count(
                "SELECT COUNT(*) FROM memory_atoms WHERE scope_id = ?1 AND state = 'archived'",
            )?,
            last_retention_cutoff_ms,
        })
    }

    /// Query only materialized, evidence-backed journal claims in the request scope.
    ///
    /// This intentionally uses a small deterministic lexical scorer instead of a
    /// vector index. The model decides whether to call this function; the native
    /// kernel decides what may cross the boundary and records the decision.
    pub fn request_recall(
        &self,
        request: &RecallRequest,
        request_json: &str,
    ) -> Result<RecallOutcome, StoreError> {
        request.validate()?;
        let scope_id = request.scope.id()?;
        let created_at_ms = now_ms();
        let nonce = unix_time().as_nanos();
        let recall_id = recall_id(&scope_id, request_json, nonce);
        let cues_json = serde_json::to_string(&request.cues)?;

        let mut connection = self
            .connection
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT OR IGNORE INTO scopes(
                scope_id, platform_id, bot_account_id, conversation_kind,
                session_id, persona_id, created_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                scope_id,
                request.scope.platform_id,
                request.scope.bot_account_id,
                request.scope.kind.as_str(),
                request.scope.session_id,
                request.scope.persona_id,
                created_at_ms,
            ],
        )?;
        #[derive(Debug)]
        struct Candidate {
            memory_id: String,
            memory_kind: String,
            statement: String,
            confidence: f64,
            evidence_event_ids: Vec<String>,
            score: i64,
            occurred_from_ms: i64,
        }

        let claim_kinds: &[&str] = match request.need_type {
            NeedType::Fact => &["fact"],
            NeedType::Preference => &["preference"],
            NeedType::Decision => &["decision"],
            NeedType::Commitment => &["commitment"],
            NeedType::Relationship => &["relationship"],
            NeedType::Procedure => &["procedure"],
            NeedType::Episode => &[],
        };
        let cues = request
            .cues
            .iter()
            .map(|cue| normalize_recall_text(cue))
            .filter(|cue| !cue.is_empty())
            .collect::<Vec<_>>();
        let time_window = request
            .time_hint
            .as_deref()
            .and_then(parse_recall_date_hint);
        let mut candidates = Vec::new();
        let mut materialized_statements = HashSet::new();
        if !claim_kinds.is_empty() {
            let mut memories = transaction.prepare(
                "SELECT atom.memory_id, atom.memory_kind, version.version,
                        version.statement, atom.confidence, atom.activation,
                        COALESCE(MAX(event.occurred_at_ms), version.created_at_ms)
                 FROM memory_atoms AS atom
                 JOIN memory_versions AS version
                   ON version.memory_id = atom.memory_id
                  AND version.version = atom.current_version
                 LEFT JOIN memory_evidence AS evidence
                   ON evidence.memory_id = version.memory_id
                  AND evidence.memory_version = version.version
                 LEFT JOIN raw_events AS event
                   ON event.event_id = evidence.event_id
                 WHERE atom.scope_id = ?1 AND atom.state = 'active'
                 GROUP BY atom.memory_id, atom.memory_kind, version.version,
                          version.statement, atom.confidence, atom.activation,
                          version.created_at_ms
                 ORDER BY atom.activation DESC, 7 DESC, atom.memory_id ASC
                 LIMIT 256",
            )?;
            let memory_rows = memories.query_map([&scope_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, f64>(4)?,
                    row.get::<_, f64>(5)?,
                    row.get::<_, i64>(6)?,
                ))
            })?;
            for memory_row in memory_rows {
                let (
                    memory_id,
                    memory_kind,
                    memory_version,
                    statement,
                    confidence,
                    activation,
                    occurred_at_ms,
                ) = memory_row?;
                if !claim_kinds.contains(&memory_kind.as_str()) {
                    continue;
                }
                if let Some((from_ms, to_ms)) = time_window
                    && (occurred_at_ms < from_ms || occurred_at_ms >= to_ms)
                {
                    continue;
                }
                let searchable = normalize_recall_text(&statement);
                let matched = cues
                    .iter()
                    .filter(|cue| searchable.contains(cue.as_str()))
                    .count();
                if matched == 0 {
                    continue;
                }
                let evidence_event_ids =
                    evidence_for_memory(&transaction, &memory_id, memory_version)?;
                if evidence_event_ids.is_empty() {
                    continue;
                }
                materialized_statements.insert(searchable);
                candidates.push(Candidate {
                    memory_id,
                    memory_kind,
                    statement: truncate_chars(&statement, 260).0,
                    confidence,
                    evidence_event_ids,
                    score: (matched as i64) * 120 + (activation * 10.0) as i64,
                    occurred_from_ms: occurred_at_ms,
                });
            }
        }
        let mut episodes = transaction.prepare(
            "SELECT episode.episode_id, episode.current_version,
                    episode.occurred_from_ms, version.title, version.summary,
                    version.cues_json, version.open_loops_json
             FROM episodes AS episode
             JOIN episode_versions AS version
               ON version.episode_id = episode.episode_id
              AND version.version = episode.current_version
             WHERE episode.scope_id = ?1
               AND episode.state = 'active'
             ORDER BY episode.occurred_from_ms DESC, episode.episode_id ASC
             LIMIT 128",
        )?;
        let episode_rows = episodes.query_map([&scope_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
            ))
        })?;
        for row in episode_rows {
            let (
                episode_id,
                version,
                occurred_from_ms,
                title,
                summary,
                cues_json_value,
                open_loops_json,
            ) = row?;
            let episode_text = normalize_recall_text(&format!(
                "{title} {summary} {cues_json_value} {open_loops_json}"
            ));
            let episode_evidence = evidence_for_episode(&transaction, &episode_id, version)?;
            if episode_evidence.is_empty() {
                continue;
            }
            let episode_occurred_at_ms =
                latest_evidence_time(&transaction, &episode_evidence, occurred_from_ms)?;
            if let Some((from_ms, to_ms)) = time_window
                && (episode_occurred_at_ms < from_ms || episode_occurred_at_ms >= to_ms)
            {
                continue;
            }
            let searchable_episode =
                normalize_recall_text(&format!("{episode_text} {}", episode_evidence.join(" ")));
            let matched = cues
                .iter()
                .filter(|cue| searchable_episode.contains(cue.as_str()))
                .count();

            if request.need_type == NeedType::Episode {
                if matched == 0 {
                    continue;
                }
                candidates.push(Candidate {
                    memory_id: episode_id,
                    memory_kind: "episode".to_string(),
                    statement: truncate_chars(&format!("{title}：{summary}"), 260).0,
                    confidence: (0.60 + 0.08 * matched as f64).min(0.94),
                    evidence_event_ids: episode_evidence,
                    score: (matched as i64) * 100,
                    occurred_from_ms: episode_occurred_at_ms,
                });
                continue;
            }

            let mut claims = transaction.prepare(
                "SELECT claim_id, claim_kind, claim_text
                 FROM episode_claims
                 WHERE episode_id = ?1 AND episode_version = ?2
                 ORDER BY claim_id ASC",
            )?;
            let claim_rows = claims.query_map(params![episode_id, version], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            for claim_row in claim_rows {
                let (claim_id, claim_kind, claim_text) = claim_row?;
                if !claim_kinds.contains(&claim_kind.as_str()) {
                    continue;
                }
                let terminal_state: Option<String> = transaction
                    .query_row(
                        "SELECT state FROM memory_atoms WHERE memory_id = ?1",
                        [journal_memory_id(&scope_id, &claim_kind, &claim_text)],
                        |row| row.get(0),
                    )
                    .optional()?;
                if terminal_state
                    .as_deref()
                    .is_some_and(is_terminal_memory_state)
                {
                    continue;
                }
                if materialized_statements.contains(&normalize_recall_text(&claim_text)) {
                    continue;
                }
                let evidence_event_ids = evidence_for_claim(&transaction, &claim_id)?;
                if evidence_event_ids.is_empty() {
                    continue;
                }
                let claim_occurred_at_ms =
                    latest_evidence_time(&transaction, &evidence_event_ids, occurred_from_ms)?;
                if let Some((from_ms, to_ms)) = time_window
                    && (claim_occurred_at_ms < from_ms || claim_occurred_at_ms >= to_ms)
                {
                    continue;
                }
                let searchable = normalize_recall_text(&format!(
                    "{title} {summary} {cues_json_value} {open_loops_json} {claim_text} {}",
                    evidence_event_ids.join(" ")
                ));
                let matched = cues
                    .iter()
                    .filter(|cue| searchable.contains(cue.as_str()))
                    .count();
                if matched == 0 {
                    continue;
                }
                candidates.push(Candidate {
                    memory_id: claim_id,
                    memory_kind: claim_kind,
                    statement: truncate_chars(&claim_text, 260).0,
                    confidence: (0.62 + 0.08 * matched as f64).min(0.96),
                    evidence_event_ids,
                    score: (matched as i64) * 100,
                    occurred_from_ms: claim_occurred_at_ms,
                });
            }
        }
        drop(episodes);

        candidates.sort_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then_with(|| right.occurred_from_ms.cmp(&left.occurred_from_ms))
                .then_with(|| left.memory_id.cmp(&right.memory_id))
        });
        let (item_limit, brief_limit) = match request.depth {
            RecallDepth::Glance => (1, 420),
            RecallDepth::Focused => (2, 620),
            RecallDepth::Deep => (3, 800),
        };
        candidates.truncate(item_limit);
        let items = candidates
            .into_iter()
            .map(|candidate| RecallItem {
                memory_id: candidate.memory_id,
                memory_kind: candidate.memory_kind,
                statement: candidate.statement,
                confidence: candidate.confidence,
                occurred_at_ms: candidate.occurred_from_ms,
                evidence_status: "supported",
                evidence_event_ids: candidate.evidence_event_ids,
            })
            .collect::<Vec<_>>();
        let status = if items.is_empty() { "not_found" } else { "ok" };
        let reason = if items.is_empty() {
            "没有找到同时满足范围、类型、线索和证据要求的记忆".to_string()
        } else {
            "返回当前范围内的证据支持记忆".to_string()
        };
        let brief = if items.is_empty() {
            None
        } else {
            let mut text = String::from("根据已保存的日记证据：");
            for (index, item) in items.iter().enumerate() {
                let evidence = item
                    .evidence_event_ids
                    .iter()
                    .take(2)
                    .map(|id| id.chars().take(8).collect::<String>())
                    .collect::<Vec<_>>()
                    .join("、");
                text.push_str(&format!(
                    "\n{}. {}（日期 {}｜类型 {}｜置信度 {:.0}%｜证据已支持：{}）",
                    index + 1,
                    item.statement,
                    utc_date_from_unix_ms(item.occurred_at_ms),
                    item.memory_kind,
                    item.confidence * 100.0,
                    evidence
                ));
            }
            Some(truncate_chars(&text, brief_limit).0)
        };
        let exposed_ids = items
            .iter()
            .map(|item| item.memory_id.clone())
            .collect::<Vec<_>>();
        let exposed_ids_json = serde_json::to_string(&exposed_ids)?;
        transaction.execute(
            "INSERT INTO recall_audits(
                recall_id, scope_id, need_type, depth, reason, cues_json,
                time_hint, decision, decision_reason,
                exposed_memory_ids_json, created_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                recall_id,
                scope_id,
                request.need_type.as_str(),
                request.depth.as_str(),
                request.reason,
                cues_json,
                request.time_hint,
                status,
                reason,
                exposed_ids_json,
                created_at_ms,
            ],
        )?;
        transaction.commit()?;

        Ok(RecallOutcome {
            status: status.to_string(),
            protocol: PROTOCOL_VERSION,
            recall_id,
            scope_id,
            reason: reason.clone(),
            summary: brief.clone().unwrap_or_else(|| reason.clone()),
            brief,
            items,
            uncertainties: if status == "not_found" {
                vec![reason]
            } else {
                Vec::new()
            },
        })
    }
}

fn decay_active_memories(
    transaction: &Transaction<'_>,
    scope_id: &str,
    now_ms: i64,
) -> Result<usize, rusqlite::Error> {
    let candidates: Vec<(String, f64, i64)> = {
        let mut statement = transaction.prepare(
            "SELECT memory_id, activation, updated_at_ms
             FROM memory_atoms
             WHERE scope_id = ?1 AND state = 'active'",
        )?;
        let rows = statement.query_map([scope_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, f64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };

    let mut decayed = 0;
    for (memory_id, activation, updated_at_ms) in candidates {
        let elapsed_ms = now_ms.saturating_sub(updated_at_ms);
        if elapsed_ms <= 0 || activation <= ACTIVATION_FLOOR {
            continue;
        }
        let periods = elapsed_ms as f64 / ACTIVATION_HALF_LIFE_MS as f64;
        let next_activation =
            ACTIVATION_FLOOR + (activation - ACTIVATION_FLOOR) * 0.5_f64.powf(periods);
        if next_activation + f64::EPSILON >= activation {
            continue;
        }
        transaction.execute(
            "UPDATE memory_atoms
             SET activation = ?2, updated_at_ms = ?3
             WHERE memory_id = ?1 AND scope_id = ?4 AND state = 'active'",
            params![memory_id, next_activation, now_ms, scope_id],
        )?;
        decayed += 1;
    }
    Ok(decayed)
}

fn utc_date_from_unix_ms(timestamp_ms: i64) -> String {
    let days = timestamp_ms.div_euclid(86_400_000);
    let z = days + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    let year = year + if month <= 2 { 1 } else { 0 };
    format!("{year:04}-{month:02}-{day:02}")
}

fn normalize_recall_text(value: &str) -> String {
    value
        .chars()
        .flat_map(|character| character.to_lowercase())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_recall_date_hint(value: &str) -> Option<(i64, i64)> {
    let mut parts = value.trim().split('-');
    let year = parts.next()?.parse::<i64>().ok()?;
    let month = parts.next()?.parse::<i64>().ok()?;
    let day = parts.next()?.parse::<i64>().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&month) {
        return None;
    }
    let month_days = match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if day < 1 || day > month_days {
        return None;
    }
    let adjusted_year = year - if month <= 2 { 1 } else { 0 };
    let era = if adjusted_year >= 0 {
        adjusted_year / 400
    } else {
        (adjusted_year - 399) / 400
    };
    let year_of_era = adjusted_year - era * 400;
    let month_prime = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * month_prime + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days_since_epoch = era * 146_097 + day_of_era - 719_468;
    let start_ms = days_since_epoch.checked_mul(86_400_000)?;
    Some((start_ms, start_ms.checked_add(86_400_000)?))
}

fn evidence_for_claim(
    transaction: &rusqlite::Transaction<'_>,
    claim_id: &str,
) -> Result<Vec<String>, rusqlite::Error> {
    let mut statement = transaction.prepare(
        "SELECT event_id
         FROM episode_evidence
         WHERE claim_id = ?1
         ORDER BY event_id ASC
         LIMIT 8",
    )?;
    let rows = statement.query_map([claim_id], |row| row.get::<_, String>(0))?;
    rows.collect()
}

fn latest_evidence_time(
    transaction: &rusqlite::Transaction<'_>,
    event_ids: &[String],
    fallback_ms: i64,
) -> Result<i64, rusqlite::Error> {
    let mut latest = fallback_ms;
    for event_id in event_ids {
        let occurred_at_ms: Option<i64> = transaction
            .query_row(
                "SELECT occurred_at_ms FROM raw_events WHERE event_id = ?1",
                [event_id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(occurred_at_ms) = occurred_at_ms {
            latest = latest.max(occurred_at_ms);
        }
    }
    Ok(latest)
}

fn evidence_for_memory(
    transaction: &rusqlite::Transaction<'_>,
    memory_id: &str,
    memory_version: i64,
) -> Result<Vec<String>, rusqlite::Error> {
    let mut statement = transaction.prepare(
        "SELECT event_id
         FROM memory_evidence
         WHERE memory_id = ?1 AND memory_version = ?2
         ORDER BY event_id ASC
         LIMIT 8",
    )?;
    let rows = statement.query_map(params![memory_id, memory_version], |row| {
        row.get::<_, String>(0)
    })?;
    rows.collect()
}

fn evidence_for_episode(
    transaction: &rusqlite::Transaction<'_>,
    episode_id: &str,
    version: i64,
) -> Result<Vec<String>, rusqlite::Error> {
    let mut statement = transaction.prepare(
        "SELECT DISTINCT event_id
         FROM episode_evidence
         WHERE episode_id = ?1 AND episode_version = ?2
         ORDER BY event_id ASC
         LIMIT 8",
    )?;
    let rows = statement.query_map(params![episode_id, version], |row| row.get::<_, String>(0))?;
    rows.collect()
}

fn materialize_journal_claim(
    transaction: &rusqlite::Transaction<'_>,
    scope_id: &str,
    claim_id: &str,
    claim_kind: &str,
    claim_text: &str,
    evidence_event_ids: &[String],
    created_at_ms: i64,
) -> Result<(), StoreError> {
    let memory_kind = journal_memory_kind(claim_kind);
    let memory_id = journal_memory_id(scope_id, claim_kind, claim_text);
    let existing: Option<(i64, String)> = transaction
        .query_row(
            "SELECT current_version, state FROM memory_atoms WHERE memory_id = ?1",
            [&memory_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let operation_id = digest(&[
        b"memory_operation",
        memory_id.as_bytes(),
        claim_id.as_bytes(),
        &created_at_ms.to_be_bytes(),
    ]);
    let operation_kind = if existing.is_some() {
        "strengthen"
    } else {
        "create"
    };
    let terminal_state = existing
        .as_ref()
        .map(|(_, state)| state.as_str())
        .filter(|state| is_terminal_memory_state(state));
    let decision = if terminal_state.is_some() {
        "rejected"
    } else {
        "accepted"
    };
    let decision_reason = match existing.as_ref().map(|(_, state)| state.as_str()) {
        Some("superseded") | Some("archived") => {
            "terminal memory state cannot be reactivated by a journal claim"
        }
        Some("candidate") => "matching candidate promoted and strengthened",
        Some(_) => "same normalized claim strengthened",
        None => "journal claim materialized",
    };
    transaction.execute(
        "INSERT INTO memory_operations(
            operation_id, scope_id, proposal_id, operation_kind,
            actor_kind, actor_id, request_json, decision, decision_reason,
            created_at_ms
         ) VALUES (?1, ?2, ?3, ?4, 'kernel', 'journal', ?5, ?6, ?7, ?8)",
        params![
            operation_id,
            scope_id,
            claim_id,
            operation_kind,
            format!("{{\"source\":\"journal\",\"claim_id\":\"{claim_id}\"}}"),
            decision,
            decision_reason,
            created_at_ms,
        ],
    )?;
    if terminal_state.is_some() {
        return Ok(());
    }
    let version = if let Some((current_version, _state)) = existing {
        transaction.execute(
            "UPDATE memory_atoms
             SET state = CASE WHEN state = 'candidate' THEN 'active' ELSE state END,
                 activation = MIN(activation + 0.1, 1.0),
                 updated_at_ms = ?2
             WHERE memory_id = ?1",
            params![memory_id, created_at_ms],
        )?;
        current_version
    } else {
        transaction.execute(
            "INSERT INTO memory_atoms(
                memory_id, scope_id, memory_kind, state, confidence,
                importance, activation, current_version, created_at_ms, updated_at_ms
             ) VALUES (?1, ?2, ?3, 'active', 0.65, 0.70, 0.50, 1, ?4, ?4)",
            params![memory_id, scope_id, memory_kind, created_at_ms],
        )?;
        transaction.execute(
            "INSERT INTO memory_versions(
                memory_id, version, statement, qualifiers_json,
                valid_from_ms, valid_to_ms, operation_id, created_at_ms
             ) VALUES (?1, 1, ?2, '{}', ?3, NULL, ?4, ?3)",
            params![memory_id, claim_text, created_at_ms, operation_id],
        )?;
        1
    };
    for event_id in evidence_event_ids {
        transaction.execute(
            "INSERT OR IGNORE INTO memory_evidence(
                memory_id, memory_version, event_id, evidence_role
             ) VALUES (?1, ?2, ?3, 'supports')",
            params![memory_id, version, event_id],
        )?;
    }
    Ok(())
}

fn journal_memory_kind(claim_kind: &str) -> &str {
    if claim_kind == "open_loop" {
        "commitment"
    } else {
        claim_kind
    }
}

fn journal_memory_id(scope_id: &str, claim_kind: &str, claim_text: &str) -> String {
    let memory_kind = journal_memory_kind(claim_kind);
    let normalized = normalize_recall_text(claim_text);
    digest(&[
        b"memory",
        scope_id.as_bytes(),
        memory_kind.as_bytes(),
        normalized.as_bytes(),
    ])
}

fn is_terminal_memory_state(state: &str) -> bool {
    matches!(state, "superseded" | "archived")
}

#[cfg(test)]
mod tests {
    use super::*;
    use mnemokernel_core::{
        CapturePolicyAction, CapturePolicyRequest, DailyJournalProposal, DailyJournalRequest,
        JournalClaimKind, JournalClaimProposal, JournalEpistemicStatus, NeedType,
        OccurredTimeSource, OriginKind, PlatformMessageIdState, RecallDepth, RecallRequest,
        RetentionRequest, ScopeDescriptor, ScopeKind, ScopeStatsRequest,
    };
    use serde_json::json;
    use std::fs;

    fn temporary_database(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "mnemokernel-{label}-{}-{}.sqlite3",
            std::process::id(),
            unix_time().as_nanos()
        ))
    }

    #[test]
    fn inspector_is_scoped_paginated_and_respects_purge() {
        use super::inspector::InspectRequest;
        let path = temporary_database("inspector");
        let store = KernelStore::open(&path).unwrap();
        let first = event("visible message");
        let result = store.ingest_event(&first).unwrap();
        let second = event_with("second", "second message", 2);
        let second_result = store.ingest_event(&second).unwrap();
        let mut other = event("other scope secret");
        other.scope.session_id = "other".into();
        store.ingest_event(&other).unwrap();
        let day = journal_request(first.scope.clone());
        let proposal = journal_proposal(&day, vec![result.event_id, second_result.event_id]);
        store.save_journal(&day, &proposal, "{}").unwrap();
        let mut request = InspectRequest {
            collection: "events".into(),
            scope_id: result.scope_id,
            query: "".into(),
            offset: 0,
            limit: 1,
        };
        let page = store.inspect(&request).unwrap();
        assert_eq!(page["total"], 2);
        assert_eq!(page["items"].as_array().unwrap().len(), 1);
        assert_eq!(page["items"][0]["content"], "second message");
        request.offset = 1;
        assert_eq!(
            store.inspect(&request).unwrap()["items"][0]["content"],
            "visible message"
        );
        request.offset = 0;
        request.query = "' OR 1=1 --".into();
        assert_eq!(store.inspect(&request).unwrap()["total"], 0);
        request.query.clear();
        for (collection, count) in [
            ("scopes", 2),
            ("memories", 2),
            ("journals", 1),
            ("recalls", 0),
        ] {
            request.collection = collection.into();
            assert_eq!(store.inspect(&request).unwrap()["total"], count);
        }
        request.collection = "sqlite_master".into();
        assert!(store.inspect(&request).is_err());
        request.collection = "events".into();
        request.limit = 51;
        assert!(store.inspect(&request).is_err());
        request.limit = 20;
        store
            .purge_scope(&PurgeScopeRequest {
                protocol: PROTOCOL_VERSION.into(),
                scope: first.scope,
                actor_id: "user".into(),
                reason: "inspector test".into(),
            })
            .unwrap();
        let page = store.inspect(&request).unwrap();
        assert!(
            page["items"]
                .as_array()
                .unwrap()
                .iter()
                .all(|row| row["content"].is_null())
        );
        for collection in ["journals", "memories"] {
            request.collection = collection.into();
            assert_eq!(store.inspect(&request).unwrap()["total"], 0);
        }
        drop(store);
        let _ = fs::remove_file(path);
    }

    fn event(content: &str) -> RawEventInput {
        RawEventInput {
            protocol: PROTOCOL_VERSION.into(),
            origin_kind: OriginKind::UserMessage,
            occurred_time_source: OccurredTimeSource::Platform,
            platform_message_id_state: PlatformMessageIdState::Present,
            source_key: "test:message-1".into(),
            scope: ScopeDescriptor {
                platform_id: "test".into(),
                bot_account_id: "bot".into(),
                kind: ScopeKind::Private,
                session_id: "user".into(),
                persona_id: "default".into(),
            },
            sender_account_id: "user".into(),
            sender_display_name: "User".into(),
            sender_is_bot: false,
            observed_at_ms: 1,
            occurred_at_ms: 1,
            content: content.into(),
            reply_to_source_key: None,
            mentions: vec![],
            metadata: json!({}),
        }
    }

    fn event_with(source_key: &str, content: &str, occurred_at_ms: i64) -> RawEventInput {
        let mut value = event(content);
        value.source_key = source_key.into();
        value.observed_at_ms = occurred_at_ms;
        value.occurred_at_ms = occurred_at_ms;
        value
    }

    fn journal_request(scope: ScopeDescriptor) -> DailyJournalRequest {
        DailyJournalRequest {
            protocol: PROTOCOL_VERSION.into(),
            scope,
            journal_date: "1970-01-01".into(),
            occurred_from_ms: 0,
            occurred_to_ms: 86_400_000,
        }
    }

    fn journal_proposal(
        request: &DailyJournalRequest,
        evidence_event_ids: Vec<String>,
    ) -> DailyJournalProposal {
        DailyJournalProposal {
            protocol: PROTOCOL_VERSION.into(),
            proposal_id: "proposal-1".into(),
            scope_id: request.scope.id().unwrap(),
            journal_date: request.journal_date.clone(),
            occurred_from_ms: request.occurred_from_ms,
            occurred_to_ms: request.occurred_to_ms,
            title: "两件事".into(),
            summary: "完成了项目整理，也记录了一个后续决定。".into(),
            cues: vec!["项目整理".into(), "后续决定".into()],
            open_loops: vec!["继续验证".into()],
            claims: vec![
                JournalClaimProposal {
                    claim_kind: JournalClaimKind::Fact,
                    claim_text: "今天整理了项目结构。".into(),
                    epistemic_status: JournalEpistemicStatus::AssistantInferred,
                    evidence_event_ids: vec![evidence_event_ids[0].clone()],
                },
                JournalClaimProposal {
                    claim_kind: JournalClaimKind::Decision,
                    claim_text: "下一步先验证手动日记闭环。".into(),
                    epistemic_status: JournalEpistemicStatus::AssistantInferred,
                    evidence_event_ids: vec![evidence_event_ids[1].clone()],
                },
            ],
        }
    }

    #[test]
    fn ingest_is_idempotent_but_rejects_changed_content() {
        let path = temporary_database("idempotent");
        let store = KernelStore::open(&path).unwrap();
        assert!(store.ingest_event(&event("hello")).unwrap().inserted);
        assert!(!store.ingest_event(&event("hello")).unwrap().inserted);
        assert!(matches!(
            store.ingest_event(&event("changed")),
            Err(StoreError::IdempotencyConflict(_))
        ));
        drop(store);
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(path.with_extension("sqlite3-wal"));
        let _ = fs::remove_file(path.with_extension("sqlite3-shm"));
    }

    #[test]
    fn raw_events_are_protected_from_updates() {
        let path = temporary_database("append-only");
        let store = KernelStore::open(&path).unwrap();
        let outcome = store.ingest_event(&event("hello")).unwrap();
        drop(store);
        let connection = Connection::open(&path).unwrap();
        let changed = connection.execute(
            "UPDATE raw_events SET content = 'tampered' WHERE event_id = ?1",
            [outcome.event_id],
        );
        assert!(changed.is_err());
        drop(connection);
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(path.with_extension("sqlite3-wal"));
        let _ = fs::remove_file(path.with_extension("sqlite3-shm"));
    }

    #[test]
    fn payload_is_separate_and_scope_purge_does_not_restore_on_replay() {
        let path = temporary_database("privacy-purge");
        let store = KernelStore::open(&path).unwrap();
        let input = event("hello-private-secret");
        let outcome = store.ingest_event(&input).unwrap();

        {
            let connection = store.connection.lock().unwrap();
            let envelope_content: String = connection
                .query_row(
                    "SELECT content FROM raw_events WHERE event_id = ?1",
                    [&outcome.event_id],
                    |row| row.get(0),
                )
                .unwrap();
            let payload_content: String = connection
                .query_row(
                    "SELECT content FROM raw_event_payloads WHERE event_id = ?1",
                    [&outcome.event_id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(envelope_content, "");
            assert_eq!(payload_content, "hello-private-secret");
        }

        let purge = store
            .purge_scope(&PurgeScopeRequest {
                protocol: PROTOCOL_VERSION.into(),
                scope: input.scope.clone(),
                actor_id: "user".into(),
                reason: "test requested deletion".into(),
            })
            .unwrap();
        assert_eq!(purge.purged_payloads, 1);
        assert!(purge.compacted);
        assert!(!store.ingest_event(&input).unwrap().inserted);

        {
            let connection = store.connection.lock().unwrap();
            let state: (String, Option<String>) = connection
                .query_row(
                    "SELECT payload_state, content FROM raw_event_payloads WHERE event_id = ?1",
                    [&outcome.event_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(state, ("purged".into(), None));
        }
        let blocked_resume = store
            .set_capture_policy(&CapturePolicyRequest {
                protocol: PROTOCOL_VERSION.into(),
                scope: input.scope.clone(),
                action: CapturePolicyAction::Resume,
                actor_id: "user".into(),
                reason: "resume after forget must be blocked".into(),
            })
            .unwrap();
        assert_eq!(blocked_resume.status, "blocked");
        assert_eq!(blocked_resume.policy_state, "opted_out");
        drop(store);
        let bytes = fs::read(&path).unwrap();
        assert!(
            !bytes
                .windows(b"hello-private-secret".len())
                .any(|window| window == b"hello-private-secret")
        );
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(path.with_extension("sqlite3-wal"));
        let _ = fs::remove_file(path.with_extension("sqlite3-shm"));
    }

    #[test]
    fn daily_journal_reads_topics_and_is_idempotent() {
        let path = temporary_database("daily-journal");
        let store = KernelStore::open(&path).unwrap();
        let mut first = event_with("test:message-1", "项目整理完成", 1_000);
        first.reply_to_source_key = Some("test:message-parent".into());
        let second = event_with("test:message-2", "决定先做手动验证", 2_000);
        let first_id = store.ingest_event(&first).unwrap().event_id;
        let second_id = store.ingest_event(&second).unwrap().event_id;
        let request = journal_request(first.scope.clone());

        let context = store.journal_context(&request).unwrap();
        assert_eq!(context.status, "ok");
        assert_eq!(context.events.len(), 2);
        assert_eq!(
            context.events[0].reply_to_source_key.as_deref(),
            Some("test:message-parent")
        );
        let proposal = journal_proposal(&request, vec![first_id, second_id]);
        let saved = store
            .save_journal(
                &request,
                &proposal,
                &serde_json::to_string(&proposal).unwrap(),
            )
            .unwrap();
        assert_eq!(saved.status, "stored");
        assert_eq!(saved.version, 1);
        assert_eq!(saved.claim_count, 2);

        let repeated = store
            .save_journal(
                &request,
                &proposal,
                &serde_json::to_string(&proposal).unwrap(),
            )
            .unwrap();
        assert_eq!(repeated.status, "unchanged");
        assert_eq!(repeated.version, 1);

        let read = store.read_journal(&request).unwrap();
        assert_eq!(read.status, "found");
        assert_eq!(read.claims.len(), 2);
        assert_eq!(read.open_loops, vec!["继续验证"]);
        drop(store);
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(path.with_extension("sqlite3-wal"));
        let _ = fs::remove_file(path.with_extension("sqlite3-shm"));
    }

    #[test]
    fn daily_journal_rejects_evidence_outside_the_admitted_day() {
        let path = temporary_database("daily-journal-invalid-evidence");
        let store = KernelStore::open(&path).unwrap();
        let input = event_with("test:message-1", "当天内容", 1_000);
        store.ingest_event(&input).unwrap();
        let request = journal_request(input.scope.clone());
        let proposal = journal_proposal(&request, vec!["f".repeat(64), "e".repeat(64)]);
        assert!(matches!(
            store.save_journal(
                &request,
                &proposal,
                &serde_json::to_string(&proposal).unwrap(),
            ),
            Err(StoreError::Validation(
                ValidationError::EvidenceOutsideAdmission(_)
            ))
        ));
        let read = store.read_journal(&request).unwrap();
        assert_eq!(read.status, "not_found");
        drop(store);
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(path.with_extension("sqlite3-wal"));
        let _ = fs::remove_file(path.with_extension("sqlite3-shm"));
    }

    #[test]
    fn journal_context_has_a_hard_event_and_character_budget() {
        let path = temporary_database("journal-context-budget");
        let store = KernelStore::open(&path).unwrap();
        for index in 0..100 {
            let source_key = format!("test:budget-{index}");
            let content = format!("事件 {index} {}", "长内容".repeat(1_000));
            store
                .ingest_event(&event_with(&source_key, &content, index + 1))
                .unwrap();
        }
        let request = journal_request(event("scope").scope);
        let context = store.journal_context(&request).unwrap();
        assert_eq!(context.events.len(), JOURNAL_EVENT_LIMIT);
        assert!(context.context_truncated);
        assert!(
            context
                .events
                .iter()
                .map(|event| event.content.chars().count())
                .sum::<usize>()
                <= JOURNAL_CONTENT_BUDGET
        );
        assert!(
            context
                .events
                .iter()
                .all(|event| { event.content.chars().count() <= JOURNAL_EVENT_CONTENT_LIMIT })
        );
        drop(store);
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(path.with_extension("sqlite3-wal"));
        let _ = fs::remove_file(path.with_extension("sqlite3-shm"));
    }

    #[test]
    fn recall_returns_scoped_evidence_brief_and_audits_decision() {
        let path = temporary_database("recall-materialized");
        let store = KernelStore::open(&path).unwrap();
        let first = event_with("test:message-1", "项目整理完成", 1_000);
        let second = event_with("test:message-2", "决定先做手动验证", 2_000);
        let first_id = store.ingest_event(&first).unwrap().event_id;
        let second_id = store.ingest_event(&second).unwrap().event_id;
        let request = journal_request(first.scope.clone());
        let proposal = journal_proposal(&request, vec![first_id, second_id]);
        store
            .save_journal(
                &request,
                &proposal,
                &serde_json::to_string(&proposal).unwrap(),
            )
            .unwrap();

        let recall = store
            .request_recall(
                &RecallRequest {
                    protocol: PROTOCOL_VERSION.into(),
                    scope: first.scope.clone(),
                    need_type: NeedType::Fact,
                    reason: "需要确认项目整理是否已完成".into(),
                    cues: vec!["项目整理".into()],
                    depth: RecallDepth::Glance,
                    time_hint: Some("1970-01-01".into()),
                },
                "recall-request-1",
            )
            .unwrap();
        assert_eq!(recall.status, "ok");
        assert_eq!(recall.items.len(), 1);
        assert_eq!(recall.items[0].memory_kind, "fact");
        assert_eq!(recall.items[0].occurred_at_ms, 1_000);
        assert_eq!(recall.items[0].evidence_status, "supported");
        assert!(!recall.items[0].evidence_event_ids.is_empty());
        assert!(recall.brief.as_ref().unwrap().contains("日期 1970-01-01"));
        assert!(recall.brief.as_ref().unwrap().contains("类型 fact"));
        assert!(recall.brief.as_ref().unwrap().chars().count() <= 800);

        let unmatched = store
            .request_recall(
                &RecallRequest {
                    protocol: PROTOCOL_VERSION.into(),
                    scope: first.scope.clone(),
                    need_type: NeedType::Fact,
                    reason: "需要核对一个不存在的主题".into(),
                    cues: vec!["完全不存在".into()],
                    depth: RecallDepth::Glance,
                    time_hint: None,
                },
                "recall-request-2",
            )
            .unwrap();
        assert_eq!(unmatched.status, "not_found");
        assert!(unmatched.brief.is_none());
        let isolated = store
            .request_recall(
                &RecallRequest {
                    protocol: PROTOCOL_VERSION.into(),
                    scope: ScopeDescriptor {
                        platform_id: "test".into(),
                        bot_account_id: "bot".into(),
                        kind: ScopeKind::Private,
                        session_id: "other-user".into(),
                        persona_id: "default".into(),
                    },
                    need_type: NeedType::Fact,
                    reason: "验证会话隔离".into(),
                    cues: vec!["项目整理".into()],
                    depth: RecallDepth::Glance,
                    time_hint: Some("1970-01-01".into()),
                },
                "recall-request-isolated",
            )
            .unwrap();
        assert_eq!(isolated.status, "not_found");
        drop(store);
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(path.with_extension("sqlite3-wal"));
        let _ = fs::remove_file(path.with_extension("sqlite3-shm"));
    }

    #[test]
    fn preference_claims_are_recalled_only_as_preferences() {
        let path = temporary_database("preference-recall");
        let store = KernelStore::open(&path).unwrap();
        let first = event_with("test:preference-1", "我偏好简洁回答", 1_000);
        let second = event_with("test:preference-2", "项目结构已整理", 2_000);
        let first_id = store.ingest_event(&first).unwrap().event_id;
        let second_id = store.ingest_event(&second).unwrap().event_id;
        let request = journal_request(first.scope.clone());
        let mut proposal = journal_proposal(&request, vec![first_id.clone(), second_id]);
        proposal.claims.push(JournalClaimProposal {
            claim_kind: JournalClaimKind::Preference,
            claim_text: "用户偏好简洁回答。".into(),
            epistemic_status: JournalEpistemicStatus::AssistantInferred,
            evidence_event_ids: vec![first_id],
        });
        store
            .save_journal(
                &request,
                &proposal,
                &serde_json::to_string(&proposal).unwrap(),
            )
            .unwrap();

        let preference = store
            .request_recall(
                &RecallRequest {
                    protocol: PROTOCOL_VERSION.into(),
                    scope: first.scope.clone(),
                    need_type: NeedType::Preference,
                    reason: "回答风格取决于用户偏好".into(),
                    cues: vec!["简洁回答".into()],
                    depth: RecallDepth::Glance,
                    time_hint: None,
                },
                "preference-recall",
            )
            .unwrap();
        assert_eq!(preference.status, "ok");
        assert_eq!(preference.items.len(), 1);
        assert!(preference.items[0].statement.contains("简洁"));

        let fact = store
            .request_recall(
                &RecallRequest {
                    protocol: PROTOCOL_VERSION.into(),
                    scope: first.scope,
                    need_type: NeedType::Fact,
                    reason: "类型隔离回归测试".into(),
                    cues: vec!["简洁回答".into()],
                    depth: RecallDepth::Glance,
                    time_hint: None,
                },
                "fact-does-not-read-preference",
            )
            .unwrap();
        assert_eq!(fact.status, "not_found");
        drop(store);
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(path.with_extension("sqlite3-wal"));
        let _ = fs::remove_file(path.with_extension("sqlite3-shm"));
    }

    #[test]
    fn recall_depth_controls_item_and_brief_budget() {
        let path = temporary_database("recall-depth-budget");
        let store = KernelStore::open(&path).unwrap();
        let first = event_with("test:depth-1", "项目整理主题一", 1_000);
        let second = event_with("test:depth-2", "项目整理主题二", 2_000);
        let first_id = store.ingest_event(&first).unwrap().event_id;
        let second_id = store.ingest_event(&second).unwrap().event_id;
        let request = journal_request(first.scope.clone());
        let mut proposal = journal_proposal(&request, vec![first_id.clone(), second_id.clone()]);
        proposal.claims.push(JournalClaimProposal {
            claim_kind: JournalClaimKind::Fact,
            claim_text: "项目整理包含主题二。".into(),
            epistemic_status: JournalEpistemicStatus::AssistantInferred,
            evidence_event_ids: vec![second_id],
        });
        proposal.claims.push(JournalClaimProposal {
            claim_kind: JournalClaimKind::Fact,
            claim_text: "项目整理保留了主题三。".into(),
            epistemic_status: JournalEpistemicStatus::AssistantInferred,
            evidence_event_ids: vec![first_id],
        });
        store
            .save_journal(
                &request,
                &proposal,
                &serde_json::to_string(&proposal).unwrap(),
            )
            .unwrap();

        for (depth, expected_items, maximum_chars) in [
            (RecallDepth::Glance, 1, 420),
            (RecallDepth::Focused, 2, 620),
            (RecallDepth::Deep, 3, 800),
        ] {
            let recall = store
                .request_recall(
                    &RecallRequest {
                        protocol: PROTOCOL_VERSION.into(),
                        scope: first.scope.clone(),
                        need_type: NeedType::Fact,
                        reason: "验证回忆深度预算".into(),
                        cues: vec!["项目整理".into()],
                        depth,
                        time_hint: None,
                    },
                    "recall-depth-budget",
                )
                .unwrap();
            assert_eq!(recall.items.len(), expected_items);
            assert!(recall.brief.unwrap().chars().count() <= maximum_chars);
        }
        drop(store);
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(path.with_extension("sqlite3-wal"));
        let _ = fs::remove_file(path.with_extension("sqlite3-shm"));
    }

    #[test]
    fn terminal_memory_state_is_not_reactivated_or_bypassed_by_episode_fallback() {
        let path = temporary_database("terminal-memory-state");
        let store = KernelStore::open(&path).unwrap();
        let first = event_with("test:terminal-1", "项目结构已整理", 1_000);
        let second = event_with("test:terminal-2", "决定继续验证", 2_000);
        let first_id = store.ingest_event(&first).unwrap().event_id;
        let second_id = store.ingest_event(&second).unwrap().event_id;
        let request = journal_request(first.scope.clone());
        let proposal = journal_proposal(&request, vec![first_id, second_id]);
        store
            .save_journal(
                &request,
                &proposal,
                &serde_json::to_string(&proposal).unwrap(),
            )
            .unwrap();

        let scope_id = first.scope.id().unwrap();
        let memory_id = journal_memory_id(&scope_id, "fact", "今天整理了项目结构。");
        {
            let connection = store.connection.lock().unwrap();
            assert_eq!(
                connection
                    .execute(
                        "UPDATE memory_atoms SET state = 'archived' WHERE memory_id = ?1",
                        [&memory_id],
                    )
                    .unwrap(),
                1
            );
        }

        let mut updated_proposal = proposal.clone();
        updated_proposal.proposal_id = "proposal-terminal-replay".into();
        updated_proposal.summary = "再次整理了项目结构，并继续进行验证。".into();
        store
            .save_journal(
                &request,
                &updated_proposal,
                &serde_json::to_string(&updated_proposal).unwrap(),
            )
            .unwrap();

        {
            let connection = store.connection.lock().unwrap();
            let state: String = connection
                .query_row(
                    "SELECT state FROM memory_atoms WHERE memory_id = ?1",
                    [&memory_id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(state, "archived");
            let rejected_operations: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM memory_operations
                     WHERE scope_id = ?1 AND decision = 'rejected'
                       AND decision_reason LIKE 'terminal memory state%'",
                    [&scope_id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(rejected_operations, 1);
        }

        let recall = store
            .request_recall(
                &RecallRequest {
                    protocol: PROTOCOL_VERSION.into(),
                    scope: first.scope,
                    need_type: NeedType::Fact,
                    reason: "确认归档记忆不会从日记回退路径重新出现".into(),
                    cues: vec!["项目结构".into()],
                    depth: RecallDepth::Glance,
                    time_hint: None,
                },
                "terminal-memory-recall",
            )
            .unwrap();
        assert_eq!(recall.status, "not_found");

        drop(store);
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(path.with_extension("sqlite3-wal"));
        let _ = fs::remove_file(path.with_extension("sqlite3-shm"));
    }

    #[test]
    fn activation_decay_is_applied_during_maintenance() {
        let path = temporary_database("activation-decay");
        let store = KernelStore::open(&path).unwrap();
        let first = event_with("test:decay-1", "长期偏好简洁", 1_000);
        let second = event_with("test:decay-2", "已经完成项目整理", 2_000);
        let first_id = store.ingest_event(&first).unwrap().event_id;
        let second_id = store.ingest_event(&second).unwrap().event_id;
        let request = journal_request(first.scope.clone());
        let proposal = journal_proposal(&request, vec![first_id, second_id]);
        store
            .save_journal(
                &request,
                &proposal,
                &serde_json::to_string(&proposal).unwrap(),
            )
            .unwrap();
        let scope_id = first.scope.id().unwrap();
        {
            let connection = store.connection.lock().unwrap();
            connection
                .execute(
                    "UPDATE memory_atoms
                     SET activation = 1.0, updated_at_ms = 0
                     WHERE scope_id = ?1",
                    [&scope_id],
                )
                .unwrap();
        }
        let retention = store
            .retain_payloads(&RetentionRequest {
                protocol: PROTOCOL_VERSION.into(),
                scope: first.scope,
                cutoff_at_ms: 0,
                actor_id: "user".into(),
                reason: "activation decay test".into(),
            })
            .unwrap();
        assert_eq!(retention.decayed_memories, 2);
        let connection = store.connection.lock().unwrap();
        let minimum_activation: f64 = connection
            .query_row(
                "SELECT MIN(activation) FROM memory_atoms WHERE scope_id = ?1",
                [&scope_id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(minimum_activation < 1.0);
        drop(connection);
        drop(store);
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(path.with_extension("sqlite3-wal"));
        let _ = fs::remove_file(path.with_extension("sqlite3-shm"));
    }

    #[test]
    fn capture_policy_retention_and_memory_cards_are_scope_bound() {
        let path = temporary_database("privacy-controls");
        let store = KernelStore::open(&path).unwrap();
        let first = event_with("test:message-1", "项目整理完成", 1_000);
        let second = event_with("test:message-2", "决定先做手动验证", 2_000);
        let first_id = store.ingest_event(&first).unwrap().event_id;
        let second_id = store.ingest_event(&second).unwrap().event_id;
        let request = journal_request(first.scope.clone());
        let proposal = journal_proposal(&request, vec![first_id, second_id]);
        store
            .save_journal(
                &request,
                &proposal,
                &serde_json::to_string(&proposal).unwrap(),
            )
            .unwrap();
        let stats = store
            .scope_stats(&ScopeStatsRequest {
                protocol: PROTOCOL_VERSION.into(),
                scope: first.scope.clone(),
            })
            .unwrap();
        assert_eq!(stats.active_memories, 2);

        let duplicate_proposal = DailyJournalProposal {
            proposal_id: "proposal-duplicate".into(),
            claims: vec![proposal.claims[0].clone()],
            ..proposal.clone()
        };
        store
            .save_journal(
                &request,
                &duplicate_proposal,
                &serde_json::to_string(&duplicate_proposal).unwrap(),
            )
            .unwrap();
        let deduplicated = store
            .scope_stats(&ScopeStatsRequest {
                protocol: PROTOCOL_VERSION.into(),
                scope: first.scope.clone(),
            })
            .unwrap();
        assert_eq!(deduplicated.active_memories, 2);

        let paused = store
            .set_capture_policy(&CapturePolicyRequest {
                protocol: PROTOCOL_VERSION.into(),
                scope: first.scope.clone(),
                action: CapturePolicyAction::Pause,
                actor_id: "user".into(),
                reason: "test pause".into(),
            })
            .unwrap();
        assert_eq!(paused.policy_state, "opted_out");
        assert_eq!(
            store
                .ingest_event(&event_with("test:message-3", "不应采集", 3_000))
                .unwrap()
                .status,
            "opted_out"
        );

        store
            .set_capture_policy(&CapturePolicyRequest {
                protocol: PROTOCOL_VERSION.into(),
                scope: first.scope.clone(),
                action: CapturePolicyAction::Resume,
                actor_id: "user".into(),
                reason: "test resume".into(),
            })
            .unwrap();
        let retention = store
            .retain_payloads(&RetentionRequest {
                protocol: PROTOCOL_VERSION.into(),
                scope: first.scope.clone(),
                cutoff_at_ms: 2_000,
                actor_id: "user".into(),
                reason: "test retention".into(),
            })
            .unwrap();
        assert_eq!(retention.purged_payloads, 1);
        assert_eq!(retention.residual_scan, "passed");
        let stats = store
            .scope_stats(&ScopeStatsRequest {
                protocol: PROTOCOL_VERSION.into(),
                scope: first.scope,
            })
            .unwrap();
        assert_eq!(stats.active_payloads, 1);
        drop(store);
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(path.with_extension("sqlite3-wal"));
        let _ = fs::remove_file(path.with_extension("sqlite3-shm"));
    }
}
