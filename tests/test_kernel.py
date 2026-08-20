import json
import unittest
from pathlib import Path

from mnemokernel_adapter.kernel import KernelClient, KernelUnavailableError
from mnemokernel_adapter.models import (
    CapturePolicyAction,
    CapturePolicyRequest,
    DailyJournalProposal,
    DailyJournalRequest,
    OccurredTimeSource,
    OriginKind,
    PlatformMessageIdState,
    PurgeScopeRequest,
    RawEventInput,
    RecallRequest,
    RetentionRequest,
    ScopeDescriptor,
    ScopeStatsRequest,
    ScopeKind,
)


class HealthyNativeKernel:
    def __init__(self, database_path):
        self.database_path = database_path

    def health_json(self):
        return json.dumps(
            {
                "status": "ok",
                "protocol": "mnemokernel.v1",
                "schema_version": 4,
                "capabilities": {
                    "capture_ready": True,
                    "episode_ready": True,
                    "recall_ready": True,
                },
            }
        )

    def close(self):
        self.closed = True

    def ingest_event_json(self, payload):
        parsed = json.loads(payload)
        return json.dumps(
            {"status": "ok", "event_id": "e" * 64, "inserted": True, "echo": parsed["source_key"]}
        )

    def purge_scope_json(self, payload):
        parsed = json.loads(payload)
        return json.dumps(
            {
                "status": "ok",
                "purged_payloads": 1,
                "actor_id": parsed["actor_id"],
            }
        )

    def journal_context_json(self, payload):
        parsed = json.loads(payload)
        return json.dumps(
            {
                "status": "ok",
                "scope_id": "s" * 64,
                "journal_date": parsed["journal_date"],
                "context_truncated": False,
                "events": [],
            }
        )

    def save_journal_json(self, request_payload, proposal_payload):
        request = json.loads(request_payload)
        proposal = json.loads(proposal_payload)
        return json.dumps(
            {
                "status": "stored",
                "scope_id": proposal["scope_id"],
                "journal_date": request["journal_date"],
                "episode_id": "e" * 64,
                "version": 1,
                "event_count": 1,
                "claim_count": len(proposal["claims"]),
            }
        )

    def read_journal_json(self, payload):
        parsed = json.loads(payload)
        return json.dumps(
            {
                "status": "not_found",
                "scope_id": "s" * 64,
                "journal_date": parsed["journal_date"],
                "episode_id": "e" * 64,
                "version": 0,
                "title": "",
                "summary": "",
                "cues": [],
                "open_loops": [],
                "claims": [],
            }
        )

    def recall_request_json(self, payload):
        parsed = json.loads(payload)
        return json.dumps(
            {
                "status": "ok",
                "recall_id": "r" * 64,
                "scope_id": "s" * 64,
                "reason": "returned evidence-backed memory",
                "brief": "根据已保存的日记证据：\n1. 测试记忆。",
                "items": [
                    {
                        "memory_id": "m" * 64,
                        "statement": "测试记忆。",
                        "confidence": 0.7,
                        "evidence_event_ids": ["e" * 64],
                    }
                ],
                "echo_need_type": parsed["need_type"],
            }
        )

    def set_capture_policy_json(self, payload):
        parsed = json.loads(payload)
        return json.dumps(
            {"status": "ok", "scope_id": "s" * 64, "policy_state": "opted_out", "echo": parsed["action"]}
        )

    def retain_payloads_json(self, payload):
        parsed = json.loads(payload)
        return json.dumps(
            {
                "status": "ok",
                "scope_id": "s" * 64,
                "run_id": "r" * 64,
                "cutoff_at_ms": parsed["cutoff_at_ms"],
                "purged_payloads": 1,
                "residual_scan": "passed",
            }
        )

    def scope_stats_json(self, payload):
        return json.dumps(
            {
                "status": "ok",
                "scope_id": "s" * 64,
                "policy_state": "active",
                "raw_events": 2,
                "active_payloads": 1,
                "episodes": 1,
                "active_memories": 1,
                "superseded_memories": 0,
                "archived_memories": 0,
                "last_retention_cutoff_ms": None,
            }
        )


class HealthyNativeModule:
    Kernel = HealthyNativeKernel


class BrokenNativeModule:
    class Kernel:
        def __init__(self, database_path):
            raise RuntimeError("ABI mismatch")


def raw_event():
    return RawEventInput(
        origin_kind=OriginKind.USER_MESSAGE,
        occurred_time_source=OccurredTimeSource.PLATFORM,
        platform_message_id_state=PlatformMessageIdState.PRESENT,
        source_key="test:1",
        scope=ScopeDescriptor(
            platform_id="test",
            bot_account_id="bot",
            kind=ScopeKind.PRIVATE,
            session_id="user",
        ),
        sender_account_id="user",
        sender_display_name="Alice",
        sender_is_bot=False,
        observed_at_ms=1,
        occurred_at_ms=1,
        content="hello",
    )


class KernelTests(unittest.TestCase):
    def test_healthy_native_bridge(self):
        client = KernelClient.open(Path("db.sqlite3"), native_module=HealthyNativeModule)
        self.assertTrue(client.available)
        self.assertTrue(client.status.capabilities.capture_ready)
        self.assertTrue(client.status.capabilities.episode_ready)
        self.assertTrue(client.status.capabilities.recall_ready)
        self.assertEqual(client.ingest_event(raw_event())["echo"], "test:1")
        request = DailyJournalRequest(
            scope=raw_event().scope,
            journal_date="1970-01-01",
            occurred_from_ms=0,
            occurred_to_ms=86_400_000,
        )
        context = client.journal_context(request)
        self.assertEqual(context["status"], "ok")
        proposal = DailyJournalProposal.from_model(
            {
                "title": "日记",
                "summary": "今天完成了测试。",
                "claims": [],
                "cues": [],
                "open_loops": [],
            },
            request=request,
            scope_id="s" * 64,
            proposal_id="journal-test",
        )
        self.assertEqual(client.save_journal(request, proposal)["status"], "stored")
        self.assertEqual(client.read_journal(request)["status"], "not_found")
        self.assertEqual(
            client.request_recall(
                RecallRequest.build(
                    scope=raw_event().scope,
                    need_type="fact",
                    reason="测试桥接",
                    cues=["测试"],
                )
            )["echo_need_type"],
            "fact",
        )
        scope = raw_event().scope
        self.assertEqual(
            client.set_capture_policy(
                CapturePolicyRequest(
                    scope=scope,
                    action=CapturePolicyAction.PAUSE,
                    actor_id="user",
                    reason="pause",
                )
            )["echo"],
            "pause",
        )
        self.assertEqual(
            client.retain_payloads(
                RetentionRequest(scope=scope, cutoff_at_ms=2, actor_id="user", reason="retention")
            )["residual_scan"],
            "passed",
        )
        self.assertEqual(
            client.scope_stats(ScopeStatsRequest(scope=scope))["active_memories"], 1
        )
        purge = client.purge_scope(
            PurgeScopeRequest(
                scope=raw_event().scope,
                actor_id="user",
                reason="user requested deletion",
            )
        )
        self.assertEqual(purge["purged_payloads"], 1)
        client.close()
        self.assertFalse(client.available)
        with self.assertRaises(KernelUnavailableError):
            client.ingest_event(raw_event())

    def test_broken_native_bridge_fails_closed(self):
        client = KernelClient.open(Path("db.sqlite3"), native_module=BrokenNativeModule)
        self.assertFalse(client.available)
        self.assertIn("ABI mismatch", client.status.detail)
        with self.assertRaises(KernelUnavailableError):
            client.ingest_event(raw_event())


if __name__ == "__main__":
    unittest.main()
