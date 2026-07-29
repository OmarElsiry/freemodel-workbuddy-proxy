"""Upstream transports for direct HTTP and the official WorkBuddy ACP client."""

from __future__ import annotations

from dataclasses import dataclass
import json
import os
from pathlib import Path
from typing import AsyncIterator
import uuid

import httpx


@dataclass
class NormalizedEvent:
    type: str
    text: str = ""
    message: str = ""
    usage: dict | None = None


class WorkBuddyAcpError(RuntimeError):
    pass


def serialize_messages(messages: list[dict]) -> str:
    """Serialize a chat history into one deterministic prompt for a fresh ACP session."""
    lines = [
        "Continue the following conversation. Return only the next assistant response. "
        "Do not call tools or describe tool use.",
        "",
    ]
    for message in messages:
        role = str(message.get("role", "user")).upper()
        if role in {"SYSTEM", "DEVELOPER"}:
            # WorkBuddy supplies its own trusted runtime instructions. Replaying another
            # agent runtime's system prompt as user text can trigger nested tool calls.
            continue
        content = message.get("content", "")
        if isinstance(content, list):
            parts = []
            for part in content:
                if isinstance(part, str):
                    parts.append(part)
                elif isinstance(part, dict) and part.get("type") in {"text", "input_text", "output_text"}:
                    parts.append(str(part.get("text", "")))
            content = "".join(parts)
        if message.get("tool_calls"):
            content = f"{content}\nTOOL_CALLS: {json.dumps(message['tool_calls'], separators=(',', ':'))}"
        if role == "TOOL":
            role = f"TOOL[{message.get('tool_call_id', '')}]"
        lines.extend([f"{role}:", str(content), ""])
    lines.append("ASSISTANT:")
    return "\n".join(lines)


class WorkBuddyAcpTransport:
    """Use the authenticated ACP service exposed by the official WorkBuddy client."""

    def __init__(self, base_url: str, password: str, cwd: str, timeout: float = 180.0):
        self.base_url = base_url.rstrip("/")
        self.password = password
        self.cwd = cwd
        self.timeout = timeout

    @classmethod
    def from_config(cls, config) -> "WorkBuddyAcpTransport":
        configured_url = str(config.WORKBUDDY_ACP_URL or "").rstrip("/")
        discovered_url, discovered_password = cls.discover()
        base_url = discovered_url or configured_url
        password = str(config.WORKBUDDY_ACP_PASSWORD or discovered_password)
        if not base_url:
            raise WorkBuddyAcpError(
                "No active WorkBuddy ACP gateway was found. Start WorkBuddy or set WORKBUDDY_ACP_URL."
            )
        return cls(
            base_url,
            password,
            config.WORKBUDDY_ACP_CWD,
            config.WORKBUDDY_ACP_TIMEOUT,
        )

    @staticmethod
    def _process_is_alive(pid) -> bool:
        try:
            return int(pid) > 0 and Path(f"/proc/{int(pid)}").exists()
        except (TypeError, ValueError):
            return False

    @staticmethod
    def discover(config_dir: Path | None = None) -> tuple[str, str]:
        root = config_dir or Path(os.environ.get("CODEBUDDY_CONFIG_DIR", Path.home() / ".workbuddy-ai"))
        sessions = root / "sessions"
        try:
            candidates = sorted(
                sessions.glob("*.json"),
                key=lambda path: path.stat().st_mtime,
                reverse=True,
            )
        except OSError:
            return "", ""
        for candidate in candidates:
            try:
                data = json.loads(candidate.read_text(encoding="utf-8"))
            except (OSError, json.JSONDecodeError):
                continue
            if not WorkBuddyAcpTransport._process_is_alive(data.get("pid")):
                continue
            url = str(data.get("url") or data.get("endpoint") or "").rstrip("/")
            if url:
                password = os.environ.get("WORKBUDDY_ACP_PASSWORD") or os.environ.get(
                    "CODEBUDDY_GATEWAY_PASSWORD", ""
                )
                return url, password
        return "", ""

    def _headers(self) -> dict[str, str]:
        headers = {
            "Accept": "application/json, text/event-stream",
            "Content-Type": "application/json",
            "X-CodeBuddy-Request": "1",
        }
        if self.password:
            headers["Authorization"] = f"Bearer {self.password}"
        return headers

    @staticmethod
    async def _read_sse_json(response: httpx.Response) -> AsyncIterator[dict]:
        async for line in response.aiter_lines():
            line = line.strip()
            if not line or line.startswith(":") or not line.startswith("data:"):
                continue
            data = line[5:].strip()
            if not data:
                continue
            try:
                yield json.loads(data)
            except json.JSONDecodeError as exc:
                raise WorkBuddyAcpError("Malformed JSON in WorkBuddy ACP stream") from exc

    async def _rpc_stream(
        self,
        client: httpx.AsyncClient,
        connection_id: str,
        session_token: str,
        request_id: int,
        method: str,
        params: dict,
    ) -> AsyncIterator[dict]:
        headers = self._headers()
        headers["acp-connection-id"] = connection_id
        if session_token:
            headers["acp-session-token"] = session_token
        payload = {"jsonrpc": "2.0", "id": request_id, "method": method, "params": params}
        async with client.stream("POST", f"{self.base_url}/api/v1/acp", headers=headers, json=payload) as response:
            if response.status_code != 200:
                body = (await response.aread()).decode("utf-8", errors="replace")
                raise WorkBuddyAcpError(f"WorkBuddy ACP {method} failed ({response.status_code}): {body}")
            found_result = False
            async for event in self._read_sse_json(response):
                if event.get("id") == request_id:
                    if "error" in event:
                        error = event.get("error") or {}
                        raise WorkBuddyAcpError(str(error.get("message") or error))
                    found_result = True
                yield event
            if not found_result:
                raise WorkBuddyAcpError(f"WorkBuddy ACP {method} ended without a result")

    async def stream_chat(self, messages: list[dict]) -> AsyncIterator[NormalizedEvent]:
        prompt = serialize_messages(messages)
        timeout = httpx.Timeout(self.timeout, connect=10.0)
        async with httpx.AsyncClient(timeout=timeout) as client:
            headers = self._headers()
            headers["Accept"] = "text/event-stream"
            async with client.stream("GET", f"{self.base_url}/api/v1/acp", headers=headers) as response:
                if response.status_code != 200:
                    body = (await response.aread()).decode("utf-8", errors="replace")
                    raise WorkBuddyAcpError(f"Cannot connect to WorkBuddy ACP ({response.status_code}): {body}")
                connection_id = response.headers.get("acp-connection-id", "")
                session_token = response.headers.get("acp-session-token", "")
            if not connection_id:
                raise WorkBuddyAcpError("WorkBuddy ACP did not provide a connection id")

            try:
                async for _ in self._rpc_stream(
                    client,
                    connection_id,
                    session_token,
                    1,
                    "initialize",
                    {
                        "protocolVersion": 1,
                        "clientCapabilities": {},
                        "clientInfo": {
                            "name": "freemodel-workbuddy-proxy",
                            "title": "Freemodel WorkBuddy Proxy",
                            "version": "1.0.0",
                        },
                    },
                ):
                    pass

                session_id = ""
                async for event in self._rpc_stream(
                    client,
                    connection_id,
                    session_token,
                    2,
                    "session/new",
                    {"cwd": self.cwd, "mcpServers": []},
                ):
                    if event.get("id") == 2:
                        session_id = str((event.get("result") or {}).get("sessionId") or "")
                if not session_id:
                    raise WorkBuddyAcpError("WorkBuddy ACP did not create a session")

                completed = False
                async for event in self._rpc_stream(
                    client,
                    connection_id,
                    session_token,
                    3,
                    "session/prompt",
                    {"sessionId": session_id, "prompt": [{"type": "text", "text": prompt}]},
                ):
                    if event.get("method") == "session/update":
                        update = ((event.get("params") or {}).get("update") or {})
                        if update.get("sessionUpdate") == "agent_message_chunk":
                            content = update.get("content") or {}
                            if content.get("type") == "text" and content.get("text"):
                                yield NormalizedEvent("text_delta", text=str(content["text"]))
                    elif event.get("id") == 3:
                        stop_reason = str((event.get("result") or {}).get("stopReason") or "")
                        if stop_reason != "end_turn":
                            raise WorkBuddyAcpError(f"WorkBuddy ACP stopped with reason: {stop_reason or 'unknown'}")
                        completed = True
                if not completed:
                    raise WorkBuddyAcpError("WorkBuddy ACP prompt ended without completion")
                yield NormalizedEvent("completed")
            finally:
                try:
                    await client.delete(
                        f"{self.base_url}/api/v1/acp",
                        headers={**self._headers(), "acp-connection-id": connection_id, "acp-session-token": session_token},
                    )
                except Exception:
                    pass

    async def complete_chat(self, messages: list[dict]) -> str:
        parts = []
        async for event in self.stream_chat(messages):
            if event.type == "text_delta":
                parts.append(event.text)
        return "".join(parts)
