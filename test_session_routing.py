"""Routing and management tests for proxy-owned WorkBuddy sessions."""

from pathlib import Path
import tempfile
import unittest
from unittest.mock import AsyncMock, patch

import httpx

import proxy_server
from session_store import SessionStore
from sidecar_manager import SidecarManager
from upstream_transport import WorkBuddyAcpError


class ProxySessionRoutingTests(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        root = Path(self.temporary.name)
        self.project_one = root / "project-one"
        self.project_two = root / "project-two"
        self.project_one.mkdir()
        self.project_two.mkdir()
        self.store = SessionStore(root / "sessions.json", max_history_turns=2)
        self.manager = SidecarManager(
            self.store,
            "/missing/codebuddy",
            root / "runtime",
            idle_timeout=0,
        )

    def tearDown(self):
        self.temporary.cleanup()

    async def call_app(self, method: str, path: str, **kwargs) -> httpx.Response:
        transport = httpx.ASGITransport(app=proxy_server.app)
        async with httpx.AsyncClient(
            transport=transport,
            base_url="http://127.0.0.1",
        ) as client:
            return await client.request(method, path, **kwargs)

    async def test_management_routes_create_list_history_get_and_delete(self):
        with (
            patch.object(proxy_server, "session_store", self.store),
            patch.object(proxy_server, "sidecar_manager", self.manager),
        ):
            created = await self.call_app(
                "POST",
                "/proxy/sessions",
                json={"project": str(self.project_one), "title": "Terminal work"},
            )
            self.assertEqual(created.status_code, 201, created.text)
            session = created.json()

            listed = await self.call_app(
                "GET",
                "/proxy/sessions",
                params={"project": str(self.project_one)},
            )
            self.assertEqual([item["id"] for item in listed.json()["data"]], [session["id"]])

            saved = await self.call_app(
                "POST",
                f"/proxy/sessions/{session['id']}/history",
                json={
                    "messages": [
                        {"role": "user", "content": "hello"},
                        {"role": "assistant", "content": "isolated"},
                    ]
                },
            )
            self.assertEqual(saved.status_code, 200, saved.text)

            loaded = await self.call_app("GET", f"/proxy/sessions/{session['id']}")
            self.assertEqual(
                [message["content"] for message in loaded.json()["history"]],
                ["hello", "isolated"],
            )

            deleted = await self.call_app("DELETE", f"/proxy/sessions/{session['id']}")
            self.assertEqual(deleted.status_code, 200, deleted.text)
            self.assertTrue(deleted.json()["deleted"])
            missing = await self.call_app("GET", f"/proxy/sessions/{session['id']}")
            self.assertEqual(missing.status_code, 404)

    async def test_management_routes_reject_non_object_and_invalid_history(self):
        with (
            patch.object(proxy_server, "session_store", self.store),
            patch.object(proxy_server, "sidecar_manager", self.manager),
        ):
            invalid_create = await self.call_app("POST", "/proxy/sessions", json=[])
            self.assertEqual(invalid_create.status_code, 400)

            session = await self.store.create(str(self.project_one), "Validation")
            invalid_history = await self.call_app(
                "POST",
                f"/proxy/sessions/{session['id']}/history",
                json={"messages": ["not-a-message"]},
            )
            self.assertEqual(invalid_history.status_code, 400)
            self.assertIn("must be an object", invalid_history.text)

    async def test_explicit_session_routes_to_its_sidecar_and_echoes_header(self):
        session = await self.store.create(str(self.project_one), "Codex session")
        ensure = AsyncMock(return_value="http://127.0.0.1:45123")
        complete = AsyncMock(return_value="isolated reply")
        self.manager.ensure = ensure

        with (
            patch.object(proxy_server, "session_store", self.store),
            patch.object(proxy_server, "sidecar_manager", self.manager),
            patch.object(proxy_server.config, "TRANSPORT", "workbuddy_acp"),
            patch.object(proxy_server, "complete_workbuddy_chat", complete),
        ):
            response = await self.call_app(
                "POST",
                "/v1/chat/completions",
                headers={
                    "X-WorkBuddy-Session": session["id"],
                    "X-WorkBuddy-Project": str(self.project_one),
                },
                json={"model": "gpt-5.6-sol", "messages": [{"role": "user", "content": "hello"}]},
            )

        self.assertEqual(response.status_code, 200, response.text)
        self.assertEqual(response.headers["x-workbuddy-session"], session["id"])
        ensure.assert_awaited_once()
        self.assertEqual(ensure.await_args.args[0]["id"], session["id"])
        complete.assert_awaited_once_with(
            [{"role": "user", "content": "hello"}],
            gateway_url="http://127.0.0.1:45123",
            project=str(self.project_one.resolve()),
        )

    async def test_automatic_session_is_stable_and_project_scoped(self):
        ensure = AsyncMock(return_value="http://127.0.0.1:45124")
        self.manager.ensure = ensure
        messages = [{"role": "user", "content": "stable initial prompt"}]

        with (
            patch.object(proxy_server, "session_store", self.store),
            patch.object(proxy_server, "sidecar_manager", self.manager),
        ):
            first_request = httpx.Request(
                "POST",
                "http://test/v1/chat/completions",
                headers={"X-WorkBuddy-Project": str(self.project_one)},
            )
            first, _ = await proxy_server.resolve_proxy_session(first_request, messages)
            second, _ = await proxy_server.resolve_proxy_session(
                first_request,
                messages
                + [
                    {"role": "assistant", "content": "answer"},
                    {"role": "user", "content": "follow-up"},
                ],
            )
            other_request = httpx.Request(
                "POST",
                "http://test/v1/chat/completions",
                headers={"X-WorkBuddy-Project": str(self.project_two)},
            )
            other, _ = await proxy_server.resolve_proxy_session(other_request, messages)

        self.assertEqual(first["id"], second["id"])
        self.assertNotEqual(first["id"], other["id"])
        self.assertEqual(len(await self.store.list()), 2)

    async def test_unknown_explicit_session_returns_404_without_starting_sidecar(self):
        ensure = AsyncMock(return_value="http://127.0.0.1:45125")
        self.manager.ensure = ensure

        with (
            patch.object(proxy_server, "session_store", self.store),
            patch.object(proxy_server, "sidecar_manager", self.manager),
            patch.object(proxy_server.config, "TRANSPORT", "workbuddy_acp"),
        ):
            response = await self.call_app(
                "POST",
                "/v1/chat/completions",
                headers={
                    "X-WorkBuddy-Session": "proxy-missing-session",
                    "X-WorkBuddy-Project": str(self.project_one),
                },
                json={"model": "gpt-5.6-sol", "messages": [{"role": "user", "content": "hello"}]},
            )

        self.assertEqual(response.status_code, 404, response.text)
        self.assertIn("Unknown proxy session", response.text)
        ensure.assert_not_awaited()

    async def test_project_mismatch_returns_409_without_starting_sidecar(self):
        session = await self.store.create(str(self.project_one), "Project one")
        ensure = AsyncMock(return_value="http://127.0.0.1:45126")
        self.manager.ensure = ensure

        with (
            patch.object(proxy_server, "session_store", self.store),
            patch.object(proxy_server, "sidecar_manager", self.manager),
            patch.object(proxy_server.config, "TRANSPORT", "workbuddy_acp"),
        ):
            response = await self.call_app(
                "POST",
                "/v1/chat/completions",
                headers={
                    "X-WorkBuddy-Session": session["id"],
                    "X-WorkBuddy-Project": str(self.project_two),
                },
                json={"model": "gpt-5.6-sol", "messages": [{"role": "user", "content": "hello"}]},
            )

        self.assertEqual(response.status_code, 409, response.text)
        self.assertIn("different project", response.text)
        ensure.assert_not_awaited()


if __name__ == "__main__":
    unittest.main()
