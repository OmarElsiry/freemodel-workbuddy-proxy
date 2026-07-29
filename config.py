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


def load_saved_value(name: str, default=""):
    if CONFIG_FILE.exists():
        try:
            with open(CONFIG_FILE, "r") as f:
                return json.load(f).get(name, default)
        except Exception:
            pass
    return default


def load_saved_base_url() -> str:
    return str(load_saved_value("FREEMODEL_BASE_URL", "")).strip()


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

TRANSPORT = str(
    os.environ.get("FREEMODEL_TRANSPORT")
    or load_saved_value(
        "FREEMODEL_TRANSPORT",
        "workbuddy_acp" if "work.freemodel.dev" in DEFAULT_BASE_URL else "http",
    )
).strip().lower()
if TRANSPORT not in {"http", "workbuddy_acp"}:
    raise ValueError(f"Unsupported FREEMODEL_TRANSPORT: {TRANSPORT}")

WORKBUDDY_ACP_URL = str(
    os.environ.get("WORKBUDDY_ACP_URL")
    or load_saved_value("WORKBUDDY_ACP_URL", "http://127.0.0.1:44741")
).rstrip("/")
WORKBUDDY_ACP_PASSWORD = str(
    os.environ.get("WORKBUDDY_ACP_PASSWORD")
    or load_saved_value("WORKBUDDY_ACP_PASSWORD", "")
)
WORKBUDDY_ACP_CWD = str(
    os.environ.get("WORKBUDDY_ACP_CWD")
    or load_saved_value("WORKBUDDY_ACP_CWD", str(Path(__file__).parent))
)
WORKBUDDY_ACP_TIMEOUT = float(
    os.environ.get("WORKBUDDY_ACP_TIMEOUT")
    or load_saved_value("WORKBUDDY_ACP_TIMEOUT", 180)
)
WORKBUDDY_ACP_MAX_ATTEMPTS = int(
    os.environ.get("WORKBUDDY_ACP_MAX_ATTEMPTS")
    or load_saved_value("WORKBUDDY_ACP_MAX_ATTEMPTS", 4)
)

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
