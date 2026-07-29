"""Tests for the official WorkBuddy ACP transport."""

import asyncio
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import httpx

import upstream_transport
from upstream_transport import (
    NormalizedEvent,
    WorkBuddyAcpError,
    WorkBuddyAcpTransport,
    serialize_messages,
)


class WorkBuddyAcpTransportTests(unittest.TestCase):
    def test_serializes_complete_history(self):
        prompt = serialize_messages(
            [
                {"role": "system", "content": "Be exact."},
                {"role": "user", "content": "Call a tool."},
                {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [
                        {
                            "id": "call_1",
                            "type": "function",
                            "function": {"name": "lookup", "arguments": "{}"},
                        }
                    ],
                },
                {"role": "tool", "tool_call_id": "call_1", "content": "done"},
            ]
        )
        self.assertNotIn("SYSTEM:\nBe exact.", prompt)
        self.assertIn("TOOL_CALLS:", prompt)
        self.assertIn("TOOL[call_1]:\ndone", prompt)
        self.assertTrue(prompt.endswith("ASSISTANT:"))

    def test_sse_parser_accepts_message_events(self):
        class Response:
            async def aiter_lines(self):
                yield ":ok"
                yield "event: message"
                yield 'data: {"jsonrpc":"2.0","id":1,"result":{}}'
                yield ""

        async def run():
            result = []
            async for event in WorkBuddyAcpTransport._read_sse_json(Response()):
                result.append(event)
            return result

        import asyncio

        self.assertEqual(asyncio.run(run())[0]["id"], 1)

    def test_normalized_event_does_not_contain_secrets(self):
        event = NormalizedEvent("text_delta", text="hello")
        self.assertEqual(event.text, "hello")
        self.assertNotIn("password", event.__dict__)

    def test_discovery_skips_stale_sessions(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            sessions = Path(temp_dir) / "sessions"
            sessions.mkdir()
            (sessions / "stale.json").write_text(
                json.dumps({"pid": 999999999, "url": "http://127.0.0.1:11111"}),
                encoding="utf-8",
            )
            (sessions / "active.json").write_text(
                json.dumps({"pid": os.getpid(), "url": "http://127.0.0.1:22222"}),
                encoding="utf-8",
            )
            with patch.dict(os.environ, {"WORKBUDDY_ACP_PASSWORD": "local-secret"}):
                self.assertEqual(
                    WorkBuddyAcpTransport.discover(Path(temp_dir)),
                    ("http://127.0.0.1:22222", "local-secret"),
                )

    def test_from_config_prefers_active_discovered_gateway(self):
        class Config:
            WORKBUDDY_ACP_URL = "http://127.0.0.1:11111"
            WORKBUDDY_ACP_PASSWORD = "configured-secret"
            WORKBUDDY_ACP_CWD = "/tmp"
            WORKBUDDY_ACP_TIMEOUT = 30

        with patch.object(
            WorkBuddyAcpTransport,
            "discover_all",
            return_value=["http://127.0.0.1:22222"],
        ):
            transport = WorkBuddyAcpTransport.from_config(Config)
        self.assertEqual(transport.base_url, "http://127.0.0.1:22222")
        self.assertEqual(transport.password, "configured-secret")

    def test_http_error_classification(self):
        auth = WorkBuddyAcpError.from_http_status("connection", 403)
        capacity = WorkBuddyAcpError.from_http_status("connection", 503)
        self.assertEqual(auth.category, "authentication")
        self.assertFalse(auth.retryable)
        self.assertTrue(capacity.retryable)
        self.assertEqual(capacity.status_code, 503)

    def test_discover_all_deduplicates_live_urls(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            sessions = Path(temp_dir) / "sessions"
            sessions.mkdir()
            for name in ("one.json", "two.json"):
                (sessions / name).write_text(
                    json.dumps({"pid": os.getpid(), "url": "http://127.0.0.1:22222"}),
                    encoding="utf-8",
                )
            self.assertEqual(
                WorkBuddyAcpTransport.discover_all(Path(temp_dir)),
                ["http://127.0.0.1:22222"],
            )

    def test_sse_parser_rejects_malformed_json(self):
        class Response:
            async def aiter_lines(self):
                yield "data: not-json"

        async def run():
            async for _ in WorkBuddyAcpTransport._read_sse_json(Response()):
                pass

        with self.assertRaises(WorkBuddyAcpError) as caught:
            asyncio.run(run())
        self.assertEqual(caught.exception.category, "protocol")

    def test_cancelled_prompt_sends_cancel_then_closes_connection(self):
        transport = WorkBuddyAcpTransport("http://test", "", "/tmp")
        calls = []
        prompt_started = asyncio.Event()

        class FakeClient:
            def __init__(self, *args, **kwargs):
                pass

            async def __aenter__(self):
                return self

            async def __aexit__(self, exc_type, exc, traceback):
                return False

            def stream(self, method, url, **kwargs):
                class GetResponse:
                    status_code = 200
                    headers = {
                        "acp-connection-id": "connection",
                        "acp-session-token": "token",
                    }

                    async def __aenter__(self):
                        return self

                    async def __aexit__(self, exc_type, exc, traceback):
                        return False

                return GetResponse()

        async def fake_rpc_stream(
            client,
            connection_id,
            session_token,
            request_id,
            method,
            params,
        ):
            calls.append(method)
            if method == "initialize":
                yield {"id": request_id, "result": {}}
            elif method == "session/new":
                yield {"id": request_id, "result": {"sessionId": "session"}}
            elif method == "session/prompt":
                prompt_started.set()
                await asyncio.Future()
            elif method == "session/cancel":
                yield {"id": request_id, "result": {}}

        async def fake_close(client, connection_id, session_token):
            calls.append("DELETE")

        async def run():
            with (
                patch.object(upstream_transport.httpx, "AsyncClient", FakeClient),
                patch.object(transport, "_rpc_stream", side_effect=fake_rpc_stream),
                patch.object(transport, "_close_connection", side_effect=fake_close),
            ):
                task = asyncio.create_task(transport.complete_chat([{"role": "user", "content": "wait"}]))
                await prompt_started.wait()
                task.cancel()
                with self.assertRaises(asyncio.CancelledError):
                    await task

        asyncio.run(run())
        self.assertIn("session/cancel", calls)
        self.assertEqual(calls[-1], "DELETE")
        self.assertLess(calls.index("session/cancel"), calls.index("DELETE"))

    def test_rpc_stream_requires_matching_result(self):
        transport = WorkBuddyAcpTransport("http://test", "", "/tmp")

        async def handler(request):
            return httpx.Response(
                200,
                content=b'data: {"jsonrpc":"2.0","id":99,"result":{}}\n\n',
                headers={"content-type": "text/event-stream"},
            )

        async def run():
            async with httpx.AsyncClient(transport=httpx.MockTransport(handler)) as client:
                async for _ in transport._rpc_stream(client, "connection", "token", 1, "initialize", {}):
                    pass

        with self.assertRaises(WorkBuddyAcpError) as caught:
            asyncio.run(run())
        self.assertIn("without a result", str(caught.exception))


if __name__ == "__main__":
    unittest.main()
