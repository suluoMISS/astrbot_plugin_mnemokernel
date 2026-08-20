import unittest

from mnemokernel_adapter.models import (
    CapturePolicyAction,
    CapturePolicyRequest,
    DailyJournalProposal,
    DailyJournalRequest,
    JournalClaimKind,
    JournalEpistemicStatus,
    NeedType,
    PurgeScopeRequest,
    RecallDepth,
    RecallRequest,
    RetentionRequest,
    ScopeDescriptor,
    ScopeStatsRequest,
    ScopeKind,
)


def scope():
    return ScopeDescriptor(
        platform_id="test",
        bot_account_id="bot",
        kind=ScopeKind.PRIVATE,
        session_id="user",
    )


class ModelTests(unittest.TestCase):
    def test_daily_journal_proposal_is_bounded_and_typed(self):
        request = DailyJournalRequest(
            scope=scope(),
            journal_date="1970-01-01",
            occurred_from_ms=0,
            occurred_to_ms=86_400_000,
        )
        proposal = DailyJournalProposal.from_model(
            {
                "title": "项目日记",
                "summary": "完成了结构整理。",
                "cues": ["项目"],
                "open_loops": ["继续验证"],
                "claims": [
                    {
                        "claim_kind": "decision",
                        "claim_text": "下一步先做手动验证。",
                        "evidence_event_ids": ["a" * 64],
                    }
                ],
            },
            request=request,
            scope_id="b" * 64,
            proposal_id="journal-test",
        )
        self.assertEqual(proposal.claims[0].claim_kind, JournalClaimKind.DECISION)
        self.assertEqual(
            proposal.claims[0].epistemic_status,
            JournalEpistemicStatus.ASSISTANT_INFERRED,
        )
        self.assertEqual(proposal.to_dict()["journal_date"], "1970-01-01")

    def test_daily_journal_accepts_preference_claims(self):
        request = DailyJournalRequest(
            scope=scope(),
            journal_date="1970-01-01",
            occurred_from_ms=0,
            occurred_to_ms=86_400_000,
        )
        proposal = DailyJournalProposal.from_model(
            {
                "title": "偏好",
                "summary": "记录用户明确表达的偏好。",
                "claims": [
                    {
                        "claim_kind": "preference",
                        "claim_text": "用户偏好简洁回答。",
                        "evidence_event_ids": ["a" * 64],
                    }
                ],
            },
            request=request,
            scope_id="b" * 64,
            proposal_id="journal-preference",
        )
        self.assertEqual(proposal.claims[0].claim_kind, JournalClaimKind.PREFERENCE)

    def test_daily_journal_rejects_invalid_model_output(self):
        request = DailyJournalRequest(
            scope=scope(),
            journal_date="1970-01-01",
            occurred_from_ms=0,
            occurred_to_ms=86_400_000,
        )
        with self.assertRaises(ValueError):
            DailyJournalProposal.from_model(
                {"title": "only title", "summary": ""},
                request=request,
                scope_id="b" * 64,
                proposal_id="journal-test",
            )

    def test_recall_request_is_typed(self):
        request = RecallRequest.build(
            scope=scope(),
            need_type="preference",
            reason="The answer depends on a previously stated choice.",
            cues=["tea", "breakfast"],
            depth="focused",
        )
        self.assertEqual(request.need_type, NeedType.PREFERENCE)
        self.assertEqual(request.depth, RecallDepth.FOCUSED)
        self.assertEqual(request.to_dict()["protocol"], "mnemokernel.v1")

    def test_recall_requires_cues_and_reason(self):
        with self.assertRaises(ValueError):
            RecallRequest.build(
                scope=scope(), need_type="fact", reason="", cues=["project"]
            )
        with self.assertRaises(ValueError):
            RecallRequest.build(
                scope=scope(), need_type="fact", reason="needed", cues=[]
            )
        with self.assertRaises(ValueError):
            RecallRequest.build(
                scope=scope(), need_type="fact", reason="needed", cues="project"
            )
        with self.assertRaises(ValueError):
            RecallRequest.build(
                scope=scope(), need_type="fact", reason="needed", cues=["project", "project"]
            )

    def test_scope_requires_stable_identifiers(self):
        with self.assertRaises(ValueError):
            ScopeDescriptor(
                platform_id="test",
                bot_account_id="bot",
                kind=ScopeKind.PRIVATE,
                session_id="",
            )

    def test_purge_request_is_typed_and_requires_reason(self):
        request = PurgeScopeRequest(
            scope=scope(), actor_id="user", reason="user requested deletion"
        )
        self.assertEqual(request.to_dict()["scope"]["session_id"], "user")
        with self.assertRaises(ValueError):
            PurgeScopeRequest(scope=scope(), actor_id="user", reason="")

    def test_privacy_control_requests_are_typed_and_bounded(self):
        paused = CapturePolicyRequest(
            scope=scope(), action=CapturePolicyAction.PAUSE, actor_id="user", reason="pause"
        )
        self.assertEqual(paused.to_dict()["action"], "pause")
        self.assertEqual(ScopeStatsRequest(scope=scope()).to_dict()["protocol"], "mnemokernel.v1")
        with self.assertRaises(ValueError):
            RetentionRequest(scope=scope(), cutoff_at_ms=-1, actor_id="user", reason="retention")


if __name__ == "__main__":
    unittest.main()

