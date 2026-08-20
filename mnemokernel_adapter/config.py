"""Configuration parsing with conservative defaults."""

from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Mapping


def _mapping(value: Any) -> Mapping[str, Any]:
    return value if isinstance(value, Mapping) else {}


def _bounded_int(value: Any, default: int, minimum: int, maximum: int) -> int:
    try:
        parsed = int(value)
    except (TypeError, ValueError):
        return default
    return max(minimum, min(maximum, parsed))


@dataclass(frozen=True, slots=True)
class CaptureConfig:
    enabled: bool = False
    capture_private: bool = True
    group_allowlist: frozenset[str] = field(default_factory=frozenset)
    max_content_chars: int = 12_000

    def permits(self, conversation_kind: str, session_id: str) -> bool:
        if not self.enabled:
            return False
        if conversation_kind == "private":
            return self.capture_private
        return conversation_kind == "group" and session_id in self.group_allowlist


@dataclass(frozen=True, slots=True)
class RecallConfig:
    enabled: bool = False
    max_cues: int = 8
    max_reason_chars: int = 500


@dataclass(frozen=True, slots=True)
class DiaryConfig:
    enabled: bool = False
    timezone: str = "Asia/Shanghai"
    daily_hour: int = 2
    daily_minute: int = 0


@dataclass(frozen=True, slots=True)
class PrivacyConfig:
    group_controller_ids: frozenset[str] = field(default_factory=frozenset)

    def permits_control(self, conversation_kind: str, actor_id: str) -> bool:
        return conversation_kind == "private" or (
            conversation_kind == "group" and actor_id in self.group_controller_ids
        )


@dataclass(frozen=True, slots=True)
class MnemoKernelConfig:
    enabled: bool = True
    diagnostic_logging: bool = False
    database_filename: str = "mnemokernel.sqlite3"
    retention_days: int = 14
    capture: CaptureConfig = field(default_factory=CaptureConfig)
    recall: RecallConfig = field(default_factory=RecallConfig)
    diary: DiaryConfig = field(default_factory=DiaryConfig)
    privacy: PrivacyConfig = field(default_factory=PrivacyConfig)

    @classmethod
    def from_astrbot_config(cls, config: Mapping[str, Any] | None) -> "MnemoKernelConfig":
        root = _mapping(config)
        general = _mapping(root.get("general"))
        capture = _mapping(root.get("capture"))
        recall = _mapping(root.get("recall"))
        diary = _mapping(root.get("diary"))
        privacy = _mapping(root.get("privacy"))
        storage = _mapping(root.get("storage"))

        raw_filename = str(storage.get("database_filename", "mnemokernel.sqlite3")).strip()
        filename = Path(raw_filename).name
        if (
            filename in {"", ".", ".."}
            or filename != raw_filename
            or "/" in raw_filename
            or "\\" in raw_filename
            or "\x00" in raw_filename
        ):
            filename = "mnemokernel.sqlite3"

        allowlist_value = capture.get("group_allowlist", [])
        allowlist = (
            frozenset(str(item).strip() for item in allowlist_value if str(item).strip())
            if isinstance(allowlist_value, (list, tuple, set, frozenset))
            else frozenset()
        )
        controller_value = privacy.get("group_controller_ids", [])
        controllers = (
            frozenset(str(item).strip() for item in controller_value if str(item).strip())
            if isinstance(controller_value, (list, tuple, set, frozenset))
            else frozenset()
        )

        return cls(
            enabled=bool(general.get("enabled", True)),
            diagnostic_logging=bool(general.get("diagnostic_logging", False)),
            database_filename=filename,
            retention_days=_bounded_int(storage.get("retention_days"), 14, 1, 3_650),
            capture=CaptureConfig(
                enabled=bool(capture.get("enabled", False)),
                capture_private=bool(capture.get("capture_private", True)),
                group_allowlist=allowlist,
                max_content_chars=_bounded_int(
                    capture.get("max_content_chars"), 12_000, 256, 100_000
                ),
            ),
            recall=RecallConfig(
                enabled=bool(recall.get("enabled", False)),
                max_cues=_bounded_int(recall.get("max_cues"), 8, 1, 24),
                max_reason_chars=_bounded_int(
                    recall.get("max_reason_chars"), 500, 32, 2_000
                ),
            ),
            diary=DiaryConfig(
                enabled=bool(diary.get("enabled", False)),
                timezone=str(diary.get("timezone", "Asia/Shanghai")).strip()
                or "Asia/Shanghai",
                daily_hour=_bounded_int(diary.get("daily_hour"), 2, 0, 23),
                daily_minute=_bounded_int(diary.get("daily_minute"), 0, 0, 59),
            ),
            privacy=PrivacyConfig(group_controller_ids=controllers),
        )

