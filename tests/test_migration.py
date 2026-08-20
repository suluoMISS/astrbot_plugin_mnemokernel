import json
import sqlite3
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


def migration_bundle() -> str:
    return "\n".join(
        path.read_text(encoding="utf-8")
        for path in sorted((ROOT / "migrations").glob("*.sql"))
    )


class MigrationTests(unittest.TestCase):
    def test_schema_and_json_contracts_parse(self):
        connection = sqlite3.connect(":memory:")
        connection.executescript(migration_bundle())
        version = connection.execute(
            "SELECT MAX(version) FROM schema_migrations"
        ).fetchone()[0]
        self.assertEqual(version, 4)
        connection.close()

        for schema in (ROOT / "schemas").glob("*.json"):
            with self.subTest(schema=schema.name):
                parsed = json.loads(schema.read_text(encoding="utf-8"))
                self.assertEqual(parsed["$schema"], "https://json-schema.org/draft/2020-12/schema")

    def test_v4_preserves_claim_evidence_and_adds_preference_kind(self):
        connection = sqlite3.connect(":memory:")
        for name in (
            "0001_initial.sql",
            "0002_event_payloads_and_episodes.sql",
            "0003_runtime_hardening.sql",
        ):
            connection.executescript(
                (ROOT / "migrations" / name).read_text(encoding="utf-8")
            )
        connection.execute(
            "INSERT INTO scopes VALUES (?, ?, ?, ?, ?, ?, ?)",
            ("scope", "test", "bot", "private", "user", "default", 1),
        )
        connection.execute(
            """INSERT INTO raw_events VALUES
               (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)""",
            (
                "event", "scope", "test:1", "user", "", 0, 1, 1,
                "", "hash", None, "[]", "{}", 1,
                "user_message", "platform", "present",
            ),
        )
        connection.execute(
            """INSERT INTO raw_event_payloads VALUES
               ('event', 'active', 'User', 'evidence', '[]', '{}', '[]', 1, 1, NULL)"""
        )
        connection.execute(
            """INSERT INTO episode_jobs(
                   job_id, scope_id, job_kind, idempotency_key, state, checkpoint_json,
                   lease_owner, lease_token, lease_expires_at_ms, attempt, max_attempts,
                   last_error_kind, last_error_text, proposal_digest, created_at_ms,
                   available_at_ms, updated_at_ms, completed_at_ms
               ) VALUES (
                   'job', 'scope', 'episode_build', '1970-01-01', 'succeeded', '{}',
                   NULL, NULL, NULL, 0, 1, NULL, NULL, 'digest', 1, 1, 1, 1
               )"""
        )
        connection.execute(
            "INSERT INTO episodes VALUES ('episode', 'scope', 'active', 1, 0, 10, 1, 1)"
        )
        connection.execute(
            """INSERT INTO episode_versions VALUES
               ('episode', 1, 'title', 'summary', '[]', '[]', 'digest', 'job',
                'assistant_inferred', 1)"""
        )
        connection.execute(
            """INSERT INTO episode_claims VALUES
               ('claim', 'episode', 1, 'fact', 'existing claim',
                'assistant_inferred', 1)"""
        )
        connection.execute(
            """INSERT INTO episode_evidence VALUES
               ('claim', 'episode', 1, 'scope', 'event', 'supports')"""
        )
        connection.commit()

        connection.executescript(
            (ROOT / "migrations" / "0004_preference_claims.sql").read_text(
                encoding="utf-8"
            )
        )
        self.assertEqual(
            connection.execute(
                "SELECT claim_kind, claim_text FROM episode_claims WHERE claim_id = 'claim'"
            ).fetchone(),
            ("fact", "existing claim"),
        )
        self.assertEqual(
            connection.execute(
                "SELECT event_id FROM episode_evidence WHERE claim_id = 'claim'"
            ).fetchone(),
            ("event",),
        )
        connection.execute(
            """INSERT INTO episode_claims VALUES
               ('preference', 'episode', 1, 'preference', 'likes concise replies',
                'assistant_inferred', 2)"""
        )
        self.assertEqual(connection.execute("PRAGMA foreign_key_check").fetchall(), [])
        connection.close()

    def test_raw_event_table_is_append_only(self):
        connection = sqlite3.connect(":memory:")
        connection.executescript(migration_bundle())
        connection.execute(
            "INSERT INTO scopes VALUES (?, ?, ?, ?, ?, ?, ?)",
            ("scope", "test", "bot", "private", "user", "default", 1),
        )
        connection.execute(
            """INSERT INTO raw_events VALUES
               (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)""",
            (
                "event", "scope", "test:1", "user", "Alice", 0, 1, 1,
                "", "hash", None, "[]", "{}", 1,
                "user_message", "platform", "present",
            ),
        )
        with self.assertRaises(sqlite3.IntegrityError):
            connection.execute(
                "UPDATE raw_events SET content = 'changed' WHERE event_id = 'event'"
            )
        with self.assertRaises(sqlite3.IntegrityError):
            connection.execute("DELETE FROM raw_events WHERE event_id = 'event'")
        connection.close()

    def test_schema4_rejects_incoherent_jobs_and_mutated_admissions(self):
        connection = sqlite3.connect(":memory:")
        connection.executescript(migration_bundle())
        connection.execute(
            "INSERT INTO scopes VALUES (?, ?, ?, ?, ?, ?, ?)",
            ("scope", "test", "bot", "private", "user", "default", 1),
        )

        job_columns = """
            job_id, scope_id, job_kind, idempotency_key, state, checkpoint_json,
            lease_owner, lease_token, lease_expires_at_ms, attempt, max_attempts,
            last_error_kind, last_error_text, proposal_digest, created_at_ms,
            available_at_ms, updated_at_ms, completed_at_ms
        """
        with self.assertRaises(sqlite3.IntegrityError):
            connection.execute(
                f"INSERT INTO episode_jobs({job_columns}) VALUES "
                "('invalid-lease', 'scope', 'episode_build', 'invalid', 'leased', '{}', "
                "NULL, NULL, NULL, 0, 3, NULL, NULL, NULL, 1, 1, 1, NULL)"
            )

        connection.execute(
            f"INSERT INTO episode_jobs({job_columns}) VALUES "
            "('pending', 'scope', 'episode_build', 'pending', 'pending', '{}', "
            "NULL, NULL, NULL, 0, 3, NULL, NULL, NULL, 1, 1, 1, NULL)"
        )
        with self.assertRaises(sqlite3.IntegrityError):
            connection.execute(
                "UPDATE episode_jobs SET completed_at_ms = 2 WHERE job_id = 'pending'"
            )

        connection.execute(
            """INSERT INTO raw_events VALUES
               (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)""",
            (
                "event", "scope", "test:1", "user", "", 0, 1, 1,
                "", "hash", None, "[]", "{}", 1,
                "user_message", "platform", "present",
            ),
        )
        connection.execute(
            "INSERT INTO job_event_admissions VALUES ('pending', 'scope', 'event', 1)"
        )
        with self.assertRaises(sqlite3.IntegrityError):
            connection.execute(
                "UPDATE job_event_admissions SET admitted_at_ms = 2 WHERE job_id = 'pending'"
            )
        with self.assertRaises(sqlite3.IntegrityError):
            connection.execute(
                "DELETE FROM job_event_admissions WHERE job_id = 'pending'"
            )

        connection.execute(
            f"INSERT INTO episode_jobs({job_columns}) VALUES "
            "('lease-a', 'scope', 'episode_build', 'lease-a', 'leased', '{}', "
            "'worker-a', 'token-a', 10, 0, 3, NULL, NULL, NULL, 1, 1, 1, NULL)"
        )
        with self.assertRaises(sqlite3.IntegrityError):
            connection.execute(
                f"INSERT INTO episode_jobs({job_columns}) VALUES "
                "('lease-b', 'scope', 'episode_build', 'lease-b', 'leased', '{}', "
                "'worker-b', 'token-b', 10, 0, 3, NULL, NULL, NULL, 1, 1, 1, NULL)"
            )
        connection.close()

    def test_v1_plaintext_is_moved_to_revocable_payload_without_changing_envelope(self):
        connection = sqlite3.connect(":memory:")
        connection.executescript(
            (ROOT / "migrations" / "0001_initial.sql").read_text(encoding="utf-8")
        )
        connection.execute(
            "INSERT INTO scopes VALUES (?, ?, ?, ?, ?, ?, ?)",
            ("scope", "test", "bot", "private", "user", "default", 1),
        )
        connection.execute(
            """INSERT INTO raw_events VALUES
               (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)""",
            (
                "event", "scope", "test:1", "user", "Alice", 0, 1, 1,
                "private migration marker", "content-hash", None,
                '["bob"]', '{"source":"test"}', 2,
            ),
        )
        before = connection.execute(
            "SELECT COUNT(*), MIN(content_hash), MAX(content_hash) FROM raw_events"
        ).fetchone()
        connection.commit()

        migration_2 = (
            ROOT / "migrations" / "0002_event_payloads_and_episodes.sql"
        ).read_text(encoding="utf-8")
        connection.executescript(migration_2)

        after = connection.execute(
            "SELECT COUNT(*), MIN(content_hash), MAX(content_hash) FROM raw_events"
        ).fetchone()
        self.assertEqual(after, before)
        envelope = connection.execute(
            """SELECT sender_display_name, content, mentions_json, metadata_json
               FROM raw_events WHERE event_id = 'event'"""
        ).fetchone()
        self.assertEqual(envelope, ("", "", "[]", "{}"))
        payload = connection.execute(
            """SELECT payload_state, sender_display_name, content, mentions_json,
                      metadata_json, purged_at_ms
               FROM raw_event_payloads WHERE event_id = 'event'"""
        ).fetchone()
        self.assertEqual(
            payload,
            (
                "active", "Alice", "private migration marker", '["bob"]',
                '{"source":"test"}', None,
            ),
        )

        connection.execute(
            """UPDATE raw_event_payloads
               SET payload_state = 'purged', sender_display_name = NULL,
                   content = NULL, mentions_json = NULL, metadata_json = NULL,
                   purged_at_ms = 3, updated_at_ms = 3
               WHERE event_id = 'event'"""
        )
        connection.commit()
        connection.executescript(migration_2)
        state = connection.execute(
            "SELECT payload_state, content FROM raw_event_payloads WHERE event_id = 'event'"
        ).fetchone()
        self.assertEqual(state, ("purged", None))
        connection.close()


if __name__ == "__main__":
    unittest.main()
