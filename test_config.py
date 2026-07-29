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
    spec = importlib.util.spec_from_file_location("config_under_test", CONFIG_PATH)
    module = importlib.util.module_from_spec(spec)
    with patch.dict(os.environ, environment or {}, clear=True):
        with patch.object(Path, "home", return_value=config_file.parent / "home"):
            spec.loader.exec_module(module)
    module.CONFIG_FILE = config_file
    with patch.dict(os.environ, environment or {}, clear=True):
        module.DEFAULT_BASE_URL = (
            os.environ.get("FREEMODEL_BASE_URL")
            or module.load_saved_base_url()
            or module.DEFAULT_PUBLIC_BASE_URL
        ).rstrip("/")
        module.DEFAULT_API_KEY = os.environ.get("FREEMODEL_API_KEY") or module.load_saved_key()
        module.TRANSPORT = str(
            os.environ.get("FREEMODEL_TRANSPORT")
            or module.load_saved_value(
                "FREEMODEL_TRANSPORT",
                "workbuddy_acp" if "work.freemodel.dev" in module.DEFAULT_BASE_URL else "http",
            )
        ).strip().lower()
    return module


class ConfigTests(unittest.TestCase):
    def test_defaults_to_public_endpoint(self):
        with tempfile.TemporaryDirectory() as directory:
            config_file = Path(directory) / "config.json"
            config = load_config(config_file)

        self.assertEqual(config.DEFAULT_BASE_URL, "https://api.freemodel.dev/v1")

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
                json.dumps({"FREEMODEL_TRANSPORT": "workbuddy_acp"}),
                encoding="utf-8",
            )
            config = load_config(config_file, {"FREEMODEL_TRANSPORT": "http"})

        self.assertEqual(config.TRANSPORT, "http")

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
