"""Lifecycle management for proxy-owned official CodeBuddy CLI gateways."""

from __future__ import annotations

import asyncio
from contextlib import suppress
import os
from pathlib import Path
import signal
import socket
import subprocess
import time

import httpx

from session_store import SessionStore


class SidecarError(RuntimeError):
    pass


def free_loopback_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


class SidecarManager:
    def __init__(
        self,
        store: SessionStore,
        cli_path: str,
        runtime_dir: str | Path,
        startup_timeout: float = 30.0,
        idle_timeout: float = 900.0,
    ):
        self.store = store
        self.cli_path = Path(cli_path)
        self.runtime_dir = Path(runtime_dir)
        self.startup_timeout = float(startup_timeout)
        self.idle_timeout = float(idle_timeout)
        self._locks: dict[str, asyncio.Lock] = {}
        self._last_used: dict[str, float] = {}
        self._processes: dict[str, subprocess.Popen] = {}

    def _lock(self, session_id: str) -> asyncio.Lock:
        return self._locks.setdefault(session_id, asyncio.Lock())

    @staticmethod
    def _process_matches(pid: int, marker: str) -> bool:
        try:
            command = Path(f"/proc/{int(pid)}/cmdline").read_bytes().replace(b"\0", b" ").decode()
        except (OSError, ValueError):
            return False
        return "codebuddy" in command and "--serve" in command and marker in command

    @staticmethod
    async def _healthy(url: str) -> bool:
        try:
            async with httpx.AsyncClient(timeout=1.5) as client:
                response = await client.get(f"{url}/api/v1/health")
            return response.status_code == 200
        except httpx.HTTPError:
            return False

    @staticmethod
    def _terminate_process(process: subprocess.Popen) -> None:
        if process.poll() is not None:
            process.wait()
            return
        with suppress(ProcessLookupError):
            os.killpg(process.pid, signal.SIGTERM)
        try:
            process.wait(timeout=5)
            return
        except subprocess.TimeoutExpired:
            pass
        with suppress(ProcessLookupError):
            os.killpg(process.pid, signal.SIGKILL)
        with suppress(subprocess.TimeoutExpired):
            process.wait(timeout=5)

    async def ensure(self, session: dict) -> str:
        session_id = session["id"]
        marker = f"proxy_{session_id}"
        async with self._lock(session_id):
            current = await self.store.get(session_id)
            if not current:
                raise SidecarError("Proxy session no longer exists")
            sidecar = current.get("sidecar") or {}
            pid = sidecar.get("pid")
            url = sidecar.get("url")
            if pid and url and self._process_matches(pid, marker) and await self._healthy(url):
                self._last_used[session_id] = time.monotonic()
                return url

            if not self.cli_path.is_file():
                raise SidecarError(f"Official CodeBuddy CLI was not found: {self.cli_path}")
            self.runtime_dir.mkdir(parents=True, exist_ok=True, mode=0o700)
            os.chmod(self.runtime_dir, 0o700)
            port = free_loopback_port()
            url = f"http://127.0.0.1:{port}"
            log_path = self.runtime_dir / f"{session_id}.log"
            log_fd = os.open(log_path, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
            os.chmod(log_path, 0o600)
            log_handle = os.fdopen(log_fd, "ab", buffering=0)
            environment = os.environ.copy()
            environment.update(
                {
                    "WORKBUDDY_PROXY_SIDECAR": "1",
                    "WORKBUDDY_PROXY_SESSION": session_id,
                    "CODEBUDDY_GATEWAY_AUTH": "none",
                }
            )
            command = [
                str(self.cli_path),
                "--serve",
                "--host",
                "127.0.0.1",
                "--port",
                str(port),
                "--session-id",
                marker,
                "--permission-mode",
                "bypassPermissions",
            ]
            try:
                process = subprocess.Popen(
                    command,
                    cwd=current["project"],
                    env=environment,
                    stdin=subprocess.DEVNULL,
                    stdout=log_handle,
                    stderr=log_handle,
                    start_new_session=True,
                )
            finally:
                log_handle.close()

            adopted = False
            try:
                deadline = time.monotonic() + self.startup_timeout
                while time.monotonic() < deadline:
                    if process.poll() is not None:
                        process.wait()
                        raise SidecarError(f"CodeBuddy sidecar exited with status {process.returncode}")
                    if await self._healthy(url):
                        await self.store.update(
                            session_id,
                            sidecar={
                                "pid": process.pid,
                                "port": port,
                                "url": url,
                                "marker": marker,
                                "started_at": time.time(),
                            },
                        )
                        self._processes[session_id] = process
                        self._last_used[session_id] = time.monotonic()
                        adopted = True
                        return url
                    await asyncio.sleep(0.2)
                raise SidecarError("Timed out waiting for the CodeBuddy sidecar")
            finally:
                if not adopted:
                    self._terminate_process(process)

    async def stop(self, session_id: str) -> bool:
        async with self._lock(session_id):
            session = await self.store.get(session_id)
            if not session:
                return False
            sidecar = session.get("sidecar") or {}
            pid = sidecar.get("pid")
            marker = sidecar.get("marker") or f"proxy_{session_id}"
            stopped = False
            process = self._processes.pop(session_id, None)
            if pid and self._process_matches(pid, marker):
                if process is not None and process.pid == int(pid):
                    self._terminate_process(process)
                    stopped = True
                else:
                    with suppress(ProcessLookupError):
                        os.killpg(int(pid), signal.SIGTERM)
                        stopped = True
                    deadline = time.monotonic() + 5
                    while Path(f"/proc/{pid}").exists() and time.monotonic() < deadline:
                        await asyncio.sleep(0.1)
                    if Path(f"/proc/{pid}").exists() and self._process_matches(pid, marker):
                        with suppress(ProcessLookupError):
                            os.killpg(int(pid), signal.SIGKILL)
            elif process is not None:
                self._terminate_process(process)
            await self.store.clear_sidecar(session_id)
            self._last_used.pop(session_id, None)
            return stopped

    async def reap_idle(self) -> None:
        if self.idle_timeout <= 0:
            return
        now = time.monotonic()
        for session_id, last_used in list(self._last_used.items()):
            if now - last_used >= self.idle_timeout:
                await self.stop(session_id)

    async def stop_all(self) -> None:
        for session in await self.store.list():
            if session.get("sidecar"):
                await self.stop(session["id"])
