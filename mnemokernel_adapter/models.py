"""Versioned transport models shared by the AstrBot shell and native core."""

from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Mapping, Sequence


PROTOCOL_VERSION = "mnemokernel.v1"
SCHEMA_VERSION = 4


class ScopeKind(str, Enum):
    PRIVATE = "private"
    GROUP = "group"


class OriginKind(str, Enum):
    USER_MESSAGE = "user_message"
    BOT_RESPONSE = "bot_response"


class OccurredTimeSource(str, Enum):
    PLATFORM = "platform"
    OBSERVED_FALLBACK = "observed_fallback"


class PlatformMessageIdState(str, Enum):
    PRESENT = "present"
    MISSING = "missing"


class NeedType(str, Enum):
    FACT = "fact"
    PREFERENCE = "preference"
    DECISION = "decision"
    COMMITMENT = "commitment"
    RELATIONSHIP = "relationship"
    EPISODE = "episode"
    PROCEDURE = "procedure"


class RecallDepth(str, Enum):
    GLANCE = "glance"
    FOCUSED = "focused"
    DEEP = "deep"


def _required_text(name: str, value: str, maximum: int) -> str:
    normalized = str(value).strip()
    if not normalized:
        raise ValueError(f"{name} must not be empty")
    if len(normalized) > maximum:
        raise ValueError(f"{name} exceeds {maximum} characters")
    return normalized


@dataclass(frozen=True, slots=True)
class ScopeDescriptor:
    platform_id: str
    bot_account_id: str
    kind: ScopeKind
    session_id: str
    persona_id: str = "default"

    def __post_init__(self) -> None:
        object.__setattr__(self, "platform_id", _required_text("platform_id", self.platform_id, 128))
        object.__setattr__(
            self, "bot_account_id", _required_text("bot_account_id", self.bot_account_id, 256)
        )
        object.__setattr__(self, "session_id", _required_text("session_id", self.session_id, 512))
        object.__setattr__(self, "persona_id", _required_text("persona_id", self.persona_id, 256))

    def to_dict(self) -> dict[str, str]:
        return {
            "platform_id": self.platform_id,
            "bot_account_id": self.bot_account_id,
            "kind": self.kind.value,
            "session_id": self.session_id,
            "persona_id": self.persona_id,
        }


@dataclass(frozen=True, slots=True)
class RawEventInput:
    origin_kind: OriginKind
    occurred_time_source: OccurredTimeSource
    platform_message_id_state: PlatformMessageIdState
    source_key: str
    scope: ScopeDescriptor
    sender_account_id: str
    sender_display_name: str
    sender_is_bot: bool
    observed_at_ms: int
    occurred_at_ms: int
    content: str
    reply_to_source_key: str | None = None
    mentions: tuple[str, ...] = ()
    metadata: Mapping[str, Any] = field(default_factory=dict)

    def __post_init__(self) -> None:
        object.__setattr__(self, "source_key", _required_text("source_key", self.source_key, 512))
        object.__setattr__(
            self,
            "sender_account_id",
            _required_text("sender_account_id", self.sender_account_id, 512),
        )
        if self.observed_at_ms < 0 or self.occurred_at_ms < 0:
            raise ValueError("event timestamps must be non-negative")
        if self.sender_is_bot != (self.origin_kind is OriginKind.BOT_RESPONSE):
            raise ValueError("origin_kind and sender_is_bot are inconsistent")
        if not self.content.strip():
            raise ValueError("content must not be empty")
        if len(self.content) > 100_000:
            raise ValueError("content exceeds protocol limit")
        if self.reply_to_source_key is not None and len(self.reply_to_source_key) > 512:
            raise ValueError("reply_to_source_key exceeds protocol limit")
        if len(self.mentions) > 64:
            raise ValueError("too many mentions")

    def to_dict(self) -> dict[str, Any]:
        return {
            "protocol": PROTOCOL_VERSION,
            "origin_kind": self.origin_kind.value,
            "occurred_time_source": self.occurred_time_source.value,
            "platform_message_id_state": self.platform_message_id_state.value,
            "source_key": self.source_key,
            "scope": self.scope.to_dict(),
            "sender_account_id": self.sender_account_id,
            "sender_display_name": self.sender_display_name[:512],
            "sender_is_bot": self.sender_is_bot,
            "observed_at_ms": self.observed_at_ms,
            "occurred_at_ms": self.occurred_at_ms,
            "content": self.content,
            "reply_to_source_key": self.reply_to_source_key,
            "mentions": list(self.mentions),
            "metadata": dict(self.metadata),
        }


@dataclass(frozen=True, slots=True)
class PurgeScopeRequest:
    scope: ScopeDescriptor
    actor_id: str
    reason: str

    def __post_init__(self) -> None:
        object.__setattr__(self, "actor_id", _required_text("actor_id", self.actor_id, 512))
        object.__setattr__(self, "reason", _required_text("reason", self.reason, 2_000))

    def to_dict(self) -> dict[str, Any]:
        return {
            "protocol": PROTOCOL_VERSION,
            "scope": self.scope.to_dict(),
            "actor_id": self.actor_id,
            "reason": self.reason,
        }


class CapturePolicyAction(str, Enum):
    PAUSE = "pause"
    RESUME = "resume"


@dataclass(frozen=True, slots=True)
class CapturePolicyRequest:
    scope: ScopeDescriptor
    action: CapturePolicyAction
    actor_id: str
    reason: str

    def __post_init__(self) -> None:
        object.__setattr__(self, "actor_id", _required_text("actor_id", self.actor_id, 512))
        object.__setattr__(self, "reason", _required_text("reason", self.reason, 2_000))

    def to_dict(self) -> dict[str, Any]:
        return {
            "protocol": PROTOCOL_VERSION,
            "scope": self.scope.to_dict(),
            "action": self.action.value,
            "actor_id": self.actor_id,
            "reason": self.reason,
        }


@dataclass(frozen=True, slots=True)
class RetentionRequest:
    scope: ScopeDescriptor
    cutoff_at_ms: int
    actor_id: str
    reason: str

    def __post_init__(self) -> None:
        if self.cutoff_at_ms < 0:
            raise ValueError("cutoff_at_ms must be non-negative")
        object.__setattr__(self, "actor_id", _required_text("actor_id", self.actor_id, 512))
        object.__setattr__(self, "reason", _required_text("reason", self.reason, 2_000))

    def to_dict(self) -> dict[str, Any]:
        return {
            "protocol": PROTOCOL_VERSION,
            "scope": self.scope.to_dict(),
            "cutoff_at_ms": self.cutoff_at_ms,
            "actor_id": self.actor_id,
            "reason": self.reason,
        }


@dataclass(frozen=True, slots=True)
class ScopeStatsRequest:
    scope: ScopeDescriptor

    def to_dict(self) -> dict[str, Any]:
        return {
            "protocol": PROTOCOL_VERSION,
            "scope": self.scope.to_dict(),
        }


class JournalClaimKind(str, Enum):
    FACT = "fact"
    PREFERENCE = "preference"
    DECISION = "decision"
    COMMITMENT = "commitment"
    RELATIONSHIP = "relationship"
    PROCEDURE = "procedure"
    OPEN_LOOP = "open_loop"


class JournalEpistemicStatus(str, Enum):
    ASSISTANT_INFERRED = "assistant_inferred"
    USER_CONFIRMED = "user_confirmed"
    DISPUTED = "disputed"


@dataclass(frozen=True, slots=True)
class DailyJournalRequest:
    scope: ScopeDescriptor
    journal_date: str
    occurred_from_ms: int
    occurred_to_ms: int

    def __post_init__(self) -> None:
        if (
            len(self.journal_date) != 10
            or self.journal_date[4] != "-"
            or self.journal_date[7] != "-"
            or not all(
                char.isdigit()
                for index, char in enumerate(self.journal_date)
                if index not in {4, 7}
            )
        ):
            raise ValueError("journal_date must use YYYY-MM-DD")
        if (
            self.occurred_from_ms < 0
            or self.occurred_to_ms <= self.occurred_from_ms
            or self.occurred_to_ms - self.occurred_from_ms > 172_800_000
        ):
            raise ValueError("journal time range is invalid")

    def to_dict(self) -> dict[str, Any]:
        return {
            "protocol": PROTOCOL_VERSION,
            "scope": self.scope.to_dict(),
            "journal_date": self.journal_date,
            "occurred_from_ms": self.occurred_from_ms,
            "occurred_to_ms": self.occurred_to_ms,
        }


@dataclass(frozen=True, slots=True)
class JournalClaimProposal:
    claim_kind: JournalClaimKind
    claim_text: str
    epistemic_status: JournalEpistemicStatus
    evidence_event_ids: tuple[str, ...]

    def to_dict(self) -> dict[str, Any]:
        return {
            "claim_kind": self.claim_kind.value,
            "claim_text": self.claim_text,
            "epistemic_status": self.epistemic_status.value,
            "evidence_event_ids": list(self.evidence_event_ids),
        }


@dataclass(frozen=True, slots=True)
class DailyJournalProposal:
    proposal_id: str
    scope_id: str
    journal_date: str
    occurred_from_ms: int
    occurred_to_ms: int
    title: str
    summary: str
    cues: tuple[str, ...]
    open_loops: tuple[str, ...]
    claims: tuple[JournalClaimProposal, ...]

    @classmethod
    def from_model(
        cls,
        value: Mapping[str, Any],
        *,
        request: DailyJournalRequest,
        scope_id: str,
        proposal_id: str,
    ) -> "DailyJournalProposal":
        if not isinstance(value, Mapping):
            raise ValueError("journal model output must be a JSON object")

        def text(name: str, maximum: int, default: str = "") -> str:
            raw = value.get(name, default)
            if not isinstance(raw, str) or not raw.strip():
                raise ValueError(f"journal field {name} must be non-empty text")
            clean = raw.strip()
            if len(clean) > maximum:
                raise ValueError(f"journal field {name} exceeds {maximum} characters")
            return clean

        def text_list(name: str, maximum_items: int, item_maximum: int) -> tuple[str, ...]:
            raw = value.get(name, [])
            if not isinstance(raw, list) or len(raw) > maximum_items:
                raise ValueError(f"journal field {name} must be a short array")
            result = []
            for item in raw:
                if not isinstance(item, str) or not item.strip() or len(item.strip()) > item_maximum:
                    raise ValueError(f"journal field {name} contains invalid text")
                result.append(item.strip())
            return tuple(result)

        raw_claims = value.get("claims", [])
        if not isinstance(raw_claims, list) or len(raw_claims) > 32:
            raise ValueError("journal claims must be a short array")
        claims = []
        for raw_claim in raw_claims:
            if not isinstance(raw_claim, Mapping):
                raise ValueError("journal claim must be an object")
            try:
                claim_kind = JournalClaimKind(str(raw_claim["claim_kind"]).strip().lower())
                status = JournalEpistemicStatus(
                    str(raw_claim.get("epistemic_status", "assistant_inferred")).strip().lower()
                )
            except (KeyError, ValueError) as exc:
                raise ValueError("journal claim kind or status is invalid") from exc
            claim_text = raw_claim.get("claim_text")
            evidence = raw_claim.get("evidence_event_ids")
            if not isinstance(claim_text, str) or not claim_text.strip() or len(claim_text.strip()) > 2_000:
                raise ValueError("journal claim text is invalid")
            if not isinstance(evidence, list) or not evidence or len(evidence) > 16:
                raise ValueError("journal claim evidence is invalid")
            clean_evidence = tuple(str(item).strip() for item in evidence)
            if any(not item for item in clean_evidence) or len(set(clean_evidence)) != len(clean_evidence):
                raise ValueError("journal claim evidence must be unique")
            claims.append(
                JournalClaimProposal(
                    claim_kind=claim_kind,
                    claim_text=claim_text.strip(),
                    epistemic_status=status,
                    evidence_event_ids=clean_evidence,
                )
            )

        return cls(
            proposal_id=_required_text("proposal_id", proposal_id, 128),
            scope_id=_required_text("scope_id", scope_id, 64),
            journal_date=request.journal_date,
            occurred_from_ms=request.occurred_from_ms,
            occurred_to_ms=request.occurred_to_ms,
            title=text("title", 200),
            summary=text("summary", 6_000),
            cues=text_list("cues", 16, 256),
            open_loops=text_list("open_loops", 16, 500),
            claims=tuple(claims),
        )

    def to_dict(self) -> dict[str, Any]:
        return {
            "protocol": PROTOCOL_VERSION,
            "proposal_id": self.proposal_id,
            "scope_id": self.scope_id,
            "journal_date": self.journal_date,
            "occurred_from_ms": self.occurred_from_ms,
            "occurred_to_ms": self.occurred_to_ms,
            "title": self.title,
            "summary": self.summary,
            "cues": list(self.cues),
            "open_loops": list(self.open_loops),
            "claims": [claim.to_dict() for claim in self.claims],
        }


@dataclass(frozen=True, slots=True)
class RecallRequest:
    scope: ScopeDescriptor
    need_type: NeedType
    reason: str
    cues: tuple[str, ...]
    depth: RecallDepth = RecallDepth.GLANCE
    time_hint: str | None = None

    @classmethod
    def build(
        cls,
        *,
        scope: ScopeDescriptor,
        need_type: str,
        reason: str,
        cues: Sequence[str],
        depth: str = "glance",
        time_hint: str | None = None,
        max_cues: int = 8,
        max_reason_chars: int = 500,
    ) -> "RecallRequest":
        parsed_need = NeedType(str(need_type).strip().lower())
        parsed_depth = RecallDepth(str(depth).strip().lower())
        clean_reason = _required_text("reason", reason, max_reason_chars)
        if isinstance(cues, (str, bytes)):
            raise ValueError("cues must be an array of short strings")
        clean_cues = tuple(cue for cue in (str(item).strip() for item in cues) if cue)
        if not clean_cues:
            raise ValueError("at least one recall cue is required")
        if len(clean_cues) > max_cues:
            raise ValueError(f"too many recall cues; maximum is {max_cues}")
        if len(set(clean_cues)) != len(clean_cues):
            raise ValueError("recall cues must be unique")
        if any(len(cue) > 256 for cue in clean_cues):
            raise ValueError("a recall cue exceeds 256 characters")
        clean_time_hint = str(time_hint).strip() if time_hint else None
        if clean_time_hint and len(clean_time_hint) > 256:
            raise ValueError("time_hint exceeds 256 characters")
        return cls(
            scope=scope,
            need_type=parsed_need,
            reason=clean_reason,
            cues=clean_cues,
            depth=parsed_depth,
            time_hint=clean_time_hint,
        )

    def to_dict(self) -> dict[str, Any]:
        return {
            "protocol": PROTOCOL_VERSION,
            "scope": self.scope.to_dict(),
            "need_type": self.need_type.value,
            "reason": self.reason,
            "cues": list(self.cues),
            "depth": self.depth.value,
            "time_hint": self.time_hint,
        }
