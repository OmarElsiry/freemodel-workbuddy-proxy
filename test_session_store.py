"""Tests for proxy-owned session persistence and automatic routing."""

import asyncio
from pathlib import Path
import stat
import tempfile
import unittest

from session_store import SessionStore, automatic_session_id


class SessionStoreTests(unittest.TestCase):
    def test_create_list_history_and_delete(self):
        async def run():
            with tempfile.TemporaryDirectory() as directory:
                project = Path(directory) / "project"
                project.mkdir()
                store = SessionStore(Path(directory) / "sessions.json", max_history_turns=1)
                session = await store.create(str(project), "Example")
                await store.append_history(
                    session["id"],
                    [
                        {"role": "user", "content": "one"},
                        {"role": "assistant", "content": "two"},
                        {"role": "user", "content": "three"},
                    ],
                )
                loaded = await store.get(session["id"])
                self.assertEqual([item["content"] for item in loaded["history"]], ["two", "three"])
                self.assertEqual(len(await store.list(str(project))), 1)
                self.assertTrue(await store.delete(session["id"]))
                self.assertIsNone(await store.get(session["id"]))

        asyncio.run(run())

    def test_automatic_id_is_stable_when_history_grows(self):
        with tempfile.TemporaryDirectory() as directory:
            messages = [{"role": "user", "content": "first prompt"}]
            first = automatic_session_id(directory, messages)
            second = automatic_session_id(
                directory,
                messages
                + [
                    {"role": "assistant", "content": "reply"},
                    {"role": "user", "content": "follow up"},
                ],
            )
            self.assertEqual(first, second)

    def test_same_prompt_in_different_projects_is_isolated(self):
        with tempfile.TemporaryDirectory() as directory:
            first = Path(directory) / "one"
            second = Path(directory) / "two"
            first.mkdir()
            second.mkdir()
            messages = [{"role": "user", "content": "hello"}]
            self.assertNotEqual(automatic_session_id(str(first), messages), automatic_session_id(str(second), messages))

    def test_store_file_is_private(self):
        async def run():
            with tempfile.TemporaryDirectory() as directory:
                project = Path(directory) / "project"
                project.mkdir()
                path = Path(directory) / "sessions.json"
                store = SessionStore(path)
                await store.create(str(project), "Private")
                self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)

        asyncio.run(run())

    def test_history_rejects_invalid_items(self):
        async def run():
            with tempfile.TemporaryDirectory() as directory:
                project = Path(directory) / "project"
                project.mkdir()
                store = SessionStore(Path(directory) / "sessions.json")
                session = await store.create(str(project), "Validation")
                with self.assertRaisesRegex(ValueError, "must be an object"):
                    await store.append_history(session["id"], ["not-a-message"])
                with self.assertRaisesRegex(ValueError, "invalid role"):
                    await store.append_history(
                        session["id"],
                        [{"role": "intruder", "content": "bad"}],
                    )

        asyncio.run(run())


if __name__ == "__main__":
    unittest.main()
