"""Authenticated, read-only AstrBot Plugin Pages endpoints (no SQLite fallback)."""
from __future__ import annotations

import asyncio
from typing import Any

from .kernel import KernelUnavailableError


def parse_query(query: Any) -> dict[str, Any]:
    collection = query.get("collection", "scopes")
    scope_id = query.get("scope_id", "")
    text = query.get("query", "")
    try:
        limit = int(query.get("limit", "20"))
        offset = int(query.get("offset", "0"))
    except (ValueError, TypeError):
        raise ValueError("分页参数必须是整数") from None
    if collection not in {"scopes", "memories", "journals", "events", "recalls"}:
        raise ValueError("不支持的数据分类")
    if not 1 <= limit <= 50 or not 0 <= offset <= 100_000:
        raise ValueError("每页应为 1–50 条，偏移量应为 0–100000")
    if not isinstance(text, str) or len(text) > 100:
        raise ValueError("搜索词最多 100 字")
    if collection != "scopes" and (
        not isinstance(scope_id, str) or len(scope_id) != 64
        or any(c not in "0123456789abcdefABCDEF" for c in scope_id)
    ):
        raise ValueError("请先选择有效会话")
    return dict(collection=collection, scope_id=scope_id, query=text, limit=limit, offset=offset)


class InspectorPage:
    def __init__(self, plugin: Any, web: Any):
        self.plugin = plugin
        self.web = web

    def reply(self, data: Any, status: int = 200):
        return self.web.json_response(data, status_code=status, headers={"Cache-Control": "no-store"})

    def error(self, message: str, status: int):
        return self.reply({"status": "error", "message": message}, status)

    def authenticated(self) -> bool:
        # Authentication is supplied by the host, never by query/body fields.
        return bool(self.web.request.username)

    async def status(self):
        if not self.authenticated():
            return self.error("请先登录 AstrBot 管理面板", 401)
        plugin = self.plugin
        kernel = plugin._kernel
        return self.reply({
            "enabled": plugin._cfg.enabled,
            "available": bool(kernel and kernel.available),
            "inspector_available": bool(kernel and kernel.inspector_available),
            "database_path": str(plugin._database_path) if plugin._database_path else None,
            "capture_enabled": plugin._cfg.capture.enabled,
            "diary_enabled": plugin._cfg.diary.enabled,
            "recall_enabled": plugin._cfg.recall.enabled,
            "capture_errors": plugin._capture_errors,
        })

    async def browse(self):
        if not self.authenticated():
            return self.error("请先登录 AstrBot 管理面板", 401)
        try:
            payload = parse_query(self.web.request.query)
            kernel = self.plugin._kernel
            if not kernel or not kernel.available:
                return self.error("原生内核未就绪，请查看插件日志和平台安装包", 503)
            return self.reply(await asyncio.to_thread(kernel.inspect, payload))
        except ValueError as exc:
            return self.error(str(exc), 400)
        except KernelUnavailableError:
            return self.error("数据库浏览不可用，请安装匹配平台的新版本原生内核", 503)
        except Exception:
            # Do not disclose SQL, filesystem or message data in error responses.
            return self.error("读取失败，请刷新或检查原生内核状态", 500)


def register_pages(context: Any, plugin: Any) -> InspectorPage | None:
    try:
        from astrbot.api import web
    except ImportError:
        return None  # Older AstrBot versions retain chat-command functionality.
    if not callable(getattr(context, "register_web_api", None)):
        return None
    page = InspectorPage(plugin, web)
    for endpoint, handler in (("status", page.status), ("browse", page.browse)):
        context.register_web_api(
            f"/astrbot_plugin_mnemokernel/inspector/{endpoint}", handler,
            ["GET"], "MnemoKernel 管理员只读数据浏览",
        )
    return page
