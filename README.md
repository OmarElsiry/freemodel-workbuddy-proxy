# ⚡ Freemodel API Proxy & Interactive TUI

A high-performance, OpenAI-compatible proxy server and Terminal User Interface (TUI) for **Codex App**, **OpenCode**, **Cursor**, **Continue**, and standard OpenAI clients bridging to the Freemodel API.

---

## 🌟 Features

- ⚡ **OpenAI Compatibility**: Emulates `/v1/chat/completions`, `/v1/models`, and `/v1/responses`.
- 💬 **Interactive TUI**: Included terminal chat client (`tui.py`) with rich UI, live response streaming, and interactive key setup.
- 🔑 **Automatic Key Resolution**: Auto-detects and persists API keys locally in `config.json` or reads existing keys from `~/.codex/auth.json`.
- 🔄 **Auto-Fallback & Stream Handling**: Handles non-streaming and Server-Sent Events (SSE) streaming seamlessly.

---

## 🚀 Quick Start

### 1. Launch Interactive TUI & Proxy
```bash
./start.sh
```
*or*
```bash
python3 tui.py
```

### 2. Run Server Only (Background or Foreground)
```bash
# Foreground
python3 proxy_server.py

# Or via start script
./start.sh --server-only
```

---

## 🔌 Connecting Your Apps (Cursor, Codex, Continue, OpenCode)

- **Base URL**: `http://localhost:40589/v1`
- **API Key**: `fe_oa_...` (or any dummy key `sk-dummy` if configured in `config.json`)
- **Supported Models**: `gpt-5.6-sol`, `gpt-4o`, `opencode-default`

---

## 🧪 Testing

To test all edge cases and endpoints:
```bash
python3 test_proxy.py
python3 test_all_edge_cases.py
```
