"""Persistent proxy-owned projects and conversation routing metadata."""

from __future__ import annotations

import asyncio
from copy import deepcopy
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import tempfile
import uuid


SESSION_ID_PATTERN = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_-]{7,127}$")
ALLOWED_HISTORY_ROLES = {"system", "developer", "user", "assistant", "tool"}


def validate_history(messages: list[dict]) -> list[dict]:
    if not isinstance(messages, list):
        raise ValueError("history must be an array")
    validated = []
    for index, message in enumerate(messages):
        if not isinstance(message, dict):
            raise ValueError(f"history item {index} must be an object")
        role = str(message.get("role") or "")
        if role not in ALLOWED_HISTORY_ROLES:
            raise ValueError(f"history item {index} has an invalid role")
        if "content" not in message:
            raise ValueError(f"history item {index} is missing content")
        validated.append(deepcopy(message))
    return validated


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat()


def canonical_project(path: str) -> str:
    project = Path(path).expanduser().resolve()
    if not project.is_dir():
        raise ValueError(f"Project directory does not exist: {project}")
    return str(project)


def validate_session_id(session_id: str) -> str:
    value = str(session_id or "").strip()
    if not SESSION_ID_PATTERN.fullmatch(value):
        raise ValueError("Invalid proxy session ID")
    return value


def stable_context_text(messages: list[dict]) -> str:
    stable = []
    for message in messages:
        if not isinstance(message, dict):
            continue
        role = str(message.get("role") or "")
        if role not in {"system", "developer", "user"}:
            continue
        content = message.get("content", "")
        if isinstance(content, list):
            content = "".join(
                str(part.get("text", ""))
                for part in content
                if isinstance(part, dict) and part.get("type") in {"text", "input_text", "output_text"}
            )
        stable.append(f"{role}:{content}")
        if role == "user":
            break
    return "\n".join(stable)


def automatic_session_id(project: str, messages: list[dict]) -> str:
    source = f"{canonical_project(project)}\0{stable_context_text(messages)}".encode("utf-8")
    return f"auto-{hashlib.sha256(source).hexdigest()[:24]}"


class SessionStore:
    def __init__(self, path: str | Path, max_history_turns: int = 100):
        self.path = Path(path)
        self.max_history_messages = max(2, int(max_history_turns) * 2)
        self._lock = asyncio.Lock()

    def _read(self) -> dict:
        if not self.path.exists():
            return {"version": 1, "sessions": {}}
        try:
            data = json.loads(self.path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            raise RuntimeError(f"Unable to read proxy session store: {exc}") from exc
        if not isinstance(data, dict) or data.get("version") != 1 or not isinstance(data.get("sessions"), dict):
            raise RuntimeError("Invalid proxy session store format")
        return data

    def _write(self, data: dict) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        fd, temporary = tempfile.mkstemp(prefix=f".{self.path.name}.", dir=self.path.parent)
        try:
            os.fchmod(fd, 0o600)
            with os.fdopen(fd, "w", encoding="utf-8") as handle:
                json.dump(data, handle, indent=2, ensure_ascii=False)
                handle.flush()
                os.fsync(handle.fileno())
            os.replace(temporary, self.path)
            os.chmod(self.path, 0o600)
        finally:
            if os.path.exists(temporary):
                os.unlink(temporary)

    @staticmethod
    def _public(record: dict) -> dict:
        return deepcopy(record)

    async def list(self, project: str | None = None) -> list[dict]:
        async with self._lock:
            records = list(self._read()["sessions"].values())
        if project:
            canonical = canonical_project(project)
            records = [record for record in records if record["project"] == canonical]
        return sorted((self._public(record) for record in records), key=lambda item: item["updated_at"], reverse=True)

    async def get(self, session_id: str) -> dict | None:
        session_id = validate_session_id(session_id)
        async with self._lock:
            record = self._read()["sessions"].get(session_id)
        return self._public(record) if record else None

    async def create(
        self,
        project: str,
        title: str = "",
        *,
        session_id: str | None = None,
        automatic: bool = False,
    ) -> dict:
        canonical = canonical_project(project)
        session_id = validate_session_id(session_id or f"proxy-{uuid.uuid4().hex}")
        now = utc_now()
        async with self._lock:
            data = self._read()
            existing = data["sessions"].get(session_id)
            if existing:
                if existing["project"] != canonical:
                    raise ValueError("Proxy session belongs to a different project")
                return self._public(existing)
            record = {
                "id": session_id,
                "title": str(title or Path(canonical).name or "Proxy session")[:120],
                "project": canonical,
                "automatic": bool(automatic),
                "created_at": now,
                "updated_at": now,
                "history": [],
                "sidecar": {},
            }
            data["sessions"][session_id] = record
            self._write(data)
        return self._public(record)

    async def get_or_create_automatic(self, project: str, messages: list[dict]) -> dict:
        session_id = automatic_session_id(project, messages)
        return await self.create(
            project,
            title="Automatic client session",
            session_id=session_id,
            automatic=True,
        )

    async def update(self, session_id: str, **changes) -> dict:
        session_id = validate_session_id(session_id)
        allowed = {"title", "history", "sidecar"}
        unknown = set(changes) - allowed
        if unknown:
            raise ValueError(f"Unsupported session fields: {', '.join(sorted(unknown))}")
        async with self._lock:
            data = self._read()
            record = data["sessions"].get(session_id)
            if not record:
                raise KeyError(session_id)
            if "title" in changes:
                record["title"] = str(changes["title"] or record["title"])[:120]
            if "history" in changes:
                history = validate_history(changes["history"])
                record["history"] = history[-self.max_history_messages :]
            if "sidecar" in changes:
                sidecar = changes["sidecar"]
                if not isinstance(sidecar, dict):
                    raise ValueError("sidecar must be an object")
                record["sidecar"] = deepcopy(sidecar)
            record["updated_at"] = utc_now()
            self._write(data)
        return self._public(record)

    async def append_history(self, session_id: str, messages: list[dict]) -> dict:
        record = await self.get(session_id)
        if not record:
            raise KeyError(session_id)
        history = record.get("history", []) + deepcopy(messages)
        return await self.update(session_id, history=history)

    async def clear_sidecar(self, session_id: str) -> dict:
        return await self.update(session_id, sidecar={})

    async def delete(self, session_id: str) -> bool:
        session_id = validate_session_id(session_id)
        async with self._lock:
            data = self._read()
            existed = data["sessions"].pop(session_id, None) is not None
            if existed:
                self._write(data)
        return existed

    async def clear_stale_runtime(self) -> None:
        async with self._lock:
            data = self._read()
            changed = False
            for record in data["sessions"].values():
                sidecar = record.get("sidecar") or {}
                pid = sidecar.get("pid")
                if pid and not Path(f"/proc/{pid}").exists():
                    record["sidecar"] = {}
                    changed = True
            if changed:
                self._write(data)
