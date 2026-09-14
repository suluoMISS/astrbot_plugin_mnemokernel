"""Load an extracted release package through a minimal AstrBot-compatible shell."""

from __future__ import annotations

import argparse
import asyncio
from dataclasses import dataclass, field
from datetime import datetime, timezone
import importlib
import json
import sys
import tempfile
import time
import types
from pathlib import Path
from typing import Any, Generic, TypeVar


class _Logger:
    def info(self, _message: str) -> None:
        pass

    def warning(self, _message: str) -> None:
        pass


class _Filter:
    class EventMessageType:
        ALL = "all"

    @staticmethod
    def _decorator(*_args, **_kwargs):
        return lambda function: function

    event_message_type = _decorator
    after_message_sent = _decorator
    command = _decorator


def install_astrbot_shell(data_directory: Path) -> list[Any]:
    astrbot = types.ModuleType("astrbot")
    api = types.ModuleType("astrbot.api")
    event = types.ModuleType("astrbot.api.event")
    star = types.ModuleType("astrbot.api.star")
    core = types.ModuleType("astrbot.core")
    agent = types.ModuleType("astrbot.core.agent")
    agent_tool = types.ModuleType("astrbot.core.agent.tool")
    astr_agent_context = types.ModuleType("astrbot.core.astr_agent_context")

    class Star:
        def __init__(self, context):
            self.context = context

    class StarTools:
        @staticmethod
        def get_data_dir() -> str:
            return str(data_directory)

    context_tools: list[Any] = []

    class Context:
        def add_llm_tools(self, *tools: Any) -> None:
            context_tools.extend(tools)

        async def llm_generate(self, **kwargs: Any) -> dict[str, str]:
            prompt = str(kwargs["prompt"])
            context = json.loads(prompt.split("事件上下文：", 1)[1])
            evidence_event_ids = [
                event["event_id"] for event in context.get("events", []) if event.get("event_id")
            ]
            if not evidence_event_ids:
                raise AssertionError("package smoke model received no journal evidence")
            proposal = {
                "title": "Package smoke journal",
                "summary": "Package smoke exercised the real plugin hook path.",
                "cues": ["package smoke"],
                "open_loops": [],
                "claims": [
                    {
                        "claim_kind": "fact",
                        "claim_text": "Package smoke evidence was stored.",
                        "epistemic_status": "assistant_inferred",
                        "evidence_event_ids": [evidence_event_ids[0]],
                    }
                ],
            }
            return {"completion_text": json.dumps(proposal)}

    context_tools.clear()

    type_context = TypeVar("type_context")

    @dataclass
    class FunctionTool(Generic[type_context]):
        name: str
        description: str
        parameters: dict[str, Any]
        handler: Any = None
        handler_module_path: str | None = None
        active: bool = True
        is_background_task: bool = False

        async def call(self, _context: Any, **_kwargs: Any) -> Any:
            raise NotImplementedError

    class AstrAgentContext:
        pass

    def field_factory(*, default_factory):
        return field(default_factory=default_factory)

    pydantic = types.ModuleType("pydantic")
    pydantic_dataclasses = types.ModuleType("pydantic.dataclasses")
    pydantic.Field = field_factory
    pydantic_dataclasses.dataclass = dataclass
    pydantic.dataclasses = pydantic_dataclasses

    def register(*_args, **_kwargs):
        return lambda plugin_class: plugin_class

    api.AstrBotConfig = dict
    api.logger = _Logger()
    event.filter = _Filter()
    star.Context = Context
    star.Star = Star
    star.StarTools = StarTools
    star.register = register
    astrbot.api = api
    agent_tool.FunctionTool = FunctionTool
    astr_agent_context.AstrAgentContext = AstrAgentContext
    core.agent = agent
    agent.tool = agent_tool
    core.astr_agent_context = astr_agent_context
    astrbot.core = core

    sys.modules.update(
        {
            "astrbot": astrbot,
            "astrbot.api": api,
            "astrbot.api.event": event,
            "astrbot.api.star": star,
            "astrbot.core": core,
            "astrbot.core.agent": agent,
            "astrbot.core.agent.tool": agent_tool,
            "astrbot.core.astr_agent_context": astr_agent_context,
            "pydantic": pydantic,
            "pydantic.dataclasses": pydantic_dataclasses,
        }
    )
    return context_tools


async def exercise(package_directory: Path, data_directory: Path) -> None:
    context_tools = install_astrbot_shell(data_directory)
    sys.path.insert(0, str(package_directory.parent))
    module = importlib.import_module(f"{package_directory.name}.main")

    class Plain:
        def __init__(self, text: str):
            self.text = text

    class Result:
        result_content_type = "llm_result"

        def __init__(self, text: str):
            self.chain = [Plain(text)]

        def get_plain_text(self) -> str:
            return " ".join(component.text for component in self.chain)

    class Message:
        def __init__(self):
            self.message_id = "package-inbound"
            self.timestamp = int(time.time())
            self.message = [Plain("package smoke inbound")]
            self.reply_to_message_id = None

    class Event:
        def __init__(self):
            self.message_obj = Message()
            self.message_str = "package smoke inbound"
            self.unified_msg_origin = "smoke:FriendMessage:user-1"
            self._extras: dict[str, Any] = {}
            self._result = Result("package smoke outbound")

        def get_platform_id(self) -> str:
            return "smoke"

        def get_sender_id(self) -> str:
            return "user-1"

        def get_sender_name(self) -> str:
            return "Smoke User"

        def get_self_id(self) -> str:
            return "bot-1"

        def get_self_name(self) -> str:
            return "Smoke Bot"

        def get_group_id(self) -> str:
            return ""

        def get_extra(self, key: str | None = None, default: Any = None) -> Any:
            if key is None:
                return self._extras
            return self._extras.get(key, default)

        def set_extra(self, key: str, value: Any) -> None:
            self._extras[key] = value

        def get_result(self) -> Result:
            return self._result

    event = Event()
    journal_date = datetime.now(timezone.utc).date().isoformat()
    context = module.Context()
    plugin = module.MnemoKernelPlugin(
        context,
        {
            "general": {"enabled": True},
            "capture": {"enabled": True, "capture_private": True},
            "recall": {"enabled": True},
            "diary": {
                "enabled": True,
                "timezone": "UTC",
                "daily_hour": 23,
                "daily_minute": 59,
            },
            "storage": {"database_filename": "package-smoke.sqlite3"},
        },
    )
    await plugin.initialize()
    assert plugin._kernel is not None
    assert plugin._kernel.available
    assert plugin._kernel.health()["schema_version"] == 4
    assert len(context_tools) == 1
    assert context_tools[0].name == "recollect"
    await plugin.capture_message(event)
    await plugin.capture_sent_message(event)
    summary = await plugin._summarize_journal(event, journal_date, include_summary=False)
    assert "日记已生成" in summary
    journal = await plugin._read_journal(event, journal_date)
    assert "Package smoke journal" in journal
    recall = await plugin._recollect_text(
        event,
        need_type="fact",
        reason="验证发布包回忆链路",
        cues=["package smoke"],
    )
    assert "日期" in recall and "类型 fact" in recall and "证据已支持" in recall
    maintenance = await plugin._maintain_scope(event)
    assert "保留期清理完成" in maintenance
    await plugin.terminate()
    assert not plugin._kernel.available
    assert (data_directory / "package-smoke.sqlite3").is_file()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("package_directory", type=Path)
    args = parser.parse_args()
    package = args.package_directory.resolve()
    if package.name != "astrbot_plugin_mnemokernel" or not package.is_dir():
        raise RuntimeError(f"unexpected extracted package: {package}")
    with tempfile.TemporaryDirectory(prefix="mnemokernel-plugin-smoke-") as temporary:
        asyncio.run(exercise(package, Path(temporary)))
    print("plugin-package-smoke: ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
