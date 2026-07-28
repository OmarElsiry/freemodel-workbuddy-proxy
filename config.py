"""Configuration for the Freemodel WorkBuddy API Proxy."""

import os
import json
from pathlib import Path

CONFIG_FILE = Path(__file__).parent / "config.json"
DEFAULT_PUBLIC_BASE_URL = "https://api.freemodel.dev/v1"


def load_saved_key() -> str:
    if CONFIG_FILE.exists():
        try:
            with open(CONFIG_FILE, "r") as f:
                data = json.load(f)
                key = data.get("FREEMODEL_API_KEY", "").strip()
                if key:
                    return key
        except Exception:
            pass
    # Fallback to ~/.codex/auth.json
    codex_auth = Path.home() / ".codex" / "auth.json"
    if codex_auth.exists():
        try:
            with open(codex_auth, "r") as f:
                cdata = json.load(f)
                key = (cdata.get("FREEMODEL_API_KEY") or cdata.get("OPENAI_API_KEY") or "").strip()
                if key:
                    return key
        except Exception:
            pass
    return ""


def load_saved_base_url() -> str:
    if CONFIG_FILE.exists():
        try:
            with open(CONFIG_FILE, "r") as f:
                data = json.load(f)
                base_url = data.get("FREEMODEL_BASE_URL", "").strip()
                if base_url:
                    return base_url
        except Exception:
            pass
    return ""


def save_key(key: str):
    data = {}
    if CONFIG_FILE.exists():
        try:
            with open(CONFIG_FILE, "r") as f:
                data = json.load(f)
        except Exception:
            pass
    data["FREEMODEL_API_KEY"] = key.strip()
    with open(CONFIG_FILE, "w") as f:
        json.dump(data, f, indent=2)

DEFAULT_BASE_URL = (
    os.environ.get("FREEMODEL_BASE_URL")
    or load_saved_base_url()
    or DEFAULT_PUBLIC_BASE_URL
).rstrip("/")
DEFAULT_API_KEY = os.environ.get("FREEMODEL_API_KEY") or load_saved_key()

CLIENT_HEADERS = {}

# Available models advertised in GET /v1/models
AVAILABLE_MODELS = [
    {
        "id": "gpt-5.6-sol",
        "object": "model",
        "created": 1785164333,
        "owned_by": "freemodel",
    },
    {
        "id": "gpt 5.6 sol",
        "object": "model",
        "created": 1785164333,
        "owned_by": "freemodel",
    },
    {
        "id": "gpt-4o",
        "object": "model",
        "created": 1785164333,
        "owned_by": "freemodel",
    },
    {
        "id": "opencode-default",
        "object": "model",
        "created": 1785164333,
        "owned_by": "freemodel",
    },
]

DEFAULT_PORT = int(os.environ.get("PROXY_PORT", "40589"))
DEFAULT_HOST = os.environ.get("PROXY_HOST", "0.0.0.0")
