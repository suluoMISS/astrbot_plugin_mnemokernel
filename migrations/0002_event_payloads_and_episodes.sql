PRAGMA foreign_keys = ON;

BEGIN IMMEDIATE;

-- raw_events remains an append-only identity/time envelope. Fields retained in
-- the v1 table for compatibility are overwritten with neutral sentinels; their
-- privacy-sensitive values live only in this revocable payload table.
CREATE TABLE IF NOT EXISTS raw_event_payloads (
    event_id TEXT PRIMARY KEY REFERENCES raw_events(event_id) ON DELETE RESTRICT,
    payload_state TEXT NOT NULL CHECK (payload_state IN ('active', 'redacted', 'purged')),
    sender_display_name TEXT,
    content TEXT,
    mentions_json TEXT,
    metadata_json TEXT,
    redaction_map_json TEXT NOT NULL DEFAULT '[]',
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    purged_at_ms INTEGER,
    CHECK (
        (payload_state IN ('active', 'redacted')
            AND sender_display_name IS NOT NULL
            AND content IS NOT NULL
            AND mentions_json IS NOT NULL
            AND metadata_json IS NOT NULL
            AND purged_at_ms IS NULL)
        OR
        (payload_state = 'purged'
            AND sender_display_name IS NULL
            AND content IS NULL
            AND mentions_json IS NULL
            AND metadata_json IS NULL
            AND purged_at_ms IS NOT NULL)
    )
);

CREATE INDEX IF NOT EXISTS idx_raw_event_payloads_state
    ON raw_event_payloads(payload_state, updated_at_ms);

CREATE UNIQUE INDEX IF NOT EXISTS uq_raw_events_event_scope
    ON raw_events(event_id, scope_id);

CREATE TABLE IF NOT EXISTS episode_jobs (
    job_id TEXT PRIMARY KEY,
    scope_id TEXT NOT NULL REFERENCES scopes(scope_id) ON DELETE RESTRICT,
    job_kind TEXT NOT NULL CHECK (job_kind = 'episode_build'),
    idempotency_key TEXT NOT NULL,
    state TEXT NOT NULL CHECK (
        state IN ('pending', 'leased', 'retry_wait', 'succeeded', 'dead_letter')
    ),
    checkpoint_json TEXT NOT NULL,
    lease_token TEXT,
    lease_expires_at_ms INTEGER,
    attempt INTEGER NOT NULL DEFAULT 0 CHECK (attempt >= 0),
    max_attempts INTEGER NOT NULL CHECK (max_attempts > 0),
    last_error_kind TEXT,
    last_error_text TEXT,
    proposal_digest TEXT,
    created_at_ms INTEGER NOT NULL,
    available_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    completed_at_ms INTEGER,
    UNIQUE (scope_id, idempotency_key),
    UNIQUE (job_id, scope_id),
    CHECK (
        (state = 'leased' AND lease_token IS NOT NULL AND lease_expires_at_ms IS NOT NULL)
        OR state <> 'leased'
    )
);

CREATE INDEX IF NOT EXISTS idx_episode_jobs_ready
    ON episode_jobs(state, available_at_ms);

CREATE TABLE IF NOT EXISTS job_event_admissions (
    job_id TEXT NOT NULL,
    scope_id TEXT NOT NULL,
    event_id TEXT NOT NULL,
    admitted_at_ms INTEGER NOT NULL,
    PRIMARY KEY (job_id, event_id),
    FOREIGN KEY (job_id, scope_id)
        REFERENCES episode_jobs(job_id, scope_id) ON DELETE RESTRICT,
    FOREIGN KEY (event_id, scope_id)
        REFERENCES raw_events(event_id, scope_id) ON DELETE RESTRICT
);

CREATE TABLE IF NOT EXISTS threads (
    thread_id TEXT PRIMARY KEY,
    scope_id TEXT NOT NULL REFERENCES scopes(scope_id) ON DELETE RESTRICT,
    state TEXT NOT NULL CHECK (state IN ('active', 'archived')),
    cue_key TEXT NOT NULL,
    started_at_ms INTEGER NOT NULL,
    last_event_at_ms INTEGER NOT NULL,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    UNIQUE (thread_id, scope_id),
    CHECK (last_event_at_ms >= started_at_ms)
);

CREATE INDEX IF NOT EXISTS idx_threads_scope_activity
    ON threads(scope_id, state, last_event_at_ms DESC);

CREATE TABLE IF NOT EXISTS episodes (
    episode_id TEXT PRIMARY KEY,
    scope_id TEXT NOT NULL REFERENCES scopes(scope_id) ON DELETE RESTRICT,
    state TEXT NOT NULL CHECK (state IN ('active', 'superseded', 'archived')),
    current_version INTEGER NOT NULL CHECK (current_version >= 1),
    occurred_from_ms INTEGER NOT NULL,
    occurred_to_ms INTEGER NOT NULL,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    UNIQUE (episode_id, scope_id),
    CHECK (occurred_to_ms >= occurred_from_ms)
);

CREATE INDEX IF NOT EXISTS idx_episodes_scope_time
    ON episodes(scope_id, state, occurred_from_ms DESC);

CREATE TABLE IF NOT EXISTS episode_versions (
    episode_id TEXT NOT NULL REFERENCES episodes(episode_id) ON DELETE RESTRICT,
    version INTEGER NOT NULL CHECK (version >= 1),
    title TEXT NOT NULL,
    summary TEXT NOT NULL,
    cues_json TEXT NOT NULL,
    open_loops_json TEXT NOT NULL,
    proposal_digest TEXT NOT NULL,
    job_id TEXT NOT NULL REFERENCES episode_jobs(job_id) ON DELETE RESTRICT,
    epistemic_status TEXT NOT NULL CHECK (epistemic_status = 'assistant_inferred'),
    created_at_ms INTEGER NOT NULL,
    PRIMARY KEY (episode_id, version)
);

CREATE TABLE IF NOT EXISTS episode_claims (
    claim_id TEXT PRIMARY KEY,
    episode_id TEXT NOT NULL,
    episode_version INTEGER NOT NULL,
    claim_kind TEXT NOT NULL CHECK (
        claim_kind IN ('fact', 'decision', 'commitment', 'relationship', 'procedure', 'open_loop')
    ),
    claim_text TEXT NOT NULL,
    epistemic_status TEXT NOT NULL CHECK (
        epistemic_status IN ('assistant_inferred', 'user_confirmed', 'disputed')
    ),
    created_at_ms INTEGER NOT NULL,
    UNIQUE (claim_id, episode_id, episode_version),
    FOREIGN KEY (episode_id, episode_version)
        REFERENCES episode_versions(episode_id, version) ON DELETE RESTRICT
);

CREATE TABLE IF NOT EXISTS episode_evidence (
    claim_id TEXT NOT NULL,
    episode_id TEXT NOT NULL,
    episode_version INTEGER NOT NULL,
    scope_id TEXT NOT NULL,
    event_id TEXT NOT NULL,
    evidence_role TEXT NOT NULL CHECK (evidence_role IN ('supports', 'context')),
    PRIMARY KEY (claim_id, event_id, evidence_role),
    FOREIGN KEY (claim_id, episode_id, episode_version)
        REFERENCES episode_claims(claim_id, episode_id, episode_version) ON DELETE RESTRICT,
    FOREIGN KEY (episode_id, scope_id)
        REFERENCES episodes(episode_id, scope_id) ON DELETE RESTRICT,
    FOREIGN KEY (event_id, scope_id)
        REFERENCES raw_events(event_id, scope_id) ON DELETE RESTRICT
);

CREATE INDEX IF NOT EXISTS idx_episode_evidence_event
    ON episode_evidence(event_id);

CREATE TABLE IF NOT EXISTS event_thread_links (
    thread_id TEXT NOT NULL,
    event_id TEXT NOT NULL,
    scope_id TEXT NOT NULL,
    link_weight REAL NOT NULL CHECK (link_weight >= 0.0 AND link_weight <= 1.0),
    source_kind TEXT NOT NULL CHECK (source_kind IN ('kernel', 'semantic_worker', 'user')),
    job_id TEXT REFERENCES episode_jobs(job_id) ON DELETE RESTRICT,
    created_at_ms INTEGER NOT NULL,
    PRIMARY KEY (thread_id, event_id),
    FOREIGN KEY (thread_id, scope_id)
        REFERENCES threads(thread_id, scope_id) ON DELETE RESTRICT,
    FOREIGN KEY (event_id, scope_id)
        REFERENCES raw_events(event_id, scope_id) ON DELETE RESTRICT
);

CREATE TABLE IF NOT EXISTS journal_exports (
    export_id TEXT PRIMARY KEY,
    scope_id TEXT NOT NULL REFERENCES scopes(scope_id) ON DELETE RESTRICT,
    journal_date TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('current', 'invalidated', 'deleted')),
    relative_path TEXT NOT NULL,
    content_sha256 TEXT NOT NULL,
    episode_set_digest TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    invalidated_at_ms INTEGER
);

CREATE UNIQUE INDEX IF NOT EXISTS uq_journal_exports_current
    ON journal_exports(scope_id, journal_date) WHERE state = 'current';

CREATE TABLE IF NOT EXISTS privacy_operations (
    operation_id TEXT PRIMARY KEY,
    scope_id TEXT NOT NULL REFERENCES scopes(scope_id) ON DELETE RESTRICT,
    operation_kind TEXT NOT NULL CHECK (operation_kind IN ('redact_payloads', 'purge_payloads')),
    actor_kind TEXT NOT NULL CHECK (actor_kind IN ('user', 'administrator', 'kernel')),
    actor_id TEXT NOT NULL,
    reason TEXT NOT NULL,
    payload_count INTEGER NOT NULL CHECK (payload_count >= 0),
    episode_count INTEGER NOT NULL CHECK (episode_count >= 0),
    journal_count INTEGER NOT NULL CHECK (journal_count >= 0),
    residual_scan TEXT NOT NULL CHECK (residual_scan IN ('pending', 'passed', 'failed')),
    created_at_ms INTEGER NOT NULL,
    completed_at_ms INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_privacy_operations_scope_time
    ON privacy_operations(scope_id, created_at_ms DESC);

-- Migrate v1 plaintext before restoring the append-only trigger. OR IGNORE is
-- intentional: replaying migrations must never resurrect an already purged
-- payload from the neutral sentinel fields in raw_events.
DROP TRIGGER IF EXISTS raw_events_no_update;

INSERT OR IGNORE INTO raw_event_payloads(
    event_id, payload_state, sender_display_name, content, mentions_json,
    metadata_json, redaction_map_json, created_at_ms, updated_at_ms, purged_at_ms
)
SELECT
    event_id, 'active', sender_display_name, content, mentions_json,
    metadata_json, '[]', ingested_at_ms, ingested_at_ms, NULL
FROM raw_events;

UPDATE raw_events
SET sender_display_name = '', content = '', mentions_json = '[]', metadata_json = '{}'
WHERE sender_display_name <> '' OR content <> '' OR mentions_json <> '[]' OR metadata_json <> '{}';

CREATE TRIGGER IF NOT EXISTS raw_events_no_update
BEFORE UPDATE ON raw_events
BEGIN
    SELECT RAISE(ABORT, 'raw_events are append-only');
END;

INSERT OR IGNORE INTO schema_migrations(version, applied_at_ms)
VALUES (2, CAST(strftime('%s', 'now') AS INTEGER) * 1000);

COMMIT;
