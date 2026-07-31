"""Streaming and routing regression tests for Chat Completions."""

import asyncio
import json
import unittest
from unittest.mock import patch

import httpx

import proxy_server
from upstream_transport import NormalizedEvent, WorkBuddyAcpError


class ChatStreamingTests(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        self.original_async_client = httpx.AsyncClient

    def client_factory(self, handler):
        transport = httpx.MockTransport(handler)
        original_async_client = self.original_async_client

        def factory(*args, **kwargs):
            kwargs["transport"] = transport
            return original_async_client(*args, **kwargs)

        return factory

    async def call_app(self, payload, handler, transport="http"):
        app_transport = httpx.ASGITransport(app=proxy_server.app)
        async with self.original_async_client(
            transport=app_transport,
            base_url="http://testserver",
        ) as client:
            with (
                patch.object(proxy_server.config, "TRANSPORT", transport),
                patch.object(proxy_server.config, "DEFAULT_BASE_URL", "https://generic.example/v1"),
                patch.object(proxy_server.httpx, "AsyncClient", self.client_factory(handler)),
            ):
                return await client.post("/v1/chat/completions", json=payload)

    async def test_http_stream_preserves_preflight_status(self):
        async def handler(request):
            return httpx.Response(503, json={"error": {"message": "busy", "type": "upstream_error"}})

        response = await self.call_app(
            {"model": "gpt-5.6-sol", "messages": [{"role": "user", "content": "hi"}], "stream": True},
            handler,
        )
        self.assertEqual(response.status_code, 503)
        self.assertEqual(response.json()["error"]["message"], "busy")

    async def test_http_stream_rejects_done_without_finish_reason(self):
        async def handler(request):
            return httpx.Response(
                200,
                content=b'data: {"choices":[{"delta":{"content":"partial"},"finish_reason":null}]}\n\ndata: [DONE]\n\n',
                headers={"content-type": "text/event-stream"},
            )

        response = await self.call_app(
            {"model": "gpt-5.6-sol", "messages": [{"role": "user", "content": "hi"}], "stream": True},
            handler,
        )
        self.assertEqual(response.status_code, 200)
        lines = [line for line in response.text.splitlines() if line.startswith("data:")]
        final = json.loads(lines[-1][5:].strip())
        self.assertEqual(final["error"]["code"], "upstream_stream_incomplete")
        self.assertNotIn("[DONE]", response.text)

    async def test_http_stream_rejects_malformed_sse(self):
        async def handler(request):
            return httpx.Response(
                200,
                content=b"data: not-json\n\n",
                headers={"content-type": "text/event-stream"},
            )

        response = await self.call_app(
            {"model": "gpt-5.6-sol", "messages": [{"role": "user", "content": "hi"}], "stream": True},
            handler,
        )
        payload = json.loads(response.text.split("data: ", 1)[1].strip())
        self.assertEqual(payload["error"]["code"], "malformed_upstream_sse")

    async def test_acp_requests_are_serialized_per_gateway(self):
        class FakeTransport:
            base_url = "http://gateway"
            active = 0
            max_active = 0

            async def stream_chat(self, messages):
                FakeTransport.active += 1
                FakeTransport.max_active = max(
                    FakeTransport.max_active,
                    FakeTransport.active,
                )
                try:
                    await asyncio.sleep(0.01)
                    yield NormalizedEvent("text_delta", text=messages[0]["content"])
                    yield NormalizedEvent("completed")
                finally:
                    FakeTransport.active -= 1

        proxy_server._workbuddy_gateway_locks.clear()
        with (
            patch.object(
                proxy_server.WorkBuddyAcpTransport,
                "candidate_urls",
                return_value=["http://gateway"],
            ),
            patch.object(
                proxy_server,
                "workbuddy_transport",
                return_value=FakeTransport(),
            ),
            patch.object(
                proxy_server.config,
                "WORKBUDDY_ACP_MAX_ATTEMPTS",
                1,
            ),
        ):
            results = await asyncio.gather(
                *(
                    proxy_server.complete_workbuddy_chat(
                        [{"role": "user", "content": value}]
                    )
                    for value in ("one", "two", "three")
                )
            )

        self.assertEqual(results, ["one", "two", "three"])
        self.assertEqual(FakeTransport.max_active, 1)

    async def test_acp_requests_on_distinct_sidecars_can_overlap(self):
        class FakeTransport:
            active = 0
            max_active = 0

            def __init__(self, base_url):
                self.base_url = base_url

            async def stream_chat(self, messages):
                FakeTransport.active += 1
                FakeTransport.max_active = max(FakeTransport.max_active, FakeTransport.active)
                try:
                    await asyncio.sleep(0.01)
                    yield NormalizedEvent("text_delta", text=messages[0]["content"])
                    yield NormalizedEvent("completed")
                finally:
                    FakeTransport.active -= 1

        proxy_server._workbuddy_gateway_locks.clear()
        with (
            patch.object(
                proxy_server,
                "workbuddy_transport",
                side_effect=lambda base_url, cwd=None: FakeTransport(base_url),
            ),
            patch.object(proxy_server.config, "WORKBUDDY_ACP_MAX_ATTEMPTS", 1),
        ):
            results = await asyncio.gather(
                proxy_server.complete_workbuddy_chat(
                    [{"role": "user", "content": "one"}],
                    gateway_url="http://sidecar-one",
                ),
                proxy_server.complete_workbuddy_chat(
                    [{"role": "user", "content": "two"}],
                    gateway_url="http://sidecar-two",
                ),
            )

        self.assertEqual(results, ["one", "two"])
        self.assertEqual(FakeTransport.max_active, 2)

    async def test_acp_stream_failure_is_http_error_not_assistant_text(self):
        async def unused_handler(request):
            raise AssertionError("HTTP transport should not be used")

        async def failed_stream(messages, **kwargs):
            raise WorkBuddyAcpError("gateway refused", category="authentication", status_code=403)
            yield

        with (
            patch.object(
                proxy_server,
                "resolve_proxy_session",
                return_value=(
                    {"id": "proxy-test", "project": "/tmp/project"},
                    "http://isolated-sidecar",
                ),
            ),
            patch.object(proxy_server, "stream_workbuddy_chat", side_effect=failed_stream),
        ):
            response = await self.call_app(
                {"model": "gpt-5.6-sol", "messages": [{"role": "user", "content": "hi"}], "stream": True},
                unused_handler,
                transport="workbuddy_acp",
            )
        self.assertEqual(response.status_code, 403)
        self.assertEqual(response.json()["error"]["type"], "workbuddy_acp_error")
        self.assertNotIn("choices", response.text)

    async def test_acp_stream_forwards_incremental_deltas(self):
        async def unused_handler(request):
            raise AssertionError("HTTP transport should not be used")

        async def events(messages, **kwargs):
            yield NormalizedEvent("text_delta", text="one ")
            yield NormalizedEvent("text_delta", text="two")
            yield NormalizedEvent("completed")

        with (
            patch.object(
                proxy_server,
                "resolve_proxy_session",
                return_value=(
                    {"id": "proxy-test", "project": "/tmp/project"},
                    "http://isolated-sidecar",
                ),
            ),
            patch.object(proxy_server, "stream_workbuddy_chat", side_effect=events),
        ):
            response = await self.call_app(
                {"model": "gpt-5.6-sol", "messages": [{"role": "user", "content": "hi"}], "stream": True},
                unused_handler,
                transport="workbuddy_acp",
            )
        data_lines = [line.removeprefix("data: ") for line in response.text.splitlines() if line.startswith("data: ")]
        chunks = [json.loads(line) for line in data_lines if line != "[DONE]"]
        deltas = [choice["delta"].get("content") for chunk in chunks for choice in chunk["choices"]]
        self.assertEqual([delta for delta in deltas if delta], ["one ", "two"])
        self.assertEqual(chunks[-1]["choices"][0]["finish_reason"], "stop")
        self.assertTrue(response.text.rstrip().endswith("data: [DONE]"))

    async def test_acp_error_after_delta_is_sse_error_without_done(self):
        async def unused_handler(request):
            raise AssertionError("HTTP transport should not be used")

        async def events(messages, **kwargs):
            yield NormalizedEvent("text_delta", text="partial")
            raise WorkBuddyAcpError("gateway lost", category="network", retryable=True)

        with (
            patch.object(
                proxy_server,
                "resolve_proxy_session",
                return_value=(
                    {"id": "proxy-test", "project": "/tmp/project"},
                    "http://isolated-sidecar",
                ),
            ),
            patch.object(proxy_server, "stream_workbuddy_chat", side_effect=events),
        ):
            response = await self.call_app(
                {"model": "gpt-5.6-sol", "messages": [{"role": "user", "content": "hi"}], "stream": True},
                unused_handler,
                transport="workbuddy_acp",
            )
        self.assertIn("workbuddy_acp_network", response.text)
        self.assertNotIn("[DONE]", response.text)


if __name__ == "__main__":
    unittest.main()
