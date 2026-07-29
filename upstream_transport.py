"""Upstream transports for direct HTTP and the official WorkBuddy ACP client."""

from __future__ import annotations

import asyncio
from dataclasses import dataclass
import json
import logging
import os
from pathlib import Path
from typing import AsyncIterator

import httpx


logger = logging.getLogger("freemodel-proxy.transport")


@dataclass
class NormalizedEvent:
    type: str
    text: str = ""
    message: str = ""
    usage: dict | None = None


class WorkBuddyAcpError(RuntimeError):
    """Sanitized ACP failure with routing and retry metadata."""

    def __init__(
        self,
        message: str,
        *,
        category: str = "protocol",
        retryable: bool = False,
        status_code: int | None = None,
    ):
        super().__init__(message)
        self.category = category
        self.retryable = retryable
        self.status_code = status_code

    @classmethod
    def from_http_status(cls, operation: str, status_code: int) -> "WorkBuddyAcpError":
        if status_code in {401, 403}:
            return cls(
                f"WorkBuddy ACP {operation} authentication failed ({status_code})",
                category="authentication",
                status_code=status_code,
            )
        retryable = status_code in {408, 409, 425, 429} or status_code >= 500
        category = "capacity" if status_code in {429, 503} else "upstream"
        return cls(
            f"WorkBuddy ACP {operation} failed ({status_code})",
            category=category,
            retryable=retryable,
            status_code=status_code,
        )


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
    def from_config(cls, config, base_url: str | None = None) -> "WorkBuddyAcpTransport":
        candidates = cls.candidate_urls(config)
        selected_url = (base_url or (candidates[0] if candidates else "")).rstrip("/")
        if not selected_url:
            raise WorkBuddyAcpError(
                "No active WorkBuddy ACP gateway was found. Start WorkBuddy or set WORKBUDDY_ACP_URL.",
                category="configuration",
            )
        return cls(
            selected_url,
            str(config.WORKBUDDY_ACP_PASSWORD or cls._environment_password()),
            config.WORKBUDDY_ACP_CWD,
            config.WORKBUDDY_ACP_TIMEOUT,
        )

    @classmethod
    def candidate_urls(cls, config) -> list[str]:
        discovered = cls.discover_all()
        configured = str(config.WORKBUDDY_ACP_URL or "").rstrip("/")
        if configured and configured not in discovered:
            discovered.append(configured)
        return discovered

    @staticmethod
    def _environment_password() -> str:
        return os.environ.get("WORKBUDDY_ACP_PASSWORD") or os.environ.get(
            "CODEBUDDY_GATEWAY_PASSWORD", ""
        )

    @staticmethod
    def _process_is_alive(pid) -> bool:
        try:
            return int(pid) > 0 and Path(f"/proc/{int(pid)}").exists()
        except (TypeError, ValueError):
            return False

    @staticmethod
    def discover_all(config_dir: Path | None = None) -> list[str]:
        root = config_dir or Path(os.environ.get("CODEBUDDY_CONFIG_DIR", Path.home() / ".workbuddy-ai"))
        sessions = root / "sessions"
        try:
            candidates = sorted(
                sessions.glob("*.json"),
                key=lambda path: path.stat().st_mtime,
                reverse=True,
            )
        except OSError:
            return []
        urls = []
        for candidate in candidates:
            try:
                data = json.loads(candidate.read_text(encoding="utf-8"))
            except (OSError, json.JSONDecodeError):
                continue
            if not WorkBuddyAcpTransport._process_is_alive(data.get("pid")):
                continue
            url = str(data.get("url") or data.get("endpoint") or "").rstrip("/")
            if url and url not in urls:
                urls.append(url)
        return urls

    @staticmethod
    def discover(config_dir: Path | None = None) -> tuple[str, str]:
        urls = WorkBuddyAcpTransport.discover_all(config_dir)
        if not urls:
            return "", ""
        return urls[0], WorkBuddyAcpTransport._environment_password()

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
                event = json.loads(data)
            except json.JSONDecodeError as exc:
                raise WorkBuddyAcpError(
                    "Malformed JSON in WorkBuddy ACP stream",
                    category="protocol",
                ) from exc
            if not isinstance(event, dict):
                raise WorkBuddyAcpError(
                    "Invalid JSON value in WorkBuddy ACP stream",
                    category="protocol",
                )
            yield event

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
                await response.aread()
                raise WorkBuddyAcpError.from_http_status(method, response.status_code)
            found_result = False
            async for event in self._read_sse_json(response):
                if event.get("id") == request_id:
                    if "error" in event:
                        error = event.get("error") or {}
                        message = str(error.get("message") or "WorkBuddy ACP JSON-RPC error")
                        lowered = message.lower()
                        retryable = any(
                            marker in lowered
                            for marker in ("timeout", "temporarily", "capacity", "refusal", "network", "interrupted")
                        )
                        raise WorkBuddyAcpError(
                            message,
                            category="upstream",
                            retryable=retryable,
                        )
                    found_result = True
                yield event
            if not found_result:
                raise WorkBuddyAcpError(
                    f"WorkBuddy ACP {method} ended without a result",
                    category="protocol",
                )

    async def _cancel_session(
        self,
        client: httpx.AsyncClient,
        connection_id: str,
        session_token: str,
        session_id: str,
    ) -> None:
        if not session_id:
            return
        try:
            async for _ in self._rpc_stream(
                client,
                connection_id,
                session_token,
                4,
                "session/cancel",
                {"sessionId": session_id},
            ):
                pass
        except Exception as exc:
            logger.warning("WorkBuddy ACP session cancellation failed: %s", type(exc).__name__)

    async def _close_connection(
        self,
        client: httpx.AsyncClient,
        connection_id: str,
        session_token: str,
    ) -> None:
        if not connection_id:
            return
        try:
            await client.delete(
                f"{self.base_url}/api/v1/acp",
                headers={
                    **self._headers(),
                    "acp-connection-id": connection_id,
                    "acp-session-token": session_token,
                },
            )
        except Exception as exc:
            logger.warning("WorkBuddy ACP connection cleanup failed: %s", type(exc).__name__)

    async def stream_chat(self, messages: list[dict]) -> AsyncIterator[NormalizedEvent]:
        prompt = serialize_messages(messages)
        timeout = httpx.Timeout(self.timeout, connect=10.0, write=30.0, pool=10.0)
        connection_id = ""
        session_token = ""
        session_id = ""
        async with httpx.AsyncClient(timeout=timeout) as client:
            try:
                headers = self._headers()
                headers["Accept"] = "text/event-stream"
                async with client.stream("GET", f"{self.base_url}/api/v1/acp", headers=headers) as response:
                    if response.status_code != 200:
                        await response.aread()
                        raise WorkBuddyAcpError.from_http_status("connection", response.status_code)
                    connection_id = response.headers.get("acp-connection-id", "")
                    session_token = response.headers.get("acp-session-token", "")
                if not connection_id:
                    raise WorkBuddyAcpError(
                        "WorkBuddy ACP did not provide a connection id",
                        category="protocol",
                    )

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
                    raise WorkBuddyAcpError(
                        "WorkBuddy ACP did not create a session",
                        category="protocol",
                    )

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
                            raise WorkBuddyAcpError(
                                f"WorkBuddy ACP stopped with reason: {stop_reason or 'unknown'}",
                                category="refusal" if stop_reason == "refusal" else "upstream",
                                retryable=stop_reason == "refusal",
                            )
                        completed = True
                if not completed:
                    raise WorkBuddyAcpError(
                        "WorkBuddy ACP prompt ended without completion",
                        category="protocol",
                    )
                yield NormalizedEvent("completed")
            except asyncio.CancelledError:
                await asyncio.shield(
                    self._cancel_session(client, connection_id, session_token, session_id)
                )
                raise
            except httpx.TimeoutException as exc:
                raise WorkBuddyAcpError(
                    "WorkBuddy ACP request timed out",
                    category="timeout",
                    retryable=True,
                ) from exc
            except httpx.RequestError as exc:
                raise WorkBuddyAcpError(
                    "WorkBuddy ACP connection failed",
                    category="network",
                    retryable=True,
                ) from exc
            finally:
                await asyncio.shield(
                    self._close_connection(client, connection_id, session_token)
                )

    async def complete_chat(self, messages: list[dict]) -> str:
        parts = []
        async for event in self.stream_chat(messages):
            if event.type == "text_delta":
                parts.append(event.text)
        return "".join(parts)
