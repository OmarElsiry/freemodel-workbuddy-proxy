# ⚡ Freemodel API Proxy & Interactive TUI

A high-performance, OpenAI-compatible proxy server and Terminal User Interface (TUI) for **Codex App**, **OpenCode**, **Cursor**, **Continue**, and standard OpenAI clients bridging to the Freemodel API.

---

## 🌟 Features

- ⚡ **OpenAI Compatibility**: Emulates `/v1/chat/completions`, `/v1/models`, and `/v1/responses`.
- 💬 **Interactive TUI**: Included terminal chat client (`tui.py`) with rich UI, live response streaming, and interactive key setup.
- 🔑 **Automatic Key Resolution**: Auto-detects and persists API keys locally in `config.json` or reads existing keys from `~/.codex/auth.json`.
- 🔄 **Native Responses Streaming**: Translates Chat Completions SSE into Responses API events, including the terminal `response.completed` event required by Codex.
- 🧰 **Function Tool Translation**: Converts Responses function definitions, calls, and outputs to and from Chat Completions tool-call format.
- 🛡️ **Explicit Stream Failures**: Reports truncated upstream streams as `response.failed` instead of silently closing before completion.
- 🔐 **Official WorkBuddy ACP Transport**: Routes the protected `work.freemodel.dev` endpoint through an active official WorkBuddy gateway instead of imitating private client authentication.
- 🧭 **Dynamic Gateway Discovery**: Finds live gateways from `~/.workbuddy-ai/sessions`, skips stale registrations, and rotates retryable failures across candidates.
- 🔒 **Concurrent Request Isolation**: Serializes ACP sessions per gateway to prevent prompt/response crossover while allowing separate gateways to operate concurrently.

---

## 🔀 Transport Configuration

The proxy supports two transports:

- `http`: Direct OpenAI-compatible HTTP, used by the public Freemodel endpoint and other generic upstreams.
- `workbuddy_acp`: Official WorkBuddy ACP, required for the protected `https://work.freemodel.dev/v1` endpoint.

Example ignored local `config.json`:

```json
{
  "FREEMODEL_BASE_URL": "https://work.freemodel.dev/v1",
  "FREEMODEL_TRANSPORT": "workbuddy_acp"
}
```

Start WorkBuddy before the proxy. Supply gateway authentication only through the environment:

```bash
export WORKBUDDY_ACP_PASSWORD="$CODEBUDDY_GATEWAY_PASSWORD"
python3 proxy_server.py
```

Optional settings are `WORKBUDDY_ACP_URL`, `WORKBUDDY_ACP_CWD`, `WORKBUDDY_ACP_TIMEOUT`, and `WORKBUDDY_ACP_MAX_ATTEMPTS`. A live discovered gateway takes precedence over a stale configured URL. Never commit gateway passwords or API keys.

### Reliability and error semantics

- Authentication and protocol/configuration errors stop immediately; transient network, timeout, capacity, and explicit refusal failures can be retried within `WORKBUDDY_ACP_MAX_ATTEMPTS`.
- User cancellation and `max_tokens` terminal results are not automatically retried.
- Cancelling a downstream request sends `session/cancel` when an ACP session exists, then closes the ACP connection.
- Errors detected before streaming preserve their HTTP status where available instead of being presented as successful assistant text.
- Mid-stream Chat failures are emitted as an OpenAI-style SSE `error` object and do not emit `[DONE]` as success.
- Mid-stream Responses failures emit exactly one `response.failed`; successful Responses streams emit exactly one `response.completed`.
- Malformed JSON, invalid SSE payloads, premature EOF, or `[DONE]` without a finish reason are treated as explicit failures.

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

- **Base URL**: `http://localhost:40589/v1` (use `/v1`, not `/v1/chat/completions`)
- **API Key**: `fe_oa_...` (or any dummy key `sk-dummy` if configured in `config.json`)
- **Supported Models**: `gpt-5.6-sol`, `gpt-4o`, `opencode-default`

---

## 🧪 Testing

To test all edge cases and endpoints:
```bash
python3 test_proxy.py
python3 test_all_edge_cases.py
```
