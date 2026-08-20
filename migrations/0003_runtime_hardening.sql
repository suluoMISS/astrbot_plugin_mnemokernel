PRAGMA foreign_keys = ON;

BEGIN IMMEDIATE;

ALTER TABLE raw_events ADD COLUMN origin_kind TEXT NOT NULL
    DEFAULT 'user_message'
    CHECK (origin_kind IN ('user_message', 'bot_response'));

ALTER TABLE raw_events ADD COLUMN occurred_time_source TEXT NOT NULL
    DEFAULT 'legacy_unknown'
    CHECK (occurred_time_source IN ('platform', 'observed_fallback', 'legacy_unknown'));

ALTER TABLE raw_events ADD COLUMN platform_message_id_state TEXT NOT NULL
    DEFAULT 'legacy_unknown'
    CHECK (platform_message_id_state IN ('present', 'missing', 'legacy_unknown'));

ALTER TABLE episode_jobs ADD COLUMN lease_owner TEXT;

CREATE TABLE IF NOT EXISTS migration_manifest (
    version INTEGER PRIMARY KEY REFERENCES schema_migrations(version) ON DELETE RESTRICT,
    name TEXT NOT NULL UNIQUE,
    sha256 TEXT NOT NULL CHECK (
        length(sha256) = 64 AND sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    applied_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS job_transitions (
    transition_id TEXT PRIMARY KEY,
    job_id TEXT NOT NULL REFERENCES episode_jobs(job_id) ON DELETE RESTRICT,
    from_state TEXT,
    to_state TEXT NOT NULL CHECK (
        to_state IN ('pending', 'leased', 'retry_wait', 'succeeded', 'dead_letter')
    ),
    actor_kind TEXT NOT NULL CHECK (actor_kind IN ('kernel', 'worker', 'user')),
    actor_id TEXT NOT NULL,
    reason_kind TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_job_transitions_job_time
    ON job_transitions(job_id, created_at_ms);

CREATE TABLE IF NOT EXISTS payload_retention_runs (
    run_id TEXT PRIMARY KEY,
    cutoff_at_ms INTEGER NOT NULL CHECK (cutoff_at_ms >= 0),
    payload_count INTEGER NOT NULL CHECK (payload_count >= 0),
    residual_scan TEXT NOT NULL CHECK (residual_scan IN ('pending', 'passed', 'failed')),
    started_at_ms INTEGER NOT NULL,
    completed_at_ms INTEGER NOT NULL,
    CHECK (completed_at_ms >= started_at_ms)
);

CREATE TABLE IF NOT EXISTS scope_capture_policies (
    scope_id TEXT PRIMARY KEY REFERENCES scopes(scope_id) ON DELETE RESTRICT,
    policy_state TEXT NOT NULL CHECK (policy_state IN ('active', 'opted_out')),
    actor_id TEXT NOT NULL,
    reason_kind TEXT NOT NULL CHECK (
        reason_kind IN ('explicit_enable', 'explicit_forgetme', 'administrator_policy')
    ),
    updated_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS privacy_confirmations (
    token_digest TEXT PRIMARY KEY CHECK (
        length(token_digest) = 64 AND token_digest NOT GLOB '*[^0-9a-f]*'
    ),
    scope_id TEXT NOT NULL REFERENCES scopes(scope_id) ON DELETE RESTRICT,
    actor_id TEXT NOT NULL,
    action_kind TEXT NOT NULL CHECK (action_kind IN ('purge_scope', 'resume_capture')),
    confirmation_state TEXT NOT NULL CHECK (
        confirmation_state IN ('pending', 'consumed', 'expired')
    ),
    created_at_ms INTEGER NOT NULL,
    expires_at_ms INTEGER NOT NULL,
    consumed_at_ms INTEGER,
    CHECK (expires_at_ms >= created_at_ms),
    CHECK (
        (confirmation_state = 'pending' AND consumed_at_ms IS NULL)
        OR (confirmation_state IN ('consumed', 'expired') AND consumed_at_ms IS NOT NULL)
    )
);

CREATE INDEX IF NOT EXISTS idx_privacy_confirmations_expiry
    ON privacy_confirmations(confirmation_state, expires_at_ms);

CREATE UNIQUE INDEX IF NOT EXISTS uq_episode_jobs_scope_leased
    ON episode_jobs(scope_id) WHERE state = 'leased';

CREATE INDEX IF NOT EXISTS idx_raw_events_scope_ingest
    ON raw_events(scope_id, ingested_at_ms, event_id);

CREATE TRIGGER IF NOT EXISTS episode_jobs_state_coherence_insert
BEFORE INSERT ON episode_jobs
WHEN NOT (
    (NEW.state = 'pending'
        AND NEW.lease_owner IS NULL AND NEW.lease_token IS NULL
        AND NEW.lease_expires_at_ms IS NULL
        AND NEW.completed_at_ms IS NULL)
    OR (NEW.state = 'leased'
        AND NEW.lease_owner IS NOT NULL AND NEW.lease_token IS NOT NULL
        AND NEW.lease_expires_at_ms IS NOT NULL
        AND NEW.completed_at_ms IS NULL)
    OR (NEW.state = 'retry_wait'
        AND NEW.lease_owner IS NULL AND NEW.lease_token IS NULL
        AND NEW.lease_expires_at_ms IS NULL
        AND NEW.last_error_kind IS NOT NULL AND NEW.completed_at_ms IS NULL)
    OR (NEW.state = 'succeeded'
        AND NEW.lease_owner IS NULL AND NEW.lease_token IS NULL
        AND NEW.lease_expires_at_ms IS NULL
        AND NEW.proposal_digest IS NOT NULL AND NEW.completed_at_ms IS NOT NULL)
    OR (NEW.state = 'dead_letter'
        AND NEW.lease_owner IS NULL AND NEW.lease_token IS NULL
        AND NEW.lease_expires_at_ms IS NULL
        AND NEW.last_error_kind IS NOT NULL AND NEW.completed_at_ms IS NOT NULL)
)
BEGIN
    SELECT RAISE(ABORT, 'episode job fields are inconsistent with state');
END;

CREATE TRIGGER IF NOT EXISTS episode_jobs_state_coherence_update
BEFORE UPDATE ON episode_jobs
WHEN NOT (
    (NEW.state = 'pending'
        AND NEW.lease_owner IS NULL AND NEW.lease_token IS NULL
        AND NEW.lease_expires_at_ms IS NULL
        AND NEW.completed_at_ms IS NULL)
    OR (NEW.state = 'leased'
        AND NEW.lease_owner IS NOT NULL AND NEW.lease_token IS NOT NULL
        AND NEW.lease_expires_at_ms IS NOT NULL
        AND NEW.completed_at_ms IS NULL)
    OR (NEW.state = 'retry_wait'
        AND NEW.lease_owner IS NULL AND NEW.lease_token IS NULL
        AND NEW.lease_expires_at_ms IS NULL
        AND NEW.last_error_kind IS NOT NULL AND NEW.completed_at_ms IS NULL)
    OR (NEW.state = 'succeeded'
        AND NEW.lease_owner IS NULL AND NEW.lease_token IS NULL
        AND NEW.lease_expires_at_ms IS NULL
        AND NEW.proposal_digest IS NOT NULL AND NEW.completed_at_ms IS NOT NULL)
    OR (NEW.state = 'dead_letter'
        AND NEW.lease_owner IS NULL AND NEW.lease_token IS NULL
        AND NEW.lease_expires_at_ms IS NULL
        AND NEW.last_error_kind IS NOT NULL AND NEW.completed_at_ms IS NOT NULL)
)
BEGIN
    SELECT RAISE(ABORT, 'episode job fields are inconsistent with state');
END;

CREATE TRIGGER IF NOT EXISTS job_event_admissions_no_update
BEFORE UPDATE ON job_event_admissions
BEGIN
    SELECT RAISE(ABORT, 'job event admissions are immutable');
END;

CREATE TRIGGER IF NOT EXISTS job_event_admissions_no_delete
BEFORE DELETE ON job_event_admissions
BEGIN
    SELECT RAISE(ABORT, 'job event admissions are immutable');
END;

INSERT OR IGNORE INTO schema_migrations(version, applied_at_ms)
VALUES (3, CAST(strftime('%s', 'now') AS INTEGER) * 1000);

COMMIT;
