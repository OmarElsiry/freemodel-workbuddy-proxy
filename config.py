"""Configuration for the Freemodel WorkBuddy API Proxy."""

import os
import json
import shutil
from pathlib import Path
from urllib.parse import urlparse

CONFIG_FILE = Path(__file__).parent / "config.json"
DEFAULT_WORKBUDDY_BASE_URL = "https://work.freemodel.dev/v1"


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

def upstream_hostname(base_url: str) -> str:
    parsed = urlparse(base_url)
    if parsed.scheme not in {"http", "https"} or not parsed.hostname:
        raise ValueError(f"Invalid FREEMODEL_BASE_URL: {base_url}")
    return parsed.hostname.lower()


def is_protected_workbuddy_url(base_url: str) -> bool:
    return upstream_hostname(base_url) == "work.freemodel.dev"


DEFAULT_BASE_URL = (
    os.environ.get("FREEMODEL_BASE_URL")
    or load_saved_base_url()
    or DEFAULT_WORKBUDDY_BASE_URL
).rstrip("/")
DEFAULT_API_KEY = os.environ.get("FREEMODEL_API_KEY") or load_saved_key()

TRANSPORT = str(
    os.environ.get("FREEMODEL_TRANSPORT")
    or load_saved_value(
        "FREEMODEL_TRANSPORT",
        "workbuddy_acp" if is_protected_workbuddy_url(DEFAULT_BASE_URL) else "http",
    )
).strip().lower()
if TRANSPORT not in {"http", "workbuddy_acp"}:
    raise ValueError(f"Unsupported FREEMODEL_TRANSPORT: {TRANSPORT}")
if is_protected_workbuddy_url(DEFAULT_BASE_URL) and TRANSPORT != "workbuddy_acp":
    raise ValueError(
        "https://work.freemodel.dev requires FREEMODEL_TRANSPORT=workbuddy_acp"
    )

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
if WORKBUDDY_ACP_TIMEOUT <= 0:
    raise ValueError("WORKBUDDY_ACP_TIMEOUT must be greater than zero")
if WORKBUDDY_ACP_MAX_ATTEMPTS < 1:
    raise ValueError("WORKBUDDY_ACP_MAX_ATTEMPTS must be at least one")

PROJECT_ROOT = Path(__file__).parent
_configured_codebuddy = str(
    os.environ.get("WORKBUDDY_CLI_PATH")
    or load_saved_value("WORKBUDDY_CLI_PATH", "")
).strip()
WORKBUDDY_CLI_PATH = str(
    Path(_configured_codebuddy).expanduser()
    if _configured_codebuddy
    else Path(shutil.which("codebuddy") or "codebuddy")
)
PROXY_SESSION_STORE = str(
    Path(
        os.environ.get("PROXY_SESSION_STORE")
        or load_saved_value("PROXY_SESSION_STORE", str(PROJECT_ROOT / ".proxy-sessions.json"))
    ).expanduser()
)
PROXY_RUNTIME_DIR = str(
    Path(
        os.environ.get("PROXY_RUNTIME_DIR")
        or load_saved_value("PROXY_RUNTIME_DIR", str(PROJECT_ROOT / ".proxy-runtime"))
    ).expanduser()
)
PROXY_DEFAULT_PROJECT = str(
    Path(
        os.environ.get("PROXY_DEFAULT_PROJECT")
        or load_saved_value("PROXY_DEFAULT_PROJECT", WORKBUDDY_ACP_CWD)
    ).expanduser()
)
PROXY_SIDECAR_STARTUP_TIMEOUT = float(
    os.environ.get("PROXY_SIDECAR_STARTUP_TIMEOUT")
    or load_saved_value("PROXY_SIDECAR_STARTUP_TIMEOUT", 30)
)
PROXY_SIDECAR_IDLE_TIMEOUT = float(
    os.environ.get("PROXY_SIDECAR_IDLE_TIMEOUT")
    or load_saved_value("PROXY_SIDECAR_IDLE_TIMEOUT", 900)
)
PROXY_MAX_HISTORY_TURNS = int(
    os.environ.get("PROXY_MAX_HISTORY_TURNS")
    or load_saved_value("PROXY_MAX_HISTORY_TURNS", 100)
)
if PROXY_SIDECAR_STARTUP_TIMEOUT <= 0:
    raise ValueError("PROXY_SIDECAR_STARTUP_TIMEOUT must be greater than zero")
if PROXY_SIDECAR_IDLE_TIMEOUT < 0:
    raise ValueError("PROXY_SIDECAR_IDLE_TIMEOUT must not be negative")
if PROXY_MAX_HISTORY_TURNS < 1:
    raise ValueError("PROXY_MAX_HISTORY_TURNS must be at least one")

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
DEFAULT_HOST = os.environ.get("PROXY_HOST", "0.0.0.0").strip()
if not 1 <= DEFAULT_PORT <= 65535:
    raise ValueError("PROXY_PORT must be between 1 and 65535")
if not DEFAULT_HOST:
    raise ValueError("PROXY_HOST must not be empty")
