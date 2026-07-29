"""Tests for the official WorkBuddy ACP transport."""

import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from upstream_transport import NormalizedEvent, WorkBuddyAcpTransport, serialize_messages


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
            "discover",
            return_value=("http://127.0.0.1:22222", "discovered-secret"),
        ):
            transport = WorkBuddyAcpTransport.from_config(Config)
        self.assertEqual(transport.base_url, "http://127.0.0.1:22222")
        self.assertEqual(transport.password, "configured-secret")


if __name__ == "__main__":
    unittest.main()
