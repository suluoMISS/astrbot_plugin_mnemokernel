"""Fail-closed bridge to the Rust extension.

There is intentionally no SQLite fallback in this module. If the native core
cannot be loaded, all memory operations remain disabled while normal chat keeps
working.
"""

from __future__ import annotations

import importlib
import json
import sys
import threading
from dataclasses import dataclass, field
from pathlib import Path
from types import ModuleType
from typing import Any, Mapping

from .models import (
    CapturePolicyRequest,
    DailyJournalProposal,
    DailyJournalRequest,
    PROTOCOL_VERSION,
    PurgeScopeRequest,
    RawEventInput,
    RecallRequest,
    RetentionRequest,
    SCHEMA_VERSION,
    ScopeStatsRequest,
)


class KernelUnavailableError(RuntimeError):
    pass


def _load_native_module() -> ModuleType:
    """Load the installed extension, or the bundled release runtime.

    AstrBot installs plugin requirements into a shared target directory. A local
    wheel can fail there for platform or installer reasons, so the release ZIP
    also carries the exact ABI3 runtime under ``native_runtime``. The fallback
    keeps normal chat usable even when that binary cannot load.
    """
    try:
        return importlib.import_module("_mnemokernel")
    except (ImportError, OSError) as installed_error:
        sys.modules.pop("_mnemokernel", None)
        runtime_dir = Path(__file__).resolve().parents[1] / "native_runtime"
        if not runtime_dir.is_dir():
            raise installed_error

        runtime_path = str(runtime_dir)
        sys.path.insert(0, runtime_path)
        importlib.invalidate_caches()
        try:
            return importlib.import_module("_mnemokernel")
        except Exception:
            sys.modules.pop("_mnemokernel", None)
            raise installed_error
        finally:
            try:
                sys.path.remove(runtime_path)
            except ValueError:
                pass


@dataclass(frozen=True, slots=True)
class KernelCapabilities:
    capture_ready: bool = False
    episode_ready: bool = False
    recall_ready: bool = False

    @classmethod
    def from_mapping(cls, value: Any) -> "KernelCapabilities":
        if not isinstance(value, Mapping):
            return cls()
        return cls(
            capture_ready=value.get("capture_ready") is True,
            episode_ready=value.get("episode_ready") is True,
            recall_ready=value.get("recall_ready") is True,
        )


@dataclass(frozen=True, slots=True)
class KernelStatus:
    available: bool
    detail: str
    protocol: str | None = None
    capabilities: KernelCapabilities = field(default_factory=KernelCapabilities)


class KernelClient:
    """Small, synchronized wrapper around the PyO3 Kernel object."""

    def __init__(self, native: Any | None, status: KernelStatus):
        self._native = native
        self._status = status
        self._lock = threading.RLock()

    @classmethod
    def open(
        cls,
        database_path: Path,
        *,
        native_module: ModuleType | None = None,
    ) -> "KernelClient":
        native = None
        try:
            module = native_module or _load_native_module()
            native = module.Kernel(str(database_path))
            health = json.loads(native.health_json())
            if health.get("status") != "ok":
                raise RuntimeError("native health check did not return ok")
            if health.get("protocol") != PROTOCOL_VERSION:
                raise RuntimeError(
                    "native protocol mismatch: "
                    f"expected {PROTOCOL_VERSION}, got {health.get('protocol')!r}"
                )
            if health.get("schema_version") != SCHEMA_VERSION:
                raise RuntimeError(
                    "native schema mismatch: "
                    f"expected {SCHEMA_VERSION}, got {health.get('schema_version')!r}"
                )
            return cls(
                native,
                KernelStatus(
                    available=True,
                    detail="native kernel ready",
                    protocol=str(health.get("protocol") or "unknown"),
                    capabilities=KernelCapabilities.from_mapping(health.get("capabilities")),
                ),
            )
        except Exception as exc:  # Import, ABI, migration, or database failure.
            if native is not None and hasattr(native, "close"):
                try:
                    native.close()
                except Exception:
                    # Preserve the initialization failure; the client is
                    # already being returned in a fail-closed state.
                    pass
            detail = f"{type(exc).__name__}: {exc}"
            return cls(None, KernelStatus(available=False, detail=detail))

    @property
    def status(self) -> KernelStatus:
        return self._status

    @property
    def available(self) -> bool:
        return self._native is not None and self._status.available

    def _require_native(self) -> Any:
        if not self.available:
            raise KernelUnavailableError(self._status.detail)
        return self._native

    @staticmethod
    def _decode(raw: str) -> Mapping[str, Any]:
        value = json.loads(raw)
        if not isinstance(value, dict):
            raise RuntimeError("native kernel returned a non-object response")
        return value

    def ingest_event(self, event: RawEventInput) -> Mapping[str, Any]:
        with self._lock:
            native = self._require_native()
            payload = json.dumps(
                event.to_dict(), ensure_ascii=False, separators=(",", ":")
            )
            return self._decode(native.ingest_event_json(payload))

    def request_recall(self, request: RecallRequest) -> Mapping[str, Any]:
        with self._lock:
            native = self._require_native()
            payload = json.dumps(
                request.to_dict(), ensure_ascii=False, separators=(",", ":")
            )
            return self._decode(native.recall_request_json(payload))

    def journal_context(self, request: DailyJournalRequest) -> Mapping[str, Any]:
        with self._lock:
            native = self._require_native()
            payload = json.dumps(
                request.to_dict(), ensure_ascii=False, separators=(",", ":")
            )
            return self._decode(native.journal_context_json(payload))

    def save_journal(
        self, request: DailyJournalRequest, proposal: DailyJournalProposal
    ) -> Mapping[str, Any]:
        with self._lock:
            native = self._require_native()
            request_payload = json.dumps(
                request.to_dict(), ensure_ascii=False, separators=(",", ":")
            )
            proposal_payload = json.dumps(
                proposal.to_dict(), ensure_ascii=False, separators=(",", ":")
            )
            return self._decode(
                native.save_journal_json(request_payload, proposal_payload)
            )

    def read_journal(self, request: DailyJournalRequest) -> Mapping[str, Any]:
        with self._lock:
            native = self._require_native()
            payload = json.dumps(
                request.to_dict(), ensure_ascii=False, separators=(",", ":")
            )
            return self._decode(native.read_journal_json(payload))

    def purge_scope(self, request: PurgeScopeRequest) -> Mapping[str, Any]:
        with self._lock:
            native = self._require_native()
            payload = json.dumps(
                request.to_dict(), ensure_ascii=False, separators=(",", ":")
            )
            return self._decode(native.purge_scope_json(payload))

    def set_capture_policy(self, request: CapturePolicyRequest) -> Mapping[str, Any]:
        with self._lock:
            native = self._require_native()
            payload = json.dumps(
                request.to_dict(), ensure_ascii=False, separators=(",", ":")
            )
            return self._decode(native.set_capture_policy_json(payload))

    def retain_payloads(self, request: RetentionRequest) -> Mapping[str, Any]:
        with self._lock:
            native = self._require_native()
            payload = json.dumps(
                request.to_dict(), ensure_ascii=False, separators=(",", ":")
            )
            return self._decode(native.retain_payloads_json(payload))

    def scope_stats(self, request: ScopeStatsRequest) -> Mapping[str, Any]:
        with self._lock:
            native = self._require_native()
            payload = json.dumps(
                request.to_dict(), ensure_ascii=False, separators=(",", ":")
            )
            return self._decode(native.scope_stats_json(payload))

    def health(self) -> Mapping[str, Any]:
        with self._lock:
            native = self._require_native()
            return self._decode(native.health_json())

    def close(self) -> None:
        with self._lock:
            native = self._native
            try:
                if native is not None and hasattr(native, "close"):
                    native.close()
            finally:
                # Even a broken native destructor must not leave the Python
                # shell believing that memory operations are still available.
                self._native = None
                self._status = KernelStatus(
                    available=False,
                    detail="native kernel closed",
                    protocol=self._status.protocol,
                )

