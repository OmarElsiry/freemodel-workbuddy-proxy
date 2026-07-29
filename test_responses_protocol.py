"""Protocol-level regression tests for the OpenAI Responses API adapter."""

import json
import unittest
from unittest.mock import patch

import httpx

import proxy_server


class ResponsesProtocolTests(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        self.original_async_client = httpx.AsyncClient
        self.captured_requests = []

    def client_factory(self, handler):
        transport = httpx.MockTransport(handler)
        original_async_client = self.original_async_client

        def factory(*args, **kwargs):
            kwargs["transport"] = transport
            return original_async_client(*args, **kwargs)

        return factory

    async def call_app(self, payload, handler):
        transport = httpx.ASGITransport(app=proxy_server.app)
        async with self.original_async_client(
            transport=transport,
            base_url="http://testserver",
        ) as app_client:
            with (
                patch.object(proxy_server.config, "TRANSPORT", "http"),
                patch.object(proxy_server.httpx, "AsyncClient", self.client_factory(handler)),
            ):
                return await app_client.post("/v1/responses", json=payload)

    @staticmethod
    def parse_sse(response):
        events = []
        event_name = None
        for line in response.text.splitlines():
            if line.startswith("event:"):
                event_name = line.removeprefix("event:").strip()
            elif line.startswith("data:"):
                data = json.loads(line.removeprefix("data:").strip())
                events.append((event_name, data))
                event_name = None
        return events

    async def test_stream_emits_native_responses_events_and_completion(self):
        upstream_sse = "".join(
            [
                'data: {"id":"chatcmpl-1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"role":"assistant","content":"stream "},"finish_reason":null}]}\n\n',
                'data: {"id":"chatcmpl-1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":"ok"},"finish_reason":null}]}\n\n',
                'data: {"id":"chatcmpl-1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":4,"completion_tokens":2,"total_tokens":6}}\n\n',
                "data: [DONE]\n\n",
            ]
        )

        async def handler(request):
            self.captured_requests.append(json.loads(request.content))
            return httpx.Response(
                200,
                content=upstream_sse.encode(),
                headers={"content-type": "text/event-stream"},
            )

        response = await self.call_app(
            {
                "model": "gpt-5.6-sol",
                "instructions": "Be exact.",
                "input": "Reply with exactly: stream ok",
                "stream": True,
            },
            handler,
        )

        self.assertEqual(response.status_code, 200, response.text)
        events = self.parse_sse(response)
        event_names = [name for name, _ in events]
        self.assertEqual(event_names[0], "response.created")
        self.assertIn("response.output_text.delta", event_names)
        self.assertIn("response.output_item.done", event_names)
        self.assertEqual(event_names[-1], "response.completed")
        self.assertEqual(event_names.count("response.completed"), 1)
        self.assertNotIn(None, event_names)

        deltas = [data["delta"] for name, data in events if name == "response.output_text.delta"]
        self.assertEqual("".join(deltas), "stream ok")

        completed = events[-1][1]
        self.assertEqual(completed["type"], "response.completed")
        self.assertTrue(completed["response"]["id"].startswith("resp_"))
        self.assertEqual(completed["response"]["status"], "completed")
        self.assertEqual(completed["response"]["output"][0]["content"][0]["text"], "stream ok")
        self.assertEqual(completed["response"]["usage"]["total_tokens"], 6)

        upstream_request = self.captured_requests[0]
        self.assertEqual(upstream_request["messages"][0], {"role": "system", "content": "Be exact."})
        self.assertEqual(
            upstream_request["messages"][1],
            {"role": "user", "content": "Reply with exactly: stream ok"},
        )

    async def test_non_streaming_returns_native_response_object(self):
        async def handler(request):
            self.captured_requests.append(json.loads(request.content))
            return httpx.Response(
                200,
                json={
                    "id": "chatcmpl-2",
                    "object": "chat.completion",
                    "choices": [
                        {
                            "index": 0,
                            "message": {"role": "assistant", "content": "native response"},
                            "finish_reason": "stop",
                        }
                    ],
                    "usage": {"prompt_tokens": 3, "completion_tokens": 2, "total_tokens": 5},
                },
            )

        response = await self.call_app(
            {"model": "gpt-5.6-sol", "input": "Test non-streaming"},
            handler,
        )

        self.assertEqual(response.status_code, 200, response.text)
        result = response.json()
        self.assertEqual(result["object"], "response")
        self.assertEqual(result["status"], "completed")
        self.assertTrue(result["id"].startswith("resp_"))
        self.assertNotIn("choices", result)
        self.assertEqual(result["output"][0]["content"][0]["text"], "native response")
        self.assertEqual(result["usage"]["total_tokens"], 5)

    async def test_streaming_upstream_http_error_preserves_http_status(self):
        async def handler(request):
            return httpx.Response(
                503,
                json={"error": {"message": "capacity exhausted", "type": "upstream_error"}},
            )

        response = await self.call_app(
            {"model": "gpt-5.6-sol", "input": "Hello", "stream": True},
            handler,
        )

        self.assertEqual(response.status_code, 503)
        self.assertEqual(response.json()["error"]["message"], "capacity exhausted")
        self.assertNotEqual(response.headers.get("content-type"), "text/event-stream; charset=utf-8")

    async def test_stream_without_done_or_finish_reason_fails_explicitly(self):
        upstream_sse = (
            'data: {"id":"chatcmpl-3","object":"chat.completion.chunk",'
            '"choices":[{"index":0,"delta":{"content":"partial"},"finish_reason":null}]}\n\n'
        )

        async def handler(request):
            return httpx.Response(
                200,
                content=upstream_sse.encode(),
                headers={"content-type": "text/event-stream"},
            )

        response = await self.call_app(
            {"model": "gpt-5.6-sol", "input": "Hello", "stream": True},
            handler,
        )

        events = self.parse_sse(response)
        event_names = [name for name, _ in events]
        self.assertEqual(event_names[-1], "response.failed")
        self.assertNotIn("response.completed", event_names)
        self.assertEqual(
            events[-1][1]["response"]["error"]["code"],
            "upstream_stream_incomplete",
        )

    async def test_malformed_stream_fails_once(self):
        async def handler(request):
            return httpx.Response(
                200,
                content=b"data: not-json\n\n",
                headers={"content-type": "text/event-stream"},
            )

        response = await self.call_app(
            {"model": "gpt-5.6-sol", "input": "Hello", "stream": True},
            handler,
        )
        events = self.parse_sse(response)
        names = [name for name, _ in events]
        self.assertEqual(names.count("response.failed"), 1)
        self.assertEqual(names[-1], "response.failed")
        self.assertNotIn("response.completed", names)
        self.assertEqual(events[-1][1]["response"]["error"]["code"], "malformed_upstream_sse")

    async def test_done_without_finish_reason_is_not_completion(self):
        async def handler(request):
            return httpx.Response(
                200,
                content=b'data: {"choices":[{"delta":{"content":"partial"},"finish_reason":null}]}\n\ndata: [DONE]\n\n',
                headers={"content-type": "text/event-stream"},
            )

        response = await self.call_app(
            {"model": "gpt-5.6-sol", "input": "Hello", "stream": True},
            handler,
        )
        events = self.parse_sse(response)
        self.assertEqual(events[-1][0], "response.failed")
        self.assertNotIn("response.completed", [name for name, _ in events])

    async def test_messages_fallback_keeps_instructions(self):
        async def handler(request):
            self.captured_requests.append(json.loads(request.content))
            return httpx.Response(
                200,
                json={
                    "choices": [
                        {
                            "message": {"role": "assistant", "content": "ok"},
                            "finish_reason": "stop",
                        }
                    ]
                },
            )

        response = await self.call_app(
            {
                "model": "gpt-5.6-sol",
                "instructions": "Follow policy.",
                "messages": [{"role": "user", "content": "Hello"}],
            },
            handler,
        )

        self.assertEqual(response.status_code, 200, response.text)
        self.assertEqual(
            self.captured_requests[0]["messages"],
            [
                {"role": "system", "content": "Follow policy."},
                {"role": "user", "content": "Hello"},
            ],
        )

    async def test_streaming_tool_call_becomes_function_call_item(self):
        upstream_sse = "".join(
            [
                'data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_123","type":"function","function":{"name":"read_","arguments":"{\\"path\\":"}}]},"finish_reason":null}]}\n\n',
                'data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"name":"file","arguments":"\\"/tmp/x\\"}"}}]},"finish_reason":"tool_calls"}]}\n\n',
                "data: [DONE]\n\n",
            ]
        )

        async def handler(request):
            self.captured_requests.append(json.loads(request.content))
            return httpx.Response(
                200,
                content=upstream_sse.encode(),
                headers={"content-type": "text/event-stream"},
            )

        response = await self.call_app(
            {
                "model": "gpt-5.6-sol",
                "input": "Read the file",
                "tools": [
                    {
                        "type": "function",
                        "name": "read_file",
                        "description": "Read a file",
                        "parameters": {
                            "type": "object",
                            "properties": {"path": {"type": "string"}},
                            "required": ["path"],
                        },
                    }
                ],
                "stream": True,
            },
            handler,
        )

        events = self.parse_sse(response)
        done_items = [data["item"] for name, data in events if name == "response.output_item.done"]
        self.assertEqual(len(done_items), 1)
        self.assertEqual(done_items[0]["type"], "function_call")
        self.assertEqual(done_items[0]["call_id"], "call_123")
        self.assertEqual(done_items[0]["name"], "read_file")
        self.assertEqual(json.loads(done_items[0]["arguments"]), {"path": "/tmp/x"})
        self.assertEqual(events[-1][0], "response.completed")

        upstream_request = self.captured_requests[0]
        self.assertEqual(upstream_request["tools"][0]["type"], "function")
        self.assertEqual(upstream_request["tools"][0]["function"]["name"], "read_file")


if __name__ == "__main__":
    unittest.main()
