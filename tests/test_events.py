import unittest
from enum import Enum, auto

from mnemokernel_adapter.events import (
    EventNormalizationError,
    actor_id_from_event,
    normalize_event,
    normalize_sent_event,
)
from mnemokernel_adapter.models import (
    OccurredTimeSource,
    OriginKind,
    PlatformMessageIdState,
    ScopeKind,
)


class Plain:
    def __init__(self, text):
        self.text = text


class At:
    def __init__(self, qq):
        self.qq = qq


class Reply:
    def __init__(self, message_id):
        self.id = message_id


class ResultContentType(Enum):
    LLM_RESULT = auto()
    GENERAL_RESULT = auto()
    AGENT_RUNNER_ERROR = auto()


class FakeResult:
    def __init__(self, *components, kind=ResultContentType.LLM_RESULT):
        self.chain = list(components)
        self.result_content_type = kind

    def get_plain_text(self):
        return " ".join(
            component.text
            for component in self.chain
            if isinstance(component, Plain)
        )


class FakeMessage:
    def __init__(self):
        self.self_id = "bot-1"
        self.message_id = "msg-1"
        self.timestamp = 1234
        self.reply_to_message_id = None
        self.message = [Plain("hello"), At("bot-1"), Reply("previous")]


class FakeEvent:
    def __init__(self):
        self.message_obj = FakeMessage()
        self.message_str = "hello"
        self.unified_msg_origin = "test:FriendMessage:user-1"
        self._extras = {}
        self._result = FakeResult(Plain("final"), At("user-1"))

    def get_platform_id(self):
        return "platform-instance-1"

    def get_platform_name(self):
        return "test"

    def get_sender_id(self):
        return "user-1"

    def get_sender_name(self):
        return "Alice"

    def get_self_id(self):
        return "bot-1"

    def get_group_id(self):
        return ""

    def get_extra(self, key=None, default=None):
        if key is None:
            return self._extras
        return self._extras.get(key, default)

    def set_extra(self, key, value):
        self._extras[key] = value

    def get_result(self):
        return self._result


class EventTests(unittest.TestCase):
    def test_actor_identity_is_sender_not_group_scope(self):
        framework_event = FakeEvent()
        framework_event.get_group_id = lambda: "group-1"
        self.assertEqual(actor_id_from_event(framework_event), "user-1")

    def test_normalizes_private_message_with_explicit_contract_fields(self):
        event = normalize_event(FakeEvent(), now_ms=9_999)
        self.assertEqual(event.source_key, "platform-instance-1:msg-1")
        self.assertEqual(event.scope.kind, ScopeKind.PRIVATE)
        self.assertEqual(event.scope.session_id, "user-1")
        self.assertEqual(event.occurred_at_ms, 1_234_000)
        self.assertEqual(event.occurred_time_source, OccurredTimeSource.PLATFORM)
        self.assertEqual(event.platform_message_id_state, PlatformMessageIdState.PRESENT)
        self.assertEqual(event.origin_kind, OriginKind.USER_MESSAGE)
        self.assertEqual(event.reply_to_source_key, "platform-instance-1:previous")
        self.assertEqual(event.mentions, ("bot-1",))
        self.assertFalse(event.sender_is_bot)

    def test_sent_result_is_bot_origin_and_duplicate_hook_is_deterministic(self):
        framework_event = FakeEvent()
        normalize_event(framework_event, now_ms=1)
        first = normalize_sent_event(framework_event, now_ms=2)
        second = normalize_sent_event(framework_event, now_ms=999_999)
        self.assertEqual(first.source_key, second.source_key)
        self.assertEqual(first.origin_kind, OriginKind.BOT_RESPONSE)
        self.assertTrue(first.sender_is_bot)
        self.assertEqual(first.content, "final")
        self.assertEqual(first.reply_to_source_key, "platform-instance-1:msg-1")
        self.assertEqual(first.mentions, ("user-1",))
        self.assertEqual(
            first.occurred_time_source, OccurredTimeSource.OBSERVED_FALLBACK
        )
        self.assertEqual(first.platform_message_id_state, PlatformMessageIdState.MISSING)

    def test_framework_recognized_command_and_its_reply_are_suppressed(self):
        framework_event = FakeEvent()
        framework_event._extras["handlers_parsed_params"] = {
            "plugin.command": {"subcommand": "status"}
        }
        with self.assertRaises(EventNormalizationError):
            normalize_event(framework_event, now_ms=1)
        with self.assertRaises(EventNormalizationError):
            normalize_sent_event(framework_event, now_ms=2)

    def test_slash_prefixed_plain_text_is_not_guessed_to_be_a_command(self):
        framework_event = FakeEvent()
        framework_event.message_str = "/usr/bin/python is a path"
        normalized = normalize_event(framework_event, now_ms=1)
        self.assertEqual(normalized.content, "/usr/bin/python is a path")

    def test_agent_error_and_empty_sent_results_are_rejected(self):
        framework_event = FakeEvent()
        framework_event._result = FakeResult(
            Plain("internal failure"), kind=ResultContentType.AGENT_RUNNER_ERROR
        )
        with self.assertRaises(EventNormalizationError):
            normalize_sent_event(framework_event, now_ms=1)
        framework_event._result = FakeResult(At("user-1"))
        with self.assertRaises(EventNormalizationError):
            normalize_sent_event(framework_event, now_ms=1)

    def test_rejects_oversized_content_without_truncating(self):
        framework_event = FakeEvent()
        framework_event.message_str = "abcdef"
        with self.assertRaises(EventNormalizationError):
            normalize_event(framework_event, max_content_chars=5, now_ms=1)


if __name__ == "__main__":
    unittest.main()
