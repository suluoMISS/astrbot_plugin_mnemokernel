"""Install and exercise exactly one MnemoKernel native wheel."""

from __future__ import annotations

import argparse
import importlib
import json
import subprocess
import sys
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("wheel_directory", type=Path)
    parser.add_argument(
        "--unpacked-directory",
        type=Path,
        help="import an already unpacked wheel instead of installing it with pip",
    )
    args = parser.parse_args()

    wheels = sorted(args.wheel_directory.resolve().glob("*.whl"))
    if len(wheels) != 1:
        raise RuntimeError(
            f"expected exactly one wheel in {args.wheel_directory}, found {len(wheels)}"
        )

    if args.unpacked_directory is not None:
        sys.path.insert(0, str(args.unpacked_directory.resolve()))
    else:
        subprocess.run(
            [
                sys.executable,
                "-m",
                "pip",
                "install",
                "--force-reinstall",
                "--no-deps",
                str(wheels[0]),
            ],
            check=True,
        )
    importlib.invalidate_caches()
    native = importlib.import_module("_mnemokernel")

    with tempfile.TemporaryDirectory(prefix="mnemokernel-native-smoke-") as temporary:
        database = Path(temporary) / "smoke.sqlite3"
        kernel = native.Kernel(str(database))
        health = json.loads(kernel.health_json())
        assert health["status"] == "ok"
        assert health["protocol"] == "mnemokernel.v1"
        assert health["schema_version"] == 4
        assert health["capabilities"] == {
            "capture_ready": True,
            "episode_ready": True,
            "recall_ready": True,
        }

        scope = {
            "platform_id": "native-smoke",
            "bot_account_id": "bot",
            "kind": "private",
            "session_id": "user",
            "persona_id": "default",
        }
        secret = "mnemokernel-native-private-marker"
        event = {
            "protocol": "mnemokernel.v1",
            "origin_kind": "user_message",
            "occurred_time_source": "platform",
            "platform_message_id_state": "present",
            "source_key": "native-smoke:1",
            "scope": scope,
            "sender_account_id": "user",
            "sender_display_name": "Smoke User",
            "sender_is_bot": False,
            "observed_at_ms": 1,
            "occurred_at_ms": 1,
            "content": secret,
            "reply_to_source_key": None,
            "mentions": [],
            "metadata": {},
        }
        inserted = json.loads(kernel.ingest_event_json(json.dumps(event)))
        assert inserted["status"] == "ok"
        assert inserted["inserted"] is True

        journal_request = {
            "protocol": "mnemokernel.v1",
            "scope": scope,
            "journal_date": "1970-01-01",
            "occurred_from_ms": 0,
            "occurred_to_ms": 86_400_000,
        }
        context = json.loads(kernel.journal_context_json(json.dumps(journal_request)))
        assert context["status"] == "ok"
        assert context["context_truncated"] is False
        assert len(context["events"]) == 1
        journal_proposal = {
            "protocol": "mnemokernel.v1",
            "proposal_id": "native-smoke-journal",
            "scope_id": context["scope_id"],
            "journal_date": "1970-01-01",
            "occurred_from_ms": 0,
            "occurred_to_ms": 86_400_000,
            "title": "Native smoke 日记",
            "summary": "验证 native wheel 可以保存和读取一份带证据的日记。",
            "cues": ["native wheel"],
            "open_loops": [],
            "claims": [
                {
                    "claim_kind": "fact",
                    "claim_text": "native wheel 日记链路可用。",
                    "epistemic_status": "assistant_inferred",
                    "evidence_event_ids": [inserted["event_id"]],
                },
                {
                    "claim_kind": "preference",
                    "claim_text": "用户偏好简洁的 native wheel 验证结果。",
                    "epistemic_status": "assistant_inferred",
                    "evidence_event_ids": [inserted["event_id"]],
                }
            ],
        }
        saved = json.loads(
            kernel.save_journal_json(
                json.dumps(journal_request), json.dumps(journal_proposal)
            )
        )
        assert saved["status"] == "stored"
        read = json.loads(kernel.read_journal_json(json.dumps(journal_request)))
        assert read["status"] == "found"
        assert read["claims"][0]["claim_text"] == "native wheel 日记链路可用。"
        recall_request = {
            "protocol": "mnemokernel.v1",
            "scope": scope,
            "need_type": "fact",
            "reason": "需要确认 native wheel 日记链路是否已经保存过。",
            "cues": ["native wheel"],
            "depth": "glance",
            "time_hint": "1970-01-01",
        }
        recalled = json.loads(kernel.recall_request_json(json.dumps(recall_request)))
        assert recalled["status"] == "ok"
        assert recalled["items"]
        assert len(recalled["items"]) <= 3
        assert len(recalled["brief"]) <= 800
        preference_request = {
            "protocol": "mnemokernel.v1",
            "scope": scope,
            "need_type": "preference",
            "reason": "需要按用户偏好决定回答长度。",
            "cues": ["简洁", "验证结果"],
            "depth": "glance",
            "time_hint": None,
        }
        preference = json.loads(
            kernel.recall_request_json(json.dumps(preference_request))
        )
        assert preference["status"] == "ok"
        assert len(preference["items"]) == 1
        assert "简洁" in preference["items"][0]["statement"]
        assert len(preference["brief"]) <= 420

        paused = json.loads(
            kernel.set_capture_policy_json(
                json.dumps(
                    {
                        "protocol": "mnemokernel.v1",
                        "scope": scope,
                        "action": "pause",
                        "actor_id": "native-smoke-user",
                        "reason": "smoke pause",
                    }
                )
            )
        )
        assert paused["policy_state"] == "opted_out"
        resumed = json.loads(
            kernel.set_capture_policy_json(
                json.dumps(
                    {
                        "protocol": "mnemokernel.v1",
                        "scope": scope,
                        "action": "resume",
                        "actor_id": "native-smoke-user",
                        "reason": "smoke resume",
                    }
                )
            )
        )
        assert resumed["policy_state"] == "active"
        retained = json.loads(
            kernel.retain_payloads_json(
                json.dumps(
                    {
                        "protocol": "mnemokernel.v1",
                        "scope": scope,
                        "cutoff_at_ms": 0,
                        "actor_id": "native-smoke-user",
                        "reason": "smoke retention",
                    }
                )
            )
        )
        assert retained["purged_payloads"] == 0
        stats = json.loads(
            kernel.scope_stats_json(json.dumps({"protocol": "mnemokernel.v1", "scope": scope}))
        )
        assert stats["active_memories"] == 2

        scopes = json.loads(kernel.inspect_json(json.dumps({"collection": "scopes", "limit": 20})))
        assert len(scopes["items"]) == 1
        scope_id = scopes["items"][0]["scope_id"]
        inspection = {"collection": "memories", "scope_id": scope_id, "limit": 20}
        assert json.loads(kernel.inspect_json(json.dumps(inspection)))["total"] == 2

        purge_request = {
            "protocol": "mnemokernel.v1",
            "scope": scope,
            "actor_id": "native-smoke-user",
            "reason": "native wheel privacy smoke test",
        }
        purged = json.loads(kernel.purge_scope_json(json.dumps(purge_request)))
        assert purged["status"] == "ok"
        assert purged["purged_payloads"] == 1
        assert purged["compacted"] is True
        assert json.loads(kernel.inspect_json(json.dumps(inspection)))["total"] == 0
        inspection["collection"] = "events"
        cleared_events = json.loads(kernel.inspect_json(json.dumps(inspection)))
        assert all(item["content"] is None for item in cleared_events["items"])
        blocked_pause = json.loads(
            kernel.set_capture_policy_json(
                json.dumps(
                    {
                        "protocol": "mnemokernel.v1",
                        "scope": scope,
                        "action": "pause",
                        "actor_id": "native-smoke-user",
                        "reason": "pause after forget must be blocked",
                    }
                )
            )
        )
        assert blocked_pause["status"] == "blocked"
        blocked_resume = json.loads(
            kernel.set_capture_policy_json(
                json.dumps(
                    {
                        "protocol": "mnemokernel.v1",
                        "scope": scope,
                        "action": "resume",
                        "actor_id": "native-smoke-user",
                        "reason": "resume after forget must be blocked",
                    }
                )
            )
        )
        assert blocked_resume["status"] == "blocked"
        assert blocked_resume["policy_state"] == "opted_out"
        replayed = json.loads(kernel.ingest_event_json(json.dumps(event)))
        assert replayed["inserted"] is False

        kernel.close()
        try:
            kernel.health_json()
        except RuntimeError:
            pass
        else:
            raise AssertionError("closed kernel unexpectedly accepted a health request")
        assert secret.encode() not in database.read_bytes()

    print(json.dumps(health, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
