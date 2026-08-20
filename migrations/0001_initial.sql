PRAGMA foreign_keys = ON;

BEGIN IMMEDIATE;

CREATE TABLE IF NOT EXISTS schema_migrations (
    version INTEGER PRIMARY KEY,
    applied_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS scopes (
    scope_id TEXT PRIMARY KEY,
    platform_id TEXT NOT NULL,
    bot_account_id TEXT NOT NULL,
    conversation_kind TEXT NOT NULL CHECK (conversation_kind IN ('private', 'group')),
    session_id TEXT NOT NULL,
    persona_id TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    UNIQUE (platform_id, bot_account_id, conversation_kind, session_id, persona_id)
);

CREATE TABLE IF NOT EXISTS raw_events (
    event_id TEXT PRIMARY KEY,
    scope_id TEXT NOT NULL REFERENCES scopes(scope_id) ON DELETE RESTRICT,
    source_key TEXT NOT NULL,
    sender_account_id TEXT NOT NULL,
    sender_display_name TEXT NOT NULL,
    sender_is_bot INTEGER NOT NULL CHECK (sender_is_bot IN (0, 1)),
    observed_at_ms INTEGER NOT NULL CHECK (observed_at_ms >= 0),
    occurred_at_ms INTEGER NOT NULL CHECK (occurred_at_ms >= 0),
    content TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    reply_to_source_key TEXT,
    mentions_json TEXT NOT NULL,
    metadata_json TEXT NOT NULL,
    ingested_at_ms INTEGER NOT NULL,
    UNIQUE (scope_id, source_key)
);

CREATE INDEX IF NOT EXISTS idx_raw_events_scope_time
    ON raw_events(scope_id, occurred_at_ms DESC);

CREATE TRIGGER IF NOT EXISTS raw_events_no_update
BEFORE UPDATE ON raw_events
BEGIN
    SELECT RAISE(ABORT, 'raw_events are append-only');
END;

CREATE TRIGGER IF NOT EXISTS raw_events_no_delete
BEFORE DELETE ON raw_events
BEGIN
    SELECT RAISE(ABORT, 'raw_events are append-only');
END;

CREATE TABLE IF NOT EXISTS memory_operations (
    operation_id TEXT PRIMARY KEY,
    scope_id TEXT NOT NULL REFERENCES scopes(scope_id) ON DELETE RESTRICT,
    proposal_id TEXT,
    operation_kind TEXT NOT NULL CHECK (
        operation_kind IN ('create', 'strengthen', 'revise', 'supersede', 'link', 'archive')
    ),
    actor_kind TEXT NOT NULL CHECK (actor_kind IN ('kernel', 'semantic_worker', 'user')),
    actor_id TEXT NOT NULL,
    request_json TEXT NOT NULL,
    decision TEXT NOT NULL CHECK (decision IN ('accepted', 'rejected')),
    decision_reason TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_memory_operations_scope_time
    ON memory_operations(scope_id, created_at_ms DESC);

CREATE TABLE IF NOT EXISTS memory_atoms (
    memory_id TEXT PRIMARY KEY,
    scope_id TEXT NOT NULL REFERENCES scopes(scope_id) ON DELETE RESTRICT,
    memory_kind TEXT NOT NULL CHECK (
        memory_kind IN ('fact', 'preference', 'decision', 'commitment', 'relationship', 'episode', 'procedure')
    ),
    state TEXT NOT NULL CHECK (state IN ('candidate', 'active', 'superseded', 'archived')),
    confidence REAL NOT NULL CHECK (confidence >= 0.0 AND confidence <= 1.0),
    importance REAL NOT NULL CHECK (importance >= 0.0 AND importance <= 1.0),
    activation REAL NOT NULL CHECK (activation >= 0.0),
    current_version INTEGER NOT NULL CHECK (current_version >= 1),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_memory_atoms_scope_state
    ON memory_atoms(scope_id, state, memory_kind);

CREATE TABLE IF NOT EXISTS memory_versions (
    memory_id TEXT NOT NULL REFERENCES memory_atoms(memory_id) ON DELETE RESTRICT,
    version INTEGER NOT NULL CHECK (version >= 1),
    statement TEXT NOT NULL,
    qualifiers_json TEXT NOT NULL,
    valid_from_ms INTEGER,
    valid_to_ms INTEGER,
    operation_id TEXT NOT NULL REFERENCES memory_operations(operation_id) ON DELETE RESTRICT,
    created_at_ms INTEGER NOT NULL,
    PRIMARY KEY (memory_id, version),
    CHECK (valid_to_ms IS NULL OR valid_from_ms IS NULL OR valid_to_ms >= valid_from_ms)
);

CREATE TABLE IF NOT EXISTS memory_evidence (
    memory_id TEXT NOT NULL,
    memory_version INTEGER NOT NULL,
    event_id TEXT NOT NULL REFERENCES raw_events(event_id) ON DELETE RESTRICT,
    evidence_role TEXT NOT NULL CHECK (evidence_role IN ('supports', 'contradicts', 'context')),
    PRIMARY KEY (memory_id, memory_version, event_id, evidence_role),
    FOREIGN KEY (memory_id, memory_version)
        REFERENCES memory_versions(memory_id, version) ON DELETE RESTRICT
);

CREATE INDEX IF NOT EXISTS idx_memory_evidence_event
    ON memory_evidence(event_id);

CREATE TABLE IF NOT EXISTS memory_edges (
    edge_id TEXT PRIMARY KEY,
    scope_id TEXT NOT NULL REFERENCES scopes(scope_id) ON DELETE RESTRICT,
    from_memory_id TEXT NOT NULL REFERENCES memory_atoms(memory_id) ON DELETE RESTRICT,
    to_memory_id TEXT NOT NULL REFERENCES memory_atoms(memory_id) ON DELETE RESTRICT,
    edge_kind TEXT NOT NULL CHECK (
        edge_kind IN ('same_entity', 'same_episode', 'causes', 'updates', 'contradicts', 'supports', 'associated')
    ),
    weight REAL NOT NULL CHECK (weight >= 0.0 AND weight <= 1.0),
    operation_id TEXT NOT NULL REFERENCES memory_operations(operation_id) ON DELETE RESTRICT,
    created_at_ms INTEGER NOT NULL,
    CHECK (from_memory_id <> to_memory_id),
    UNIQUE (scope_id, from_memory_id, to_memory_id, edge_kind)
);

CREATE TABLE IF NOT EXISTS recall_audits (
    recall_id TEXT PRIMARY KEY,
    scope_id TEXT NOT NULL REFERENCES scopes(scope_id) ON DELETE RESTRICT,
    need_type TEXT NOT NULL,
    depth TEXT NOT NULL CHECK (depth IN ('glance', 'focused', 'deep')),
    reason TEXT NOT NULL,
    cues_json TEXT NOT NULL,
    time_hint TEXT,
    decision TEXT NOT NULL CHECK (decision IN ('ok', 'not_found', 'declined', 'error')),
    decision_reason TEXT NOT NULL,
    exposed_memory_ids_json TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_recall_audits_scope_time
    ON recall_audits(scope_id, created_at_ms DESC);

CREATE TABLE IF NOT EXISTS maintenance_jobs (
    job_id TEXT PRIMARY KEY,
    scope_id TEXT REFERENCES scopes(scope_id) ON DELETE RESTRICT,
    job_kind TEXT NOT NULL CHECK (
        job_kind IN ('episode_build', 'consolidate', 'decay', 'consistency_scan', 'checkpoint')
    ),
    state TEXT NOT NULL CHECK (state IN ('pending', 'running', 'succeeded', 'failed')),
    cursor_json TEXT NOT NULL,
    attempt INTEGER NOT NULL DEFAULT 0 CHECK (attempt >= 0),
    error_text TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

INSERT OR IGNORE INTO schema_migrations(version, applied_at_ms)
VALUES (1, CAST(strftime('%s', 'now') AS INTEGER) * 1000);

COMMIT;

