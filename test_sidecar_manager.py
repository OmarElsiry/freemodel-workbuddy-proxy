"""Tests for safe ownership and lifecycle checks in the sidecar manager."""

from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import AsyncMock, MagicMock, patch

from session_store import SessionStore
from sidecar_manager import SidecarManager


class SidecarManagerTests(unittest.IsolatedAsyncioTestCase):
    async def test_stop_does_not_signal_unowned_process(self):
        with tempfile.TemporaryDirectory() as directory:
            project = Path(directory) / "project"
            project.mkdir()
            store = SessionStore(Path(directory) / "sessions.json")
            session = await store.create(str(project), "Test")
            await store.update(
                session["id"],
                sidecar={"pid": 12345, "url": "http://127.0.0.1:1", "marker": "wrong"},
            )
            manager = SidecarManager(store, "/missing", Path(directory) / "runtime")
            with (
                patch.object(manager, "_process_matches", return_value=False),
                patch("sidecar_manager.os.killpg") as killpg,
            ):
                stopped = await manager.stop(session["id"])
            self.assertFalse(stopped)
            killpg.assert_not_called()
            self.assertEqual((await store.get(session["id"]))["sidecar"], {})

    async def test_reap_idle_stops_only_expired_sessions(self):
        with tempfile.TemporaryDirectory() as directory:
            project = Path(directory) / "project"
            project.mkdir()
            store = SessionStore(Path(directory) / "sessions.json")
            first = await store.create(str(project), "First")
            second = await store.create(str(project), "Second")
            manager = SidecarManager(
                store,
                "/missing",
                Path(directory) / "runtime",
                idle_timeout=10,
            )
            manager._last_used = {first["id"]: 1.0, second["id"]: 95.0}
            manager.stop = AsyncMock(return_value=True)
            with patch("sidecar_manager.time.monotonic", return_value=100.0):
                await manager.reap_idle()
            manager.stop.assert_awaited_once_with(first["id"])

    async def test_ensure_reuses_healthy_owned_sidecar(self):
        with tempfile.TemporaryDirectory() as directory:
            project = Path(directory) / "project"
            project.mkdir()
            store = SessionStore(Path(directory) / "sessions.json")
            session = await store.create(str(project), "Reusable")
            sidecar = {
                "pid": 12345,
                "url": "http://127.0.0.1:45555",
                "marker": f"proxy_{session['id']}",
            }
            await store.update(session["id"], sidecar=sidecar)
            manager = SidecarManager(store, "/missing", Path(directory) / "runtime")
            with (
                patch.object(manager, "_process_matches", return_value=True),
                patch.object(manager, "_healthy", AsyncMock(return_value=True)),
            ):
                url = await manager.ensure(session)
            self.assertEqual(url, sidecar["url"])
            self.assertIn(session["id"], manager._last_used)

    async def test_failed_store_adoption_terminates_spawned_process(self):
        with tempfile.TemporaryDirectory() as directory:
            project = Path(directory) / "project"
            project.mkdir()
            cli = Path(directory) / "codebuddy"
            cli.touch()
            store = SessionStore(Path(directory) / "sessions.json")
            session = await store.create(str(project), "Adoption failure")
            manager = SidecarManager(store, cli, Path(directory) / "runtime")
            process = MagicMock(spec=subprocess.Popen)
            process.pid = 54321
            process.poll.return_value = None
            terminate = MagicMock()
            with (
                patch("sidecar_manager.subprocess.Popen", return_value=process),
                patch.object(manager, "_healthy", AsyncMock(return_value=True)),
                patch.object(store, "update", AsyncMock(side_effect=RuntimeError("store failed"))),
                patch.object(manager, "_terminate_process", terminate),
            ):
                with self.assertRaisesRegex(RuntimeError, "store failed"):
                    await manager.ensure(session)
            terminate.assert_called_once_with(process)
            self.assertNotIn(session["id"], manager._processes)

    def test_terminate_process_reaps_early_exit(self):
        process = MagicMock(spec=subprocess.Popen)
        process.poll.return_value = 3
        SidecarManager._terminate_process(process)
        process.wait.assert_called_once_with()


if __name__ == "__main__":
    unittest.main()
