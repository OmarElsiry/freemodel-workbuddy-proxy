"""Configuration regression tests for the Freemodel proxy."""

import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch


CONFIG_PATH = Path(__file__).with_name("config.py")


def load_config(config_file: Path, environment: dict[str, str] | None = None):
    """Import config.py beside an isolated test config.json.

    config.py intentionally resolves its project configuration relative to its own
    location at import time. Loading a temporary copy exercises that real behavior
    without allowing the developer's ignored local config.json to leak into tests.
    """
    isolated_module_path = config_file.with_name("config_under_test.py")
    isolated_module_path.write_text(CONFIG_PATH.read_text(encoding="utf-8"), encoding="utf-8")
    spec = importlib.util.spec_from_file_location("config_under_test", isolated_module_path)
    module = importlib.util.module_from_spec(spec)
    with patch.dict(os.environ, environment or {}, clear=True):
        with patch.object(Path, "home", return_value=config_file.parent / "home"):
            spec.loader.exec_module(module)
    return module


class ConfigTests(unittest.TestCase):
    def test_defaults_to_workbuddy_service_over_official_acp(self):
        with tempfile.TemporaryDirectory() as directory:
            config_file = Path(directory) / "config.json"
            config = load_config(config_file)

        self.assertEqual(config.DEFAULT_BASE_URL, "https://work.freemodel.dev/v1")
        self.assertEqual(config.TRANSPORT, "workbuddy_acp")
        self.assertEqual(config.WORKBUDDY_CLI_PATH, "codebuddy")

    def test_reads_saved_base_url_and_strips_trailing_slash(self):
        with tempfile.TemporaryDirectory() as directory:
            config_file = Path(directory) / "config.json"
            config_file.write_text(
                json.dumps({"FREEMODEL_BASE_URL": "https://example.test/v1/"}),
                encoding="utf-8",
            )
            config = load_config(config_file)

        self.assertEqual(config.DEFAULT_BASE_URL, "https://example.test/v1")
        self.assertEqual(
            f"{config.DEFAULT_BASE_URL.rstrip('/')}/chat/completions",
            "https://example.test/v1/chat/completions",
        )

    def test_environment_base_url_overrides_saved_value(self):
        with tempfile.TemporaryDirectory() as directory:
            config_file = Path(directory) / "config.json"
            config_file.write_text(
                json.dumps({"FREEMODEL_BASE_URL": "https://saved.test/v1"}),
                encoding="utf-8",
            )
            config = load_config(
                config_file,
                {"FREEMODEL_BASE_URL": "https://environment.test/v1/"},
            )

        self.assertEqual(config.DEFAULT_BASE_URL, "https://environment.test/v1")

    def test_protected_endpoint_defaults_to_workbuddy_acp(self):
        with tempfile.TemporaryDirectory() as directory:
            config_file = Path(directory) / "config.json"
            config_file.write_text(
                json.dumps({"FREEMODEL_BASE_URL": "https://work.freemodel.dev/v1"}),
                encoding="utf-8",
            )
            config = load_config(config_file)

        self.assertEqual(config.TRANSPORT, "workbuddy_acp")

    def test_transport_environment_overrides_saved_value(self):
        with tempfile.TemporaryDirectory() as directory:
            config_file = Path(directory) / "config.json"
            config_file.write_text(
                json.dumps(
                    {
                        "FREEMODEL_BASE_URL": "https://generic.example/v1",
                        "FREEMODEL_TRANSPORT": "workbuddy_acp",
                    }
                ),
                encoding="utf-8",
            )
            config = load_config(
                config_file,
                {
                    "FREEMODEL_BASE_URL": "https://generic.example/v1",
                    "FREEMODEL_TRANSPORT": "http",
                },
            )

        self.assertEqual(config.TRANSPORT, "http")

    def test_protected_endpoint_rejects_http_transport(self):
        with tempfile.TemporaryDirectory() as directory:
            config_file = Path(directory) / "config.json"
            with self.assertRaisesRegex(ValueError, "requires FREEMODEL_TRANSPORT=workbuddy_acp"):
                load_config(
                    config_file,
                    {
                        "FREEMODEL_BASE_URL": "https://work.freemodel.dev/v1",
                        "FREEMODEL_TRANSPORT": "http",
                    },
                )

    def test_similar_hostname_is_not_treated_as_protected(self):
        with tempfile.TemporaryDirectory() as directory:
            config_file = Path(directory) / "config.json"
            config = load_config(
                config_file,
                {"FREEMODEL_BASE_URL": "https://work.freemodel.dev.attacker.test/v1"},
            )

        self.assertEqual(config.TRANSPORT, "http")

    def test_codebuddy_cli_uses_explicit_path_or_path_discovery(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            bin_dir = root / "bin"
            bin_dir.mkdir()
            discovered = bin_dir / "codebuddy"
            discovered.write_text("#!/bin/sh\n", encoding="utf-8")
            discovered.chmod(0o700)
            config = load_config(root / "config.json", {"PATH": str(bin_dir)})
            self.assertEqual(config.WORKBUDDY_CLI_PATH, str(discovered))

            explicit = root / "custom-codebuddy"
            config = load_config(
                root / "config.json",
                {"PATH": str(bin_dir), "WORKBUDDY_CLI_PATH": str(explicit)},
            )
            self.assertEqual(config.WORKBUDDY_CLI_PATH, str(explicit))

        self.assertNotIn("/home/potterparker/", CONFIG_PATH.read_text(encoding="utf-8"))

    def test_reads_key_without_exposing_it_in_source(self):
        with tempfile.TemporaryDirectory() as directory:
            config_file = Path(directory) / "config.json"
            config_file.write_text(
                json.dumps({"FREEMODEL_API_KEY": "fe_test_secret"}),
                encoding="utf-8",
            )
            config = load_config(config_file)

        self.assertEqual(config.DEFAULT_API_KEY, "fe_test_secret")
        self.assertNotIn("fe_test_secret", CONFIG_PATH.read_text(encoding="utf-8"))


if __name__ == "__main__":
    unittest.main()
