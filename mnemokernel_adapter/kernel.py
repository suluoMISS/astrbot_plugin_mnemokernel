"""Fail-closed bridge to the Rust extension.

There is intentionally no SQLite fallback in this module. If the native core
cannot be loaded, all memory operations remain disabled while normal chat keeps
working.
"""

from __future__ import annotations

import importlib
import json
import platform
import sys
import tempfile
import threading
import zipfile
from dataclasses import dataclass, field
from pathlib import Path
from pathlib import PurePosixPath
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


def _module_supports_inspector(module: ModuleType | None) -> bool:
    kernel_type = getattr(module, "Kernel", None)
    return callable(getattr(kernel_type, "inspect_json", None))


def _import_runtime(runtime_dir: Path) -> ModuleType:
    """Import one runtime directory while bypassing stale module cache."""
    cached_module = sys.modules.get("_mnemokernel")
    runtime_root = runtime_dir.resolve()
    cached_file = getattr(cached_module, "__file__", None)
    cached_from_runtime = False
    if cached_file:
        try:
            Path(cached_file).resolve().relative_to(runtime_root)
            cached_from_runtime = True
        except (OSError, ValueError):
            pass
    if cached_module is not None and (
        not cached_from_runtime or not _module_supports_inspector(cached_module)
    ):
        sys.modules.pop("_mnemokernel", None)

    runtime_path = str(runtime_dir)
    sys.path.insert(0, runtime_path)
    importlib.invalidate_caches()
    try:
        module = importlib.import_module("_mnemokernel")
        if not _module_supports_inspector(module):
            raise ImportError("bundled native module has no inspector interface")
        return module
    except (ImportError, OSError):
        sys.modules.pop("_mnemokernel", None)
        if cached_module is not None:
            sys.modules["_mnemokernel"] = cached_module
        raise
    finally:
        try:
            sys.path.remove(runtime_path)
        except ValueError:
            pass


def _extract_bundled_wheel() -> Path | None:
    """Extract a platform wheel bundled in a release ZIP for self-repair."""
    root = Path(__file__).resolve().parents[1]
    native_dir = root / "native"
    if not native_dir.is_dir():
        return None
    machine = platform.machine().lower()
    if sys.platform == "win32" and machine in {"amd64", "x86_64"}:
        pattern = "*-win_amd64.whl"
    elif sys.platform == "linux" and machine in {"x86_64", "amd64"}:
        pattern = "*-manylinux_2_34_x86_64.whl"
    else:
        return None
    wheels = sorted(native_dir.glob(pattern))
    if len(wheels) != 1:
        return None
    repaired = Path(tempfile.mkdtemp(prefix="mnemokernel-native-repair-"))
    try:
        with zipfile.ZipFile(wheels[0]) as archive:
            for member in archive.infolist():
                name = PurePosixPath(member.filename)
                if (
                    name.is_absolute()
                    or ".." in name.parts
                    or not name.parts
                    or name.parts[0] != "_mnemokernel"
                    or member.is_dir()
                ):
                    continue
                destination = repaired.joinpath(*name.parts)
                destination.parent.mkdir(parents=True, exist_ok=True)
                destination.write_bytes(archive.read(member))
    except (OSError, zipfile.BadZipFile):
        return None
    if not (repaired / "_mnemokernel" / "__init__.py").is_file():
        return None
    return repaired


def _load_native_module() -> ModuleType:
    """Load the bundled release runtime, or the installed extension.

    AstrBot installs plugin requirements into a shared target directory, which
    can still contain an older extension after a plugin update. The release ZIP
    carries its exact ABI3 runtime under ``native_runtime`` and must take
    precedence so Python code and native protocol stay in lockstep. The
    installed extension remains a development fallback when no bundled runtime
    exists or the bundled binary cannot load.
    """
    runtime_dir = Path(__file__).resolve().parents[1] / "native_runtime"
    bundled_error: Exception | None = None
    if runtime_dir.is_dir():
        try:
            return _import_runtime(runtime_dir)
        except (ImportError, OSError) as exc:
            bundled_error = exc
    repaired_runtime = _extract_bundled_wheel()
    if repaired_runtime is not None:
        try:
            return _import_runtime(repaired_runtime)
        except (ImportError, OSError) as exc:
            bundled_error = exc

    try:
        return importlib.import_module("_mnemokernel")
    except (ImportError, OSError) as installed_error:
        sys.modules.pop("_mnemokernel", None)
        if bundled_error is not None:
            raise bundled_error from installed_error
        raise


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

    def __init__(
        self,
        native: Any | None,
        status: KernelStatus,
        native_module: ModuleType | None = None,
    ):
        self._native = native
        self._native_module = native_module
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
                native_module=module,
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

    @property
    def inspector_available(self) -> bool:
        with self._lock:
            return self.available and callable(getattr(self._native, "inspect_json", None))

    @property
    def native_module_path(self) -> str | None:
        with self._lock:
            path = getattr(self._native_module, "__file__", None)
            return str(Path(path).resolve()) if path else None

    def inspect(self, payload: Mapping[str, Any]) -> Mapping[str, Any]:
        with self._lock:
            native = self._require_native()
            if not self.inspector_available:
                raise KernelUnavailableError("原生内核版本过旧，请安装包含数据库浏览接口的新版本安装包。")
            return self._decode(native.inspect_json(json.dumps(payload, ensure_ascii=False)))

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
                self._native_module = None
                self._status = KernelStatus(
                    available=False,
                    detail="native kernel closed",
                    protocol=self._status.protocol,
                )

