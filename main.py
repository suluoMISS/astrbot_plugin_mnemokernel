"""MnemoKernel v0.1 — AstrBot shell around the trusted native kernel."""

from __future__ import annotations

import asyncio
import hashlib
import json
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any
from zoneinfo import ZoneInfo

from astrbot.api import AstrBotConfig, logger
from astrbot.api.event import filter
from astrbot.api.star import Context, Star, StarTools, register

from .mnemokernel_adapter import (
    CapturePolicyAction,
    CapturePolicyRequest,
    DailyJournalProposal,
    DailyJournalRequest,
    KernelClient,
    KernelUnavailableError,
    MnemoKernelConfig,
    PurgeScopeRequest,
    RecallRequest,
    RetentionRequest,
    ScopeStatsRequest,
    actor_id_from_event,
    normalize_event,
    normalize_sent_event,
)
from .mnemokernel_adapter.events import scope_from_event
from .mnemokernel_adapter.scheduling import due_journal_date


VERSION = "v0.1.0"
BUILTIN_TIMEZONE_OFFSETS = {
    "UTC": 0,
    "Asia/Shanghai": 8,
    "Asia/Singapore": 8,
    "Asia/Tokyo": 9,
    "Asia/Seoul": 9,
    "Asia/Hong_Kong": 8,
    "Asia/Taipei": 8,
}


def _configured_timezone(name: str):
    try:
        return ZoneInfo(name)
    except Exception as exc:
        offset_hours = BUILTIN_TIMEZONE_OFFSETS.get(name)
        if offset_hours is None:
            raise ValueError(f"日记时区无效：{name}") from exc
        return timezone(timedelta(hours=offset_hours), name=name)


def _extract_json_object(raw_text: str) -> dict[str, Any]:
    """Accept strict JSON, plus one optional Markdown JSON fence."""
    text = str(raw_text).strip()
    if text.startswith("```"):
        lines = text.splitlines()
        if lines and lines[0].strip().startswith("```"):
            lines = lines[1:]
        if lines and lines[-1].strip() == "```":
            lines = lines[:-1]
        text = "\n".join(lines).strip()
    decoder = json.JSONDecoder()
    value, end = decoder.raw_decode(text)
    if text[end:].strip() or not isinstance(value, dict):
        raise ValueError("模型日记结果必须是单个 JSON 对象")
    return value


def _completion_text(response: Any) -> str:
    value = getattr(response, "completion_text", None)
    if value is None and isinstance(response, dict):
        value = response.get("completion_text")
    if not isinstance(value, str) or not value.strip():
        raise ValueError("模型没有返回可用的日记文本")
    return value.strip()


@register(
    "astrbot_plugin_mnemokernel",
    "suluo",
    "MnemoKernel（忆核）——证据驱动、按需回忆的长期记忆内核",
    version=VERSION,
)
class MnemoKernelPlugin(Star):
    def __init__(self, context: Context, config: AstrBotConfig):
        super().__init__(context)
        self._cfg = MnemoKernelConfig.from_astrbot_config(config)
        self._kernel: KernelClient | None = None
        self._capture_errors = 0
        self._recall_tool_registered = False
        self._forget_confirmations: dict[str, float] = {}
        self._journal_tasks: dict[str, asyncio.Task[None]] = {}
        self._journal_attempted: set[str] = set()
        self._database_path: Path | None = None
        from .mnemokernel_adapter.pages import register_pages
        self._pages = register_pages(context, self)

    async def initialize(self) -> None:
        if not self._cfg.enabled:
            logger.info("[mnemokernel] 插件已在配置中停用")
            return

        data_dir = Path(StarTools.get_data_dir())
        await asyncio.to_thread(data_dir.mkdir, parents=True, exist_ok=True)
        database = data_dir / self._cfg.database_filename
        self._database_path = database.resolve()
        self._kernel = await asyncio.to_thread(KernelClient.open, database)
        if self._kernel.available:
            capabilities = self._kernel.status.capabilities
            logger.info(
                f"[mnemokernel] {VERSION} 原生内核就绪 "
                f"protocol={self._kernel.status.protocol} "
                f"capture={capabilities.capture_ready} episode={capabilities.episode_ready} "
                f"recall={capabilities.recall_ready}"
            )
            if self._cfg.recall.enabled and capabilities.recall_ready:
                self._register_recall_tool()
        else:
            logger.warning(
                "[mnemokernel] 原生内核不可用，已安全停用全部记忆读写；"
                f"普通聊天不受影响。detail={self._kernel.status.detail}"
            )

    async def terminate(self) -> None:
        tasks = tuple(self._journal_tasks.values())
        for task in tasks:
            task.cancel()
        if tasks:
            await asyncio.gather(*tasks, return_exceptions=True)
        self._journal_tasks.clear()
        if self._kernel is not None:
            try:
                await asyncio.to_thread(self._kernel.close)
            except Exception as exc:
                # KernelClient closes its capability gate even when the native
                # destructor fails; shutdown should remain best-effort too.
                logger.warning(
                    "[mnemokernel] 原生内核关闭时出现异常，已保持不可用状态："
                    f"{type(exc).__name__}: {exc}"
                )

    @staticmethod
    def _local_scope_key(scope: Any) -> str:
        return hashlib.sha256(
            json.dumps(scope.to_dict(), sort_keys=True, separators=(",", ":")).encode("utf-8")
        ).hexdigest()

    def _control_identity(self, event: Any) -> tuple[Any, str]:
        scope = scope_from_event(event)
        actor_id = actor_id_from_event(event)
        if not self._cfg.privacy.permits_control(scope.kind.value, actor_id):
            raise PermissionError(
                "群聊中的暂停、恢复、遗忘和清理仅允许 privacy.group_controller_ids 中的账号执行。"
            )
        return scope, actor_id

    def _journal_request(self, event: Any, date_text: str = "") -> DailyJournalRequest:
        timezone_name = self._cfg.diary.timezone
        timezone = _configured_timezone(timezone_name)
        if date_text.strip():
            try:
                target_date = datetime.strptime(date_text.strip(), "%Y-%m-%d").date()
            except ValueError as exc:
                raise ValueError("日期必须使用 YYYY-MM-DD 格式") from exc
        else:
            target_date = datetime.now(timezone).date()
        start = datetime(
            target_date.year,
            target_date.month,
            target_date.day,
            tzinfo=timezone,
        )
        end = start + timedelta(days=1)
        return DailyJournalRequest(
            scope=scope_from_event(event),
            journal_date=target_date.isoformat(),
            occurred_from_ms=int(start.timestamp() * 1000),
            occurred_to_ms=int(end.timestamp() * 1000),
        )

    async def _generate_journal_proposal(
        self, event: Any, request: DailyJournalRequest, context: dict[str, Any]
    ) -> DailyJournalProposal:
        if not hasattr(self.context, "llm_generate"):
            raise RuntimeError("当前 AstrBot 版本没有 context.llm_generate 接口")
        prompt = (
            "你是一个严格的个人日记整理器。请根据下面的当天事件生成日记提案。\n"
            "只输出一个 JSON 对象，不要输出 Markdown、解释或前后缀。\n"
            "字段必须是：title(string)、summary(string)、cues(array[string])、"
            "open_loops(array[string])、claims(array[object])。\n"
            "每个 claim 必须包含 claim_kind、claim_text、epistemic_status、"
            "evidence_event_ids。claim_kind 只能是 fact、preference、decision、commitment、"
            "relationship、procedure、open_loop；epistemic_status 默认使用"
            "assistant_inferred。每个 claim 至少引用一个事件 ID。\n"
            "只总结事件中明确出现或可以直接归纳的内容，不补充常识，不猜测人物动机，"
            "不把普通闲聊强行写成长期事实。summary 应简短，按主题分开，最多保留少量"
            "真正有后续价值的事项。若 context_truncated=true，只能总结可见证据，"
            "不能把未出现的内容解释为没有发生。\n"
            f"目标日期：{request.journal_date}\n"
            f"事件上下文：{json.dumps(context, ensure_ascii=False, separators=(',', ':'))}"
        )
        provider_id = None
        umo = getattr(event, "unified_msg_origin", None)
        get_provider_id = getattr(self.context, "get_current_chat_provider_id", None)
        if callable(get_provider_id) and umo:
            provider_id = await get_provider_id(umo)
        kwargs: dict[str, Any] = {"prompt": prompt}
        if provider_id:
            kwargs["chat_provider_id"] = provider_id
        response = await self.context.llm_generate(**kwargs)
        parsed = _extract_json_object(_completion_text(response))
        fingerprint = hashlib.sha256(
            json.dumps(parsed, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode(
                "utf-8"
            )
        ).hexdigest()
        return DailyJournalProposal.from_model(
            parsed,
            request=request,
            scope_id=str(context.get("scope_id", "")),
            proposal_id=f"journal-{fingerprint}",
        )

    async def _summarize_journal(
        self,
        event: Any,
        date_text: str = "",
        *,
        skip_existing: bool = False,
        include_summary: bool = True,
    ) -> str:
        if not self._cfg.enabled or not self._cfg.diary.enabled:
            return "[MnemoKernel] 日记功能未启用。请在配置中打开 diary.enabled。"
        if (
            self._kernel is None
            or not self._kernel.available
            or not self._kernel.status.capabilities.episode_ready
        ):
            return "[MnemoKernel] 日记内核不可用；没有写入任何内容。"
        request = self._journal_request(event, date_text)
        if skip_existing:
            existing = await asyncio.to_thread(self._kernel.read_journal, request)
            if existing.get("status") == "found":
                return f"[MnemoKernel] {request.journal_date} 日记已存在，自动整理跳过。"
        context = await asyncio.to_thread(self._kernel.journal_context, request)
        if context.get("status") == "empty":
            return f"[MnemoKernel] {request.journal_date} 没有可整理的事件。"
        proposal = await self._generate_journal_proposal(event, request, dict(context))
        result = await asyncio.to_thread(self._kernel.save_journal, request, proposal)
        status_text = {
            "stored": "已生成",
            "updated": "已更新",
            "unchanged": "内容未变化，保持原版本",
        }.get(str(result.get("status")), "已处理")
        if not include_summary:
            return (
                f"[MnemoKernel] {request.journal_date} 日记{status_text}"
                f"（第 {result.get('version', '?')} 版）。"
            )
        return (
            f"[MnemoKernel] {request.journal_date} 日记{status_text}（第 {result.get('version', '?')} 版）\n"
            f"{proposal.title}\n{proposal.summary}"
        )

    def _schedule_auto_journal(self, event: Any) -> None:
        if (
            not self._cfg.diary.enabled
            or self._kernel is None
            or not self._kernel.available
            or not self._kernel.status.capabilities.episode_ready
        ):
            return
        try:
            timezone = ZoneInfo(self._cfg.diary.timezone)
            target_date = due_journal_date(
                datetime.now(timezone),
                self._cfg.diary.daily_hour,
                self._cfg.diary.daily_minute,
            )
            scope_key = self._local_scope_key(scope_from_event(event))
        except (ValueError, KeyError) as exc:
            logger.warning(f"[mnemokernel] 自动日记未调度：{exc}")
            return
        task_key = f"{scope_key}:{target_date.isoformat()}"
        if task_key in self._journal_attempted or task_key in self._journal_tasks:
            return
        task = asyncio.create_task(
            self._run_auto_journal(event, target_date.isoformat(), task_key),
            name=f"mnemokernel-journal-{target_date.isoformat()}",
        )
        self._journal_tasks[task_key] = task
        task.add_done_callback(
            lambda _completed, key=task_key: self._journal_tasks.pop(key, None)
        )

    async def _run_auto_journal(self, event: Any, date_text: str, task_key: str) -> None:
        self._journal_attempted.add(task_key)
        try:
            result = await self._summarize_journal(
                event,
                date_text,
                skip_existing=True,
                include_summary=False,
            )
            logger.info(f"[mnemokernel] 自动日记任务完成 date={date_text} result={result}")
        except asyncio.CancelledError:
            raise
        except Exception as exc:
            logger.warning(
                "[mnemokernel] 自动日记任务失败；普通聊天不受影响 "
                f"date={date_text} error={type(exc).__name__}"
            )

    async def _read_journal(self, event: Any, date_text: str = "") -> str:
        if not self._cfg.enabled or not self._cfg.diary.enabled:
            return "[MnemoKernel] 日记功能未启用。请在配置中打开 diary.enabled。"
        if (
            self._kernel is None
            or not self._kernel.available
            or not self._kernel.status.capabilities.episode_ready
        ):
            return "[MnemoKernel] 日记内核不可用。"
        request = self._journal_request(event, date_text)
        result = await asyncio.to_thread(self._kernel.read_journal, request)
        if result.get("status") != "found":
            return f"[MnemoKernel] {request.journal_date} 还没有日记。"
        lines = [
            f"[MnemoKernel] {request.journal_date} · {result.get('title', '')}",
            str(result.get("summary", "")),
        ]
        cues = [str(item) for item in result.get("cues", []) if str(item).strip()]
        open_loops = [str(item) for item in result.get("open_loops", []) if str(item).strip()]
        claims = result.get("claims", [])
        if cues:
            lines.append("线索：" + "；".join(cues))
        if open_loops:
            lines.append("未完事项：" + "；".join(open_loops))
        if claims:
            lines.append("记录：")
            lines.extend(
                f"- {claim.get('claim_text', '')}（{claim.get('epistemic_status', 'unknown')}）"
                for claim in claims[:8]
                if isinstance(claim, dict)
            )
        return "\n".join(lines)

    @filter.event_message_type(filter.EventMessageType.ALL, priority=5)
    async def capture_message(self, event: Any) -> None:
        if not self._cfg.enabled or not self._cfg.capture.enabled:
            return
        if (
            self._kernel is None
            or not self._kernel.available
            or not self._kernel.status.capabilities.capture_ready
        ):
            return
        try:
            raw = normalize_event(
                event, max_content_chars=self._cfg.capture.max_content_chars
            )
            if not self._cfg.capture.permits(raw.scope.kind.value, raw.scope.session_id):
                return
            result = await asyncio.to_thread(self._kernel.ingest_event, raw)
            self._capture_errors = 0
            if result.get("status") == "ok":
                self._schedule_auto_journal(event)
            if self._cfg.diagnostic_logging:
                logger.info(
                    "[mnemokernel] event accepted "
                    f"id={result.get('event_id', '?')} inserted={result.get('inserted', False)}"
                )
        except ValueError as exc:
            if self._cfg.diagnostic_logging:
                logger.warning(f"[mnemokernel] 事件未采集：{exc}")
        except Exception as exc:
            self._capture_errors += 1
            logger.warning(
                "[mnemokernel] 原始事件写入失败 "
                f"count={self._capture_errors} error={type(exc).__name__}: {exc}"
            )

    @filter.after_message_sent(priority=5)
    async def capture_sent_message(self, event: Any) -> None:
        if not self._cfg.enabled or not self._cfg.capture.enabled:
            return
        if (
            self._kernel is None
            or not self._kernel.available
            or not self._kernel.status.capabilities.capture_ready
        ):
            return
        try:
            raw = normalize_sent_event(
                event, max_content_chars=self._cfg.capture.max_content_chars
            )
            if not self._cfg.capture.permits(raw.scope.kind.value, raw.scope.session_id):
                return
            result = await asyncio.to_thread(self._kernel.ingest_event, raw)
            self._capture_errors = 0
            if self._cfg.diagnostic_logging:
                logger.info(
                    "[mnemokernel] sent event accepted "
                    f"id={result.get('event_id', '?')} inserted={result.get('inserted', False)}"
                )
        except ValueError as exc:
            if self._cfg.diagnostic_logging:
                logger.warning(f"[mnemokernel] 发送结果未采集：{exc}")
        except Exception as exc:
            self._capture_errors += 1
            logger.warning(
                "[mnemokernel] 发送结果写入失败 "
                f"count={self._capture_errors} error={type(exc).__name__}: {exc}"
            )

    def _register_recall_tool(self) -> None:
        """Expose recollect only after the native capability gate passes."""
        if self._recall_tool_registered:
            return
        try:
            from pydantic import Field
            from pydantic.dataclasses import dataclass

            from astrbot.core.agent.tool import FunctionTool
            from astrbot.core.astr_agent_context import AstrAgentContext

            plugin = self

            @dataclass
            class RecallTool(FunctionTool[AstrAgentContext]):
                name: str = "recollect"
                description: str = (
                    "按需申请一次与当前回答直接相关的个人历史记忆。仅当当前对话缺少"
                    "会实质改变答案的既往事实、偏好、决定、承诺、关系、事件或流程时调用；"
                    "不要用于常识、联网事实、重复读取当前上下文或试探性搜索。返回 not_found"
                    "后不要用同义词重复调用。"
                )
                parameters: dict[str, Any] = Field(
                    default_factory=lambda: {
                        "type": "object",
                        "properties": {
                            "need_type": {
                                "type": "string",
                                "enum": [
                                    "fact", "preference", "decision", "commitment",
                                    "relationship", "episode", "procedure",
                                ],
                            },
                            "reason": {
                                "type": "string",
                                "description": "缺少记忆为何会实质影响当前回答",
                            },
                            "cues": {
                                "type": "array",
                                "items": {"type": "string"},
                                "minItems": 1,
                                "maxItems": 8,
                                "description": "人物、主题、事件或原话等短线索",
                            },
                            "time_hint": {
                                "type": "string",
                                "description": "可选时间线索；没有就传空字符串",
                            },
                            "depth": {
                                "type": "string",
                                "enum": ["glance", "focused", "deep"],
                                "default": "glance",
                            },
                        },
                        "required": ["need_type", "reason", "cues"],
                    }
                )

                async def call(self, context: Any, **kwargs: Any) -> str:
                    event = getattr(getattr(context, "context", None), "event", None)
                    if event is None:
                        return "记忆申请缺少当前会话范围；请根据当前上下文回答。"
                    return await plugin._recollect_text(
                        event,
                        need_type=str(kwargs.get("need_type", "")),
                        reason=str(kwargs.get("reason", "")),
                        cues=kwargs.get("cues", []),
                        time_hint=str(kwargs.get("time_hint", "")),
                        depth=str(kwargs.get("depth", "glance")),
                    )

            # Keep the plugin instance out of the Pydantic dataclass.  AstrBot's
            # FunctionTool already has defaulted fields, so a required subclass
            # field would fail class creation with ``non-default argument ...``.
            # The nested method closes over this stable plugin instance instead.
            self.context.add_llm_tools(RecallTool())
            self._recall_tool_registered = True
            logger.info("[mnemokernel] 已按能力门控注册 recollect 工具")
        except Exception as exc:
            logger.warning(
                "[mnemokernel] recollect 工具注册失败，已保持普通对话可用："
                f"{type(exc).__name__}: {exc}"
            )

    async def _recollect_text(
        self,
        event: Any,
        need_type: str,
        reason: str,
        cues: list[str],
        time_hint: str = "",
        depth: str = "glance",
    ) -> str:
        """仅当回答依赖当前对话中没有提供的既往经历时，申请一次回忆。

        不要用它搜索常识、互联网事实或重复读取当前对话。信息已在当前上下文中、
        不确定性不影响回答、或可以直接向用户澄清时，不应调用。返回 not_found 后
        不要用同义词反复调用。

        Args:
            need_type(string): 所需记忆类型，只能是 fact、preference、decision、commitment、relationship、episode、procedure。
            reason(string): 说明缺少这段记忆为何会实质影响当前回答。
            cues(array[string]): 1 到 8 个短线索，使用人物、主题、事件或用户原话，不要写完整问题。
            time_hint(string): 可选时间线索；没有就传空字符串。
            depth(string): 回忆深度，只能是 glance、focused、deep；默认 glance。
        """
        if not self._cfg.enabled or not self._cfg.recall.enabled:
            return (
                "记忆申请被拒绝：MnemoKernel 当前已停用。请仅根据当前对话回答；"
                "如信息不足，应直接向用户澄清。"
            )
        if (
            self._kernel is None
            or not self._kernel.available
            or not self._kernel.status.capabilities.recall_ready
        ):
            return (
                "记忆当前不可用：可信原生内核未就绪。不要假装记得，也不要重复调用；"
                "请根据当前对话回答或向用户澄清。"
            )

        try:
            request = RecallRequest.build(
                scope=scope_from_event(event),
                need_type=need_type,
                reason=reason,
                cues=cues,
                time_hint=time_hint,
                depth=depth,
                max_cues=self._cfg.recall.max_cues,
                max_reason_chars=self._cfg.recall.max_reason_chars,
            )
            result = await asyncio.to_thread(self._kernel.request_recall, request)
        except (ValueError, KernelUnavailableError) as exc:
            return (
                f"记忆申请无效或不可用：{exc}。请修正一次；不要循环调用。"
            )
        except Exception as exc:
            logger.warning(f"[mnemokernel] 回忆申请失败：{type(exc).__name__}: {exc}")
            return (
                "记忆申请处理失败。不要假装记得，也不要重复调用；请向用户澄清。"
            )

        status = str(result.get("status", "error"))
        if status == "not_found":
            return (
                "没有找到满足当前范围和证据要求的记忆。不要用近义线索重复调用；"
                "请坦诚说明不知道，或向用户澄清。"
            )
        if status == "declined":
            reason_text = str(result.get("reason", "申请不满足准入规则"))
            return (
                f"记忆申请被拒绝：{reason_text}。请使用当前上下文继续。"
            )

        # Only a native evidence brief may cross this boundary. The Python shell
        # does not format database rows or expose raw events itself.
        brief = result.get("brief")
        if status == "ok" and isinstance(brief, str) and brief.strip():
            return brief
        return "记忆内核返回了不可用结果。不要依据该结果推断任何既往事实。"

    async def recollect(
        self,
        event: Any,
        need_type: str,
        reason: str,
        cues: list[str],
        time_hint: str = "",
        depth: str = "glance",
    ):
        """兼容手动调用；模型工具通过动态 FunctionTool 使用同一闭环。"""
        yield event.plain_result(
            await self._recollect_text(
                event,
                need_type=need_type,
                reason=reason,
                cues=cues,
                time_hint=time_hint,
                depth=depth,
            )
        )

    async def _scope_control(self, event: Any, action: CapturePolicyAction) -> str:
        if self._kernel is None or not self._kernel.available:
            return "[MnemoKernel] 原生内核不可用，未修改作用域状态。"
        try:
            scope, actor_id = self._control_identity(event)
        except PermissionError as exc:
            return f"[MnemoKernel] 拒绝操作：{exc}"
        request = CapturePolicyRequest(
            scope=scope,
            action=action,
            actor_id=actor_id,
            reason=f"user requested {action.value}",
        )
        result = await asyncio.to_thread(self._kernel.set_capture_policy, request)
        if result.get("status") == "blocked":
            return "[MnemoKernel] 当前作用域曾执行过 forget，隐私策略已锁定，未修改；请使用新的会话作用域。"
        state = "暂停采集" if action is CapturePolicyAction.PAUSE else "恢复采集"
        return f"[MnemoKernel] 已{state}当前作用域；状态：{result.get('policy_state', 'unknown')}。"

    async def _forget_scope(self, event: Any, confirmation_text: str = "") -> str:
        if self._kernel is None or not self._kernel.available:
            return "[MnemoKernel] 原生内核不可用，未执行遗忘。"
        try:
            scope, actor_id = self._control_identity(event)
        except PermissionError as exc:
            return f"[MnemoKernel] 拒绝操作：{exc}"
        confirmation_key = f"{self._local_scope_key(scope)}:{actor_id}"
        now = datetime.now().timestamp()
        confirmation = confirmation_text.strip().lower()
        expires_at = self._forget_confirmations.get(confirmation_key, 0.0)
        if confirmation != "confirm" or expires_at < now:
            self._forget_confirmations[confirmation_key] = now + 60.0
            return (
                "[MnemoKernel] 这是破坏性操作：会清除当前作用域的原始正文、日记、"
                "稳定记忆和回忆审计，并暂停后续采集。确认请在 60 秒内发送 "
                "/mnemo forget confirm。"
            )
        self._forget_confirmations.pop(confirmation_key, None)
        result = await asyncio.to_thread(
            self._kernel.purge_scope,
            PurgeScopeRequest(
                scope=scope,
                actor_id=actor_id,
                reason="user confirmed forget current scope",
            ),
        )
        return (
            "[MnemoKernel] 当前作用域已遗忘："
            f"清除正文 {result.get('purged_payloads', 0)} 条，"
            f"日记/episode {result.get('purged_episodes', 0)} 个；后续采集已暂停。"
        )

    async def _scope_stats(self, event: Any) -> str:
        if self._kernel is None or not self._kernel.available:
            return "[MnemoKernel] 原生内核不可用，无法读取统计。"
        result = await asyncio.to_thread(
            self._kernel.scope_stats,
            ScopeStatsRequest(scope=scope_from_event(event)),
        )
        return (
            "[MnemoKernel] 当前作用域统计："
            f"采集策略={result.get('policy_state', 'unknown')}，"
            f"事件={result.get('raw_events', 0)}，活跃正文={result.get('active_payloads', 0)}，"
            f"日记={result.get('episodes', 0)}，"
            f"活跃记忆={result.get('active_memories', 0)}，"
            f"已替代={result.get('superseded_memories', 0)}，"
            f"已归档={result.get('archived_memories', 0)}。"
        )

    async def _maintain_scope(self, event: Any) -> str:
        if self._kernel is None or not self._kernel.available:
            return "[MnemoKernel] 原生内核不可用，未执行保留期清理。"
        try:
            scope, actor_id = self._control_identity(event)
        except PermissionError as exc:
            return f"[MnemoKernel] 拒绝操作：{exc}"
        timezone = _configured_timezone(self._cfg.diary.timezone)
        cutoff = datetime.now(timezone) - timedelta(days=self._cfg.retention_days)
        result = await asyncio.to_thread(
            self._kernel.retain_payloads,
            RetentionRequest(
                scope=scope,
                cutoff_at_ms=int(cutoff.timestamp() * 1000),
                actor_id=actor_id,
                reason=f"retention policy {self._cfg.retention_days} days",
            ),
        )
        return (
            "[MnemoKernel] 保留期清理完成："
            f"清除正文 {result.get('purged_payloads', 0)} 条，"
            f"激活度衰减 {result.get('decayed_memories', 0)} 张，"
            f"截止时间 {result.get('cutoff_at_ms')}，扫描={result.get('residual_scan', 'unknown')}。"
        )

    @filter.command("mnemo")
    async def mnemo_status(
        self, event: Any, subcommand: str = "status", date_text: str = ""
    ):
        subcommand = (subcommand or "status").strip().lower()
        if subcommand in {"pause", "resume"}:
            try:
                action = (
                    CapturePolicyAction.PAUSE
                    if subcommand == "pause"
                    else CapturePolicyAction.RESUME
                )
                yield event.plain_result(await self._scope_control(event, action))
            except Exception as exc:
                logger.warning(f"[mnemokernel] 作用域策略更新失败：{type(exc).__name__}: {exc}")
                yield event.plain_result("[MnemoKernel] 作用域策略更新失败，未修改数据。")
            return
        if subcommand == "forget":
            try:
                yield event.plain_result(await self._forget_scope(event, date_text))
            except Exception as exc:
                logger.warning(f"[mnemokernel] 作用域遗忘失败：{type(exc).__name__}: {exc}")
                yield event.plain_result("[MnemoKernel] 遗忘失败，未完成清除。")
            return
        if subcommand in {"stats", "statistics"}:
            try:
                yield event.plain_result(await self._scope_stats(event))
            except Exception as exc:
                logger.warning(f"[mnemokernel] 统计读取失败：{type(exc).__name__}: {exc}")
                yield event.plain_result("[MnemoKernel] 统计读取失败。")
            return
        if subcommand in {"maintain", "retention"}:
            try:
                yield event.plain_result(await self._maintain_scope(event))
            except Exception as exc:
                logger.warning(f"[mnemokernel] 保留期清理失败：{type(exc).__name__}: {exc}")
                yield event.plain_result("[MnemoKernel] 保留期清理失败，未完成清除。")
            return
        if subcommand == "summarize":
            try:
                yield event.plain_result(await self._summarize_journal(event, date_text))
            except Exception as exc:
                logger.warning(f"[mnemokernel] 日记总结失败：{type(exc).__name__}: {exc}")
                yield event.plain_result(
                    "[MnemoKernel] 日记总结失败，没有写入任何内容；请检查模型调用和事件日志。"
                )
            return
        if subcommand == "today":
            try:
                yield event.plain_result(await self._read_journal(event, date_text))
            except Exception as exc:
                logger.warning(f"[mnemokernel] 日记读取失败：{type(exc).__name__}: {exc}")
                yield event.plain_result("[MnemoKernel] 日记读取失败。")
            return
        if subcommand not in {"status", "doctor"}:
            yield event.plain_result(
                "[MnemoKernel] /mnemo status|doctor|pause|resume|forget|stats|maintain|"
                "summarize [YYYY-MM-DD]|today [YYYY-MM-DD]"
            )
            return
        if not self._cfg.enabled:
            yield event.plain_result(f"[MnemoKernel] {VERSION}\n  状态：已由配置停用")
            return
        if self._kernel is None or not self._kernel.available:
            detail = "请查看 AstrBot 服务端日志获取模块、ABI 或数据库初始化详情"
            yield event.plain_result(
                f"[MnemoKernel] {VERSION}\n"
                "  原生内核：不可用（记忆读写已安全停用）\n"
                f"  采集开关：{'开' if self._cfg.capture.enabled else '关'}\n"
                f"  诊断：{detail}"
            )
            return
        health = await asyncio.to_thread(self._kernel.health)
        capabilities = self._kernel.status.capabilities
        yield event.plain_result(
            f"[MnemoKernel] {VERSION}\n"
            f"  原生内核：{health.get('status', 'unknown')}\n"
            f"  协议：{health.get('protocol', 'unknown')}\n"
            f"  数据库 Schema：{health.get('schema_version', 'unknown')}\n"
            "  能力："
            f"capture={capabilities.capture_ready} "
            f"episode={capabilities.episode_ready} recall={capabilities.recall_ready}\n"
            f"  采集开关：{'开' if self._cfg.capture.enabled else '关'}"
        )
