import json
import unittest
from pathlib import Path
from types import SimpleNamespace

from mnemokernel_adapter.config import MnemoKernelConfig
from mnemokernel_adapter.kernel import KernelClient, KernelStatus, KernelUnavailableError
from mnemokernel_adapter.pages import InspectorPage, parse_query
from scripts.package_release import source_files


class PageTests(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        self.calls = []
        def inspect_json(payload):
            self.calls.append(json.loads(payload))
            return '{"items":[],"total":0}'
        self.kernel = KernelClient(SimpleNamespace(inspect_json=inspect_json), KernelStatus(True, "ready"))
        self.web = SimpleNamespace(
            request=SimpleNamespace(username="admin", query={}),
            json_response=lambda data, **kwargs: (data, kwargs),
        )
        self.plugin = SimpleNamespace(_kernel=self.kernel, _cfg=MnemoKernelConfig(),
                                      _database_path=Path("test.sqlite3"), _capture_errors=0)
        self.page = InspectorPage(self.plugin, self.web)

    async def test_anonymous_requests_never_reach_database(self):
        self.web.request.username = None
        for handler in (self.page.status, self.page.browse):
            _, response = await handler()
            self.assertEqual(response["status_code"], 401)
        self.assertEqual(self.calls, [])

    async def test_authenticated_browse_is_uncached_and_bounded(self):
        result, response = await self.page.browse()
        self.assertEqual(result["items"], [])
        self.assertEqual(self.calls[0]["limit"], 20)
        self.assertEqual(response["headers"]["Cache-Control"], "no-store")

    async def test_invalid_request_does_not_call_native(self):
        self.web.request.query = {"collection": "sqlite_master"}
        _, response = await self.page.browse()
        self.assertEqual(response["status_code"], 400)
        self.assertEqual(self.calls, [])

    async def test_older_native_and_closed_kernel_are_explicit(self):
        self.kernel._native = SimpleNamespace()
        status, _ = await self.page.status()
        self.assertFalse(status["inspector_available"])
        _, response = await self.page.browse()
        self.assertEqual(response["status_code"], 503)
        self.kernel.close()
        with self.assertRaises(KernelUnavailableError):
            self.kernel.inspect({})

    async def test_internal_errors_do_not_disclose_content(self):
        def broken(_):
            raise RuntimeError("private database data")
        self.kernel._native.inspect_json = broken
        data, response = await self.page.browse()
        self.assertEqual(response["status_code"], 500)
        self.assertNotIn("private", str(data))

    def test_query_validation(self):
        for values in ({"limit":"bad"}, {"limit":"51"}, {"offset":"-1"},
                       {"query":"x"*101}, {"collection":"events"},
                       {"collection":"events", "scope_id":"' OR 1=1--"}):
            with self.subTest(values=values), self.assertRaises(ValueError):
                parse_query(values)
        self.assertEqual(parse_query({"collection":"events", "scope_id":"a"*64})["scope_id"], "a"*64)

    def test_release_includes_page_assets(self):
        names = {str(name) for name, _ in source_files()}
        for name in ("pages/memory/index.html", "pages/memory/app.js", "pages/memory/style.css",
                     ".astrbot-plugin/i18n/zh-CN.json", "mnemokernel_adapter/pages.py"):
            self.assertIn(name, names)
        self.assertNotIn("requirements.txt", names)
