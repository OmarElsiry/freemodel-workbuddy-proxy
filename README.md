# ⚡ Freemodel API Proxy & Interactive TUI

A high-performance, OpenAI-compatible proxy server and Terminal User Interface (TUI) for **Codex App**, **OpenCode**, **Cursor**, **Continue**, and standard OpenAI clients bridging to the Freemodel API.

---

## 🌟 Features

- ⚡ **OpenAI Compatibility**: Emulates `/v1/chat/completions`, `/v1/models`, and `/v1/responses`.
- 💬 **Interactive TUI**: Included terminal chat client (`tui.py`) with rich UI, live response streaming, and interactive key setup.
- 🔑 **Automatic Key Resolution**: Auto-detects and persists API keys locally in `config.json` or reads existing keys from `~/.codex/auth.json`.
- 🔄 **Native Incremental Streaming**: Forwards direct HTTP and WorkBuddy ACP deltas immediately for both Chat Completions and Responses API clients.
- 🧰 **Function Tool Translation**: Converts Responses function definitions, calls, and outputs to and from Chat Completions tool-call format.
- 🛡️ **Explicit Stream Failures**: Reports truncated upstream streams as `response.failed` instead of silently closing before completion.
- 🔐 **Official WorkBuddy ACP Transport**: Routes the protected `work.freemodel.dev` endpoint through an active official WorkBuddy gateway instead of imitating private client authentication.
- 🧭 **Dynamic Gateway Discovery**: Finds live gateways from `~/.workbuddy-ai/sessions`, skips stale registrations, and rotates retryable failures across candidates.
- 🔒 **Concurrent Request Isolation**: Serializes ACP sessions per gateway to prevent prompt/response crossover while allowing separate gateways to operate concurrently.
- 🧩 **Proxy-Owned Sessions**: Keeps terminal and Codex conversations in a separate proxy store instead of reusing the active WorkBuddy GUI conversation.
- 📁 **Project-Aware TUI**: Select a project, create a fresh proxy session, or reopen an older proxy-only session with saved history.
- 🛰️ **Dedicated Official Sidecars**: Starts one loopback-only official CodeBuddy CLI gateway per active proxy session, stops it after an idle timeout, and relaunches it on demand.

---

## 🔀 Transport Configuration

The proxy supports two transports:

- `http`: Direct OpenAI-compatible HTTP, used by the public Freemodel endpoint and other generic upstreams.
- `workbuddy_acp`: Official WorkBuddy ACP, required for the protected `https://work.freemodel.dev/v1` endpoint. Startup fails if this host is paired with `http` transport.

Example ignored local `config.json`:

```json
{
  "FREEMODEL_BASE_URL": "https://work.freemodel.dev/v1",
  "FREEMODEL_TRANSPORT": "workbuddy_acp"
}
```

For the protected endpoint, the proxy launches the bundled official CodeBuddy CLI as a dedicated local gateway for each active proxy session. This prevents proxy traffic from being attached to the WorkBuddy GUI gateway and conversation. WorkBuddy account access must already be available to the official CLI; never copy or imitate private HTTP authentication.

Optional ACP settings are `WORKBUDDY_ACP_TIMEOUT` and `WORKBUDDY_ACP_MAX_ATTEMPTS`. Session-isolation settings are:

- `WORKBUDDY_CLI_PATH`: path to the official bundled `codebuddy` executable.
- `PROXY_DEFAULT_PROJECT`: fallback project for clients that cannot send a project header.
- `PROXY_SESSION_STORE`: proxy-owned JSON metadata and TUI history store.
- `PROXY_RUNTIME_DIR`: sidecar logs and runtime files.
- `PROXY_SIDECAR_STARTUP_TIMEOUT`: maximum sidecar startup wait.
- `PROXY_SIDECAR_IDLE_TIMEOUT`: seconds before an inactive sidecar is stopped; its session metadata remains reusable.
- `PROXY_MAX_HISTORY_TURNS`: number of user/assistant turn pairs retained for the TUI.

`WORKBUDDY_ACP_URL`, `WORKBUDDY_ACP_CWD`, and `WORKBUDDY_ACP_PASSWORD` remain available for legacy/manual ACP use, but normal protected-host requests are resolved to a proxy-owned sidecar. Never commit gateway passwords or API keys.

### Reliability and error semantics

- Authentication and protocol/configuration errors stop immediately; transient network, timeout, capacity, and explicit refusal failures can be retried within `WORKBUDDY_ACP_MAX_ATTEMPTS`, but only before the first response delta has been sent.
- User cancellation and `max_tokens` terminal results are not automatically retried.
- Cancelling a downstream request sends `session/cancel` when an ACP session exists, then closes the ACP connection.
- Errors detected before streaming preserve their HTTP status where available instead of being presented as successful assistant text.
- Mid-stream Chat failures are emitted as an OpenAI-style SSE `error` object and do not emit `[DONE]` as success.
- Mid-stream Responses failures emit exactly one `response.failed`; successful Responses streams emit exactly one `response.completed`.
- Malformed JSON, invalid SSE payloads, premature EOF, or `[DONE]` without a finish reason are treated as explicit failures.

---

## 🧩 Proxy Session Isolation

The proxy session ID is independent from WorkBuddy GUI conversation IDs. Session records are stored only in `PROXY_SESSION_STORE`; the proxy does not edit `~/.workbuddy-ai/app/sessions.json` and never terminates GUI-managed processes.

### TUI workflow

Running `python3 tui.py` now:

1. asks for a project directory;
2. lists only proxy-owned sessions for that project;
3. lets you create a new session or reopen an old one;
4. restores the selected session's saved TUI history;
5. routes requests through that session's dedicated sidecar.

Sidecar processes are temporary. An idle sidecar is stopped after `PROXY_SIDECAR_IDLE_TIMEOUT`, while the session title, project, and history stay available. Selecting or addressing that session again starts a new sidecar automatically. The proxy writes session metadata and sidecar log files with owner-only permissions (`0600`) and keeps runtime directories private (`0700`).

### Codex and OpenAI-compatible clients

Use the normal base URL:

```text
http://127.0.0.1:40589/v1
```

For deterministic routing, send both headers:

```http
X-WorkBuddy-Session: proxy-<session-id>
X-WorkBuddy-Project: /absolute/path/to/project
```

Create and inspect proxy-only sessions through the loopback management API:

```bash
curl -sS -X POST http://127.0.0.1:40589/proxy/sessions \
  -H "Content-Type: application/json" \
  -d '{"project":"/absolute/path/to/project","title":"Codex work"}'

curl -sS "http://127.0.0.1:40589/proxy/sessions?project=/absolute/path/to/project"
```

If a client cannot set custom headers, omit them. The proxy derives a stable automatic session from the canonical project and the earliest system/developer/user context, and returns the resolved ID in `X-WorkBuddy-Session`. Set `PROXY_DEFAULT_PROJECT` correctly for headerless clients. For reliable resume behavior across client restarts, explicit session headers are preferred.

Management routes are loopback-only:

- `GET /proxy/sessions`
- `POST /proxy/sessions`
- `GET /proxy/sessions/{session_id}`
- `POST /proxy/sessions/{session_id}/history`
- `DELETE /proxy/sessions/{session_id}`

Deleting a session stops only a process whose PID and command line match the proxy-owned sidecar marker.

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

Run the isolated unit and protocol suite:

```bash
python3 -m unittest discover -s . -p "test_*.py" -v
```

With a proxy already running on port `40589`, optional live endpoint checks are:

```bash
python3 test_proxy.py
python3 test_all_edge_cases.py
```
