PRAGMA foreign_keys = ON;

BEGIN IMMEDIATE;

CREATE TABLE episode_claims_v4 (
    claim_id TEXT PRIMARY KEY,
    episode_id TEXT NOT NULL,
    episode_version INTEGER NOT NULL,
    claim_kind TEXT NOT NULL CHECK (
        claim_kind IN (
            'fact', 'preference', 'decision', 'commitment',
            'relationship', 'procedure', 'open_loop'
        )
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

INSERT INTO episode_claims_v4(
    claim_id, episode_id, episode_version, claim_kind,
    claim_text, epistemic_status, created_at_ms
)
SELECT
    claim_id, episode_id, episode_version, claim_kind,
    claim_text, epistemic_status, created_at_ms
FROM episode_claims;

CREATE TABLE episode_evidence_v4 (
    claim_id TEXT NOT NULL,
    episode_id TEXT NOT NULL,
    episode_version INTEGER NOT NULL,
    scope_id TEXT NOT NULL,
    event_id TEXT NOT NULL,
    evidence_role TEXT NOT NULL CHECK (evidence_role IN ('supports', 'context')),
    PRIMARY KEY (claim_id, event_id, evidence_role),
    FOREIGN KEY (claim_id, episode_id, episode_version)
        REFERENCES episode_claims_v4(claim_id, episode_id, episode_version) ON DELETE RESTRICT,
    FOREIGN KEY (episode_id, scope_id)
        REFERENCES episodes(episode_id, scope_id) ON DELETE RESTRICT,
    FOREIGN KEY (event_id, scope_id)
        REFERENCES raw_events(event_id, scope_id) ON DELETE RESTRICT
);

INSERT INTO episode_evidence_v4(
    claim_id, episode_id, episode_version, scope_id, event_id, evidence_role
)
SELECT claim_id, episode_id, episode_version, scope_id, event_id, evidence_role
FROM episode_evidence;

DROP TABLE episode_evidence;
DROP TABLE episode_claims;

ALTER TABLE episode_claims_v4 RENAME TO episode_claims;
ALTER TABLE episode_evidence_v4 RENAME TO episode_evidence;

CREATE INDEX idx_episode_evidence_event
    ON episode_evidence(event_id);

INSERT OR IGNORE INTO schema_migrations(version, applied_at_ms)
VALUES (4, CAST(strftime('%s', 'now') AS INTEGER) * 1000);

COMMIT;
