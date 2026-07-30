# ⚡ Freemodel API Proxy & Interactive TUI

A high-performance, OpenAI-compatible proxy server and Terminal User Interface (TUI) for **Codex App**, **OpenCode**, **Cursor**, **Continue**, and standard OpenAI clients bridging to the Freemodel API.

---

## 🌟 Features

- ⚡ **OpenAI Compatibility**: Emulates `/v1/chat/completions`, `/v1/models`, and `/v1/responses`.
- 💬 **Interactive Rust TUI**: Full-screen Ratatui workspace with guided setup, validated live streaming, session/model/project pickers, retry/edit/cancel controls, search, diagnostics, logs, preferences, and masked key setup.
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
- `PROXY_API_KEY`: optional Bearer key required by `/v1/models`, `/v1/chat/completions`, and `/v1/responses`. Loopback-only management and health routes remain available locally. When enabled, the proxy uses `FREEMODEL_API_KEY` for the direct upstream instead of forwarding the proxy credential.

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

Running `./start.sh` opens the Rust hybrid TUI:

1. verifies or starts the compatible local Rust proxy and reports actionable startup failures;
2. asks for a project directory with recent-project choices;
3. lists only proxy-owned sessions for that project and rejects invalid selections;
4. lets you create a new session or reopen an old one;
5. restores saved history and routes through the selected session's dedicated sidecar;
6. opens a full-screen, resize-safe chat workspace with multiline input, validated streaming, cancellation, retry/edit-resend, transcript search, model and project selection, diagnostics, a bounded sanitized proxy-log view, and persisted non-secret preferences;
7. uses full-transcript replacement after retry/edit so corrected turns replace saved history rather than duplicating old turns.

Press `F1` or type `/help` for all shortcuts and commands. Common actions include `Ctrl+O` session picker, `Ctrl+P` project switch, `Ctrl+M` model picker, `Ctrl+R` retry, `Ctrl+E` edit/resend, `Esc` cancel/close, and `Ctrl+Q` safe exit. Session commands include `/new`, `/sessions`, `/switch`, `/rename`, `/clear`, and `/delete`; destructive commands require confirmation.

Sidecar processes are temporary. An idle sidecar is stopped after `PROXY_SIDECAR_IDLE_TIMEOUT`, while the session title, project, and history stay available. Selecting or addressing that session again starts a new sidecar automatically. The proxy writes session metadata and sidecar log files with owner-only permissions (`0600`), keeps runtime directories private (`0700`), and launches each sidecar with a minimized environment that excludes proxy credentials, provider API keys, gateway passwords, and dynamic-loader injection variables.

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
- `PATCH /proxy/sessions/{session_id}`
- `POST /proxy/sessions/{session_id}/history`
- `PUT /proxy/sessions/{session_id}/history`
- `DELETE /proxy/sessions/{session_id}/history`
- `DELETE /proxy/sessions/{session_id}`
- `GET /proxy/diagnostics`

Deleting a session stops only a process whose PID and command line match the proxy-owned sidecar marker.

---

## 🚀 Quick Start

### 1. Launch the hybrid TUI and proxy

```bash
./start.sh
```

Or build and run it directly:

```bash
cargo run --release -- tui
```

### 2. Run the server only

```bash
./start.sh --server-only
# or
cargo run --release -- server
```

Configure the API key without exposing it as a process argument:

```bash
cargo run --release -- key set
```

---

## 🔌 Connecting Your Apps (Cursor, Codex, Continue, OpenCode)

- **Base URL**: `http://localhost:40589/v1` (use `/v1`, not `/v1/chat/completions`)
- **API Key**: `fe_oa_...` (or any dummy key `sk-dummy` if configured in `config.json`)
- **Supported Models**: `gpt-5.6-sol`, `gpt-4o`, `opencode-default`

---

## 🧪 Testing

Run the Rust unit, protocol, API, and TUI suite:

```bash
cargo fmt --check
cargo check --all-targets
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
cargo test --release --all-targets
```

The Python implementation remains temporarily available only as a differential compatibility oracle until the wider Rust migration reaches final cutover.
