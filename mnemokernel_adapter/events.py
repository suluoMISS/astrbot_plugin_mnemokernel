"""Typed AstrBot inbound and sent-result normalization.

Only small, documented framework fields cross this boundary. Platform raw
objects are never serialized wholesale.
"""

from __future__ import annotations

import hashlib
import time
from collections.abc import Mapping, Sequence
from typing import Any

from .models import (
    OccurredTimeSource,
    OriginKind,
    PlatformMessageIdState,
    RawEventInput,
    ScopeDescriptor,
    ScopeKind,
)


CAPTURE_SUPPRESSION_EXTRA = "mnemokernel_suppress_capture"
INBOUND_SOURCE_EXTRA = "mnemokernel_inbound_source_key"


class EventNormalizationError(ValueError):
    """The framework event cannot be represented by the v1 protocol."""


def _value(obj: Any, name: str, default: Any = None) -> Any:
    if obj is None:
        return default
    candidate = getattr(obj, name, default)
    if callable(candidate):
        try:
            return candidate()
        except (AttributeError, TypeError, ValueError):
            return default
    return candidate


def _text(value: Any, default: str = "") -> str:
    return default if value is None else str(value).strip()


def _raw_text(value: Any) -> str:
    return "" if value is None else str(value)


def _timestamp_ms(
    value: Any, fallback: int
) -> tuple[int, OccurredTimeSource]:
    try:
        number = float(value)
    except (TypeError, ValueError):
        return fallback, OccurredTimeSource.OBSERVED_FALLBACK
    if number < 0:
        return fallback, OccurredTimeSource.OBSERVED_FALLBACK
    timestamp = int(number if number >= 10_000_000_000 else number * 1000)
    return timestamp, OccurredTimeSource.PLATFORM


def _extra(event: Any, key: str, default: Any = None) -> Any:
    getter = getattr(event, "get_extra", None)
    if callable(getter):
        try:
            return getter(key, default)
        except TypeError:
            try:
                value = getter(key)
                return default if value is None else value
            except (AttributeError, TypeError, ValueError):
                pass
    extras = getattr(event, "_extras", None)
    if isinstance(extras, Mapping):
        return extras.get(key, default)
    return default


def event_is_recognized_command(event: Any) -> bool:
    """Use AstrBot's parsed-handler contract instead of slash-prefix guesses."""

    if _extra(event, CAPTURE_SUPPRESSION_EXTRA, False) is True:
        return True
    parsed = _extra(event, "handlers_parsed_params", {})
    return isinstance(parsed, Mapping) and bool(parsed)


def suppress_event_capture(event: Any) -> None:
    setter = getattr(event, "set_extra", None)
    if callable(setter):
        setter(CAPTURE_SUPPRESSION_EXTRA, True)
        return
    extras = getattr(event, "_extras", None)
    if isinstance(extras, dict):
        extras[CAPTURE_SUPPRESSION_EXTRA] = True


def _set_extra(event: Any, key: str, value: Any) -> None:
    setter = getattr(event, "set_extra", None)
    if callable(setter):
        setter(key, value)
        return
    extras = getattr(event, "_extras", None)
    if isinstance(extras, dict):
        extras[key] = value


def _components(value: Any) -> tuple[Any, ...]:
    chain = getattr(value, "chain", None)
    if isinstance(chain, Sequence) and not isinstance(chain, (str, bytes)):
        return tuple(chain)
    message = getattr(value, "message", None)
    if isinstance(message, Sequence) and not isinstance(message, (str, bytes)):
        return tuple(message)
    return ()


def _component_types(components: Sequence[Any]) -> tuple[str, ...]:
    result: list[str] = []
    for component in components:
        name = type(component).__name__.strip().lower()
        if name and name not in result:
            result.append(name[:64])
        if len(result) == 32:
            break
    return tuple(result)


def _mentions(components: Sequence[Any]) -> tuple[str, ...]:
    result: list[str] = []
    for component in components:
        if type(component).__name__.lower() != "at":
            continue
        mention = (
            _text(getattr(component, "qq", ""))
            or _text(getattr(component, "user_id", ""))
            or _text(getattr(component, "id", ""))
        )
        if mention and mention not in result:
            result.append(mention[:512])
        if len(result) == 64:
            break
    return tuple(result)


def _reply_id(message_obj: Any, components: Sequence[Any]) -> str | None:
    direct = _text(_value(message_obj, "reply_to_message_id"))
    if direct:
        return direct
    for component in components:
        if type(component).__name__.lower() != "reply":
            continue
        reply = (
            _text(getattr(component, "id", ""))
            or _text(getattr(component, "message_id", ""))
        )
        if reply:
            return reply
    return None


def scope_from_event(event: Any, persona_id: str = "default") -> ScopeDescriptor:
    message_obj = getattr(event, "message_obj", None)
    platform = (
        _text(_value(event, "get_platform_id"))
        or _text(_value(event, "get_platform_name"))
        or "unknown"
    )
    sender_id = _text(_value(event, "get_sender_id"))
    group_id = _text(_value(event, "get_group_id"))
    unified_origin = _text(getattr(event, "unified_msg_origin", ""))
    self_id = (
        _text(_value(event, "get_self_id"))
        or _text(_value(message_obj, "self_id"))
        or "default"
    )
    if group_id:
        kind = ScopeKind.GROUP
        session_id = group_id
    else:
        kind = ScopeKind.PRIVATE
        session_id = sender_id or unified_origin
    if not session_id:
        raise EventNormalizationError("event has no stable session identifier")
    return ScopeDescriptor(
        platform_id=platform,
        bot_account_id=self_id,
        kind=kind,
        session_id=session_id,
        persona_id=persona_id or "default",
    )


def actor_id_from_event(event: Any) -> str:
    """Return the real sender account, never a group/session identifier."""

    message_obj = getattr(event, "message_obj", None)
    actor_id, _ = _sender(event, message_obj)
    return actor_id


def _sender(event: Any, message_obj: Any) -> tuple[str, str]:
    sender_id = _text(_value(event, "get_sender_id"))
    sender = getattr(message_obj, "sender", None)
    if not sender_id:
        sender_id = _text(_value(sender, "user_id")) or _text(_value(sender, "id"))
    if not sender_id:
        raise EventNormalizationError("event has no stable sender identifier")
    sender_name = _text(_value(event, "get_sender_name"))
    if not sender_name:
        sender_name = (
            _text(_value(sender, "nickname"))
            or _text(_value(sender, "name"))
            or sender_id
        )
    return sender_id, sender_name


def _inbound_source_key(
    event: Any,
    scope: ScopeDescriptor,
    sender_id: str,
    occurred_at_ms: int,
    content: str,
) -> tuple[str, PlatformMessageIdState]:
    message_obj = getattr(event, "message_obj", None)
    message_id = _text(_value(message_obj, "message_id")) or _text(
        _value(event, "message_id")
    )
    if message_id:
        return (
            f"{scope.platform_id}:{message_id}",
            PlatformMessageIdState.PRESENT,
        )
    material = "\x1f".join(
        (
            scope.bot_account_id,
            scope.kind.value,
            scope.session_id,
            sender_id,
            str(occurred_at_ms),
            content,
        )
    ).encode("utf-8")
    return (
        f"{scope.platform_id}:synthetic:{hashlib.sha256(material).hexdigest()}",
        PlatformMessageIdState.MISSING,
    )


def _metadata(event: Any, components: Sequence[Any], **extra: str) -> dict[str, Any]:
    metadata: dict[str, Any] = {
        "unified_origin": _text(getattr(event, "unified_msg_origin", ""))[:512],
        "component_types": list(_component_types(components)),
    }
    metadata.update(extra)
    return metadata


def normalize_event(
    event: Any,
    *,
    max_content_chars: int = 12_000,
    persona_id: str = "default",
    now_ms: int | None = None,
) -> RawEventInput:
    """Normalize a non-command inbound user message."""

    if event_is_recognized_command(event):
        raise EventNormalizationError("recognized command events are not evidence")
    observed_at_ms = int(time.time() * 1000) if now_ms is None else int(now_ms)
    message_obj = getattr(event, "message_obj", None)
    scope = scope_from_event(event, persona_id=persona_id)
    content = _raw_text(getattr(event, "message_str", ""))
    if not content:
        content = _raw_text(_value(event, "get_message_str"))
    if not content:
        content = _raw_text(_value(message_obj, "message_str"))
    if not content.strip():
        raise EventNormalizationError("message has no plain text content")
    if len(content) > max_content_chars:
        raise EventNormalizationError(
            f"message has {len(content)} characters; configured maximum is {max_content_chars}"
        )

    sender_id, sender_name = _sender(event, message_obj)
    occurred_at_ms, time_source = _timestamp_ms(
        _value(message_obj, "timestamp", _value(event, "timestamp")), observed_at_ms
    )
    source_key, message_id_state = _inbound_source_key(
        event, scope, sender_id, occurred_at_ms, content
    )
    _set_extra(event, INBOUND_SOURCE_EXTRA, source_key)
    components = _components(message_obj)
    reply_id = _reply_id(message_obj, components)
    mentions = _mentions(components)
    return RawEventInput(
        origin_kind=OriginKind.USER_MESSAGE,
        occurred_time_source=time_source,
        platform_message_id_state=message_id_state,
        source_key=source_key,
        scope=scope,
        sender_account_id=sender_id,
        sender_display_name=sender_name,
        sender_is_bot=False,
        observed_at_ms=observed_at_ms,
        occurred_at_ms=occurred_at_ms,
        content=content,
        reply_to_source_key=(
            f"{scope.platform_id}:{reply_id}" if reply_id else None
        ),
        mentions=mentions,
        metadata=_metadata(
            event,
            components,
            reply_state="present" if reply_id else "missing",
            mention_state="present" if mentions else "missing",
        ),
    )


def _result_plain_text(result: Any) -> str:
    getter = getattr(result, "get_plain_text", None)
    if callable(getter):
        try:
            return _raw_text(getter())
        except (AttributeError, TypeError, ValueError):
            pass
    texts = [
        _raw_text(getattr(component, "text", ""))
        for component in _components(result)
        if type(component).__name__.lower() == "plain"
    ]
    return " ".join(texts)


def _result_content_type(result: Any) -> str:
    content_type = getattr(result, "result_content_type", None)
    name = _text(getattr(content_type, "name", content_type)).lower()
    return name or "unknown"


def normalize_sent_event(
    event: Any,
    *,
    max_content_chars: int = 12_000,
    persona_id: str = "default",
    now_ms: int | None = None,
) -> RawEventInput:
    """Normalize AstrBot's final non-command result after the send hook."""

    if event_is_recognized_command(event):
        raise EventNormalizationError("command or diagnostic replies are not evidence")
    getter = getattr(event, "get_result", None)
    result = getter() if callable(getter) else None
    if result is None:
        raise EventNormalizationError("sent event has no final result")
    result_type = _result_content_type(result)
    if result_type == "agent_runner_error":
        raise EventNormalizationError("agent error replies are not evidence")
    content = _result_plain_text(result)
    if not content.strip():
        raise EventNormalizationError("sent result has no plain text content")
    if len(content) > max_content_chars:
        raise EventNormalizationError(
            f"sent result has {len(content)} characters; configured maximum is {max_content_chars}"
        )

    observed_at_ms = int(time.time() * 1000) if now_ms is None else int(now_ms)
    scope = scope_from_event(event, persona_id=persona_id)
    message_obj = getattr(event, "message_obj", None)
    inbound_sender, _ = _sender(event, message_obj)
    parent_key = _text(_extra(event, INBOUND_SOURCE_EXTRA, ""))
    if not parent_key:
        parent_occurred_at_ms, _ = _timestamp_ms(
            _value(message_obj, "timestamp", _value(event, "timestamp")),
            observed_at_ms,
        )
        parent_key, _ = _inbound_source_key(
            event,
            scope,
            inbound_sender,
            parent_occurred_at_ms,
            _raw_text(getattr(event, "message_str", "")),
        )
        _set_extra(event, INBOUND_SOURCE_EXTRA, parent_key)
    material = "\x1f".join(
        (
            scope.platform_id,
            scope.bot_account_id,
            scope.kind.value,
            scope.session_id,
            parent_key,
            result_type,
            content,
        )
    ).encode("utf-8")
    components = _components(result)
    mentions = _mentions(components)
    return RawEventInput(
        origin_kind=OriginKind.BOT_RESPONSE,
        occurred_time_source=OccurredTimeSource.OBSERVED_FALLBACK,
        platform_message_id_state=PlatformMessageIdState.MISSING,
        source_key=f"{scope.platform_id}:bot:{hashlib.sha256(material).hexdigest()}",
        scope=scope,
        sender_account_id=scope.bot_account_id,
        sender_display_name=_text(_value(event, "get_self_name")) or scope.bot_account_id,
        sender_is_bot=True,
        observed_at_ms=observed_at_ms,
        occurred_at_ms=observed_at_ms,
        content=content,
        reply_to_source_key=parent_key,
        mentions=mentions,
        metadata=_metadata(
            event,
            components,
            reply_state="present",
            mention_state="present" if mentions else "missing",
            result_content_type=result_type[:64],
        ),
    )
