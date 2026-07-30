# Exhaustive Validation and Repair Prompt

## Mission

Validate, diagnose, and repair the entire `freemodel-workbuddy-proxy` project. Do not stop at compilation or the first green test run. Establish an explicit feature inventory, test every externally promised behavior and critical internal invariant, add missing regression coverage, exercise the binary through real HTTP clients and Codex CLI, fix every reproducible defect with the smallest safe change, and rerun the complete matrix until no known failure remains.

The target project is:

```text
/home/potterparker/Desktop/freemodel-workbuddy-proxy
```

Treat the current working tree as user-owned and potentially dirty. Never discard, overwrite, reset, clean, or reformat unrelated pre-existing changes. Inspect `git status` and diffs before editing. Keep all test artifacts in isolated temporary directories unless the test explicitly validates a project-owned path. Never expose API keys, gateway passwords, tokens, private session data, or authorization headers in logs or reports.

## Definition of done

The work is complete only when all of the following are true:

1. The implementation and README feature claims have been mapped to named tests or documented manual checks.
2. Formatting, compilation, Clippy, debug tests, and release tests pass.
3. Every supported HTTP route has positive, negative, malformed-input, boundary, concurrency, and failure-semantic coverage where applicable.
4. Both direct HTTP and WorkBuddy ACP transports have end-to-end coverage with controlled upstreams.
5. Streaming tests prove incremental delivery, exact terminal semantics, malformed-frame handling, premature-EOF handling, cancellation, timeout, and network-error behavior.
6. Proxy session routing, durable storage, sidecar ownership, gateway locking, retries, and concurrency invariants have targeted tests.
7. Responses API translation covers text and function tools in both non-streaming and streaming modes.
8. The Rust TUI state machine, HTTP client, setup, persistence, rendering, cancellation, retry/edit, and safe-exit behavior have unit or integration coverage.
9. The shipped CLI and `start.sh` have smoke tests.
10. Codex CLI can communicate with the local proxy through a controlled test upstream without using or mutating the user's real Codex configuration.
11. Every confirmed defect has a regression test that fails before and passes after the fix.
12. The full matrix is rerun after the final code change, and a final review finds no known untested critical path or unresolved reproducible issue.

## Operating rules

- Begin with read-only inspection. Record the existing Git status and do not alter unrelated files.
- Prefer deterministic local mock servers, temporary HOME/config/session/runtime paths, random loopback ports, and bounded timeouts.
- Do not depend on the public internet for correctness tests.
- Do not contact the protected WorkBuddy service with fabricated private authentication. Test ACP against controlled local fakes; perform a real official-gateway smoke test only if a valid official local gateway is already available and doing so is safe.
- Never terminate a process unless the test created it and ownership is positively proven.
- Run independent test cases in parallel only when they do not share ports, environment variables, files, processes, gateways, or global state.
- Use `serial_test` or equivalent isolation for environment-sensitive tests.
- Assert semantic response content, event order, cardinality, headers, status codes, and cleanup. Do not accept snapshot-only or “did not panic” tests as sufficient for protocol behavior.
- Preserve the first meaningful error. Avoid tests that pass merely because a later generic error masks an earlier protocol violation.
- If a test is flaky, diagnose and remove the nondeterminism; do not add arbitrary sleeps or retries to hide it.
- After every source fix, run the narrow regression first, then all affected suites. After the last fix, run the complete matrix.

## Phase 1: Baseline and feature-to-test traceability

1. Record:
   - `git status --short`
   - compiler, Cargo, Python, and Codex CLI versions
   - relevant environment variables by name only, with secret values redacted
   - existing test files and test names
2. Read and reconcile behavior promised by:
   - `README.md`
   - `Cargo.toml`
   - `src/main.rs`
   - all modules under `src/`
   - `start.sh`
   - the legacy Python implementation as a differential oracle only
3. Build a traceability matrix with columns:
   - feature or invariant
   - implementation file/function
   - existing automated test
   - missing test or manual check
   - result
   - defect/regression reference
4. Identify any README claim not implemented, implemented behavior not documented, configuration field not enforced, or test that asserts weaker semantics than the implementation promises.

## Phase 2: Mandatory static and build checks

Run exactly, preserving full failure output:

```bash
cargo fmt --check
cargo check --all-targets
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
cargo test --release --all-targets
```

Also inspect dependency and target configuration for:

- Rust 1.97 / edition 2024 compatibility.
- Debug and release-only behavior differences.
- Panics, `unwrap`/`expect` on runtime-controlled data, unchecked indexing, blocking work on async executors, leaked tasks, unbounded channels or buffers, unsafe process handling, and secret logging.
- Tests that accidentally use the user's real HOME, config, session store, runtime directory, Codex auth file, clipboard, terminal, or network.
- Module selection and dead-code hazards: verify the compiled module graph is the intended one, especially the coexistence of `src/tui.rs` and `src/tui/mod.rs`; remove, rename, or explicitly justify legacy source files that are not compiled so fixes cannot land in the wrong implementation.
- Resource limits and denial-of-service exposure: request-body limits, upstream non-streaming response limits, SSE line/event/buffer limits, history/title/path limits, diagnostic/log read limits, channel capacities, concurrent request/sidecar limits, and bounded shutdown/cancellation. Add explicit safe limits or document intentional unbounded behavior with rationale.
- Run `cargo metadata --no-deps`, `cargo tree -d`, and `cargo test --doc`; audit duplicate dependencies, feature activation, binaries built as test fixtures, and documentation examples. Use `cargo deny`/`cargo audit` only if already installed or installable without changing project dependencies; otherwise record the check as unavailable rather than silently skipping it.
- Run the relevant test matrix with single-threaded scheduling at least once for environment/process-sensitive suites and run concurrency/stress suites repeatedly with bounded counts to detect hidden order dependence. Do not call nondeterministic repetition a proof of correctness.

## Phase 3: Configuration and CLI

Test `Config` and binary startup exhaustively.

### Configuration sources and validation

- Defaults when environment and config file are absent.
- Config file loading from the intended project path.
- Environment variables override config file values independently.
- Empty strings, whitespace, invalid UTF-8 environment values where representable, unknown transport, invalid URL, unsupported scheme, missing host, invalid port, zero/negative/non-numeric timeouts and limits, overflow, and out-of-range values.
- Exact protected-host enforcement for `https://work.freemodel.dev/v1`, including trailing slash, explicit default port, case normalization, path/query variants, deceptive suffix/prefix hosts, userinfo, subdomains, and lookalike hosts.
- Protected host rejects direct `http` transport and accepts only `workbuddy_acp`.
- API key precedence and fallback to isolated `~/.codex/auth.json` fixtures: valid JSON, malformed JSON, missing fields, wrong types, empty token, unreadable file, and non-UTF-8 content.
- Path expansion/canonicalization for `~`, Unicode, spaces, symlinks, missing paths, non-directory projects, and permission errors.
- CORS origin parsing: empty list, whitespace, duplicate origins, invalid header values, wildcard policy, exact matching, and no accidental credentialed wildcard.

### Key persistence and command-line behavior

- `key set <value>` and masked interactive `key set` behavior.
- Empty/whitespace key rejection.
- Atomic replacement, final file mode `0600`, parent mode `0700` where owned, preserved valid unrelated config fields, valid JSON after interruption simulations, and no key in process listings or output for interactive mode.
- CLI help, version/error output, unknown subcommands, default command behavior, `server`, `tui`, and `key set` exit codes.
- Startup bind failure, invalid configuration, incompatible occupied port, and early child failure must be actionable and non-zero.

## Phase 4: Core HTTP server and middleware

Exercise the real Axum router through loopback sockets and, where necessary, direct service calls with synthetic peer addresses.

### Health and models

- `GET /` and `GET /health`: exact status, JSON content type, service marker, version, transport, upstream identifier, and monotonic/non-hardcoded uptime.
- `GET /v1/models`: OpenAI-compatible list shape, unique IDs, required canonical model, aliases/documented models, deterministic ordering, and no secret/internal gateway fields.
- Unsupported method and unknown route behavior, including `HEAD`, method-not-allowed `Allow` headers where applicable, malformed paths/query strings, and duplicate query parameters.
- Strict media-type and JSON extraction behavior: absent `Content-Type`, `application/json` with parameters/case variants, unsupported media types, invalid charset, empty body, trailing JSON, deeply nested JSON, duplicate JSON keys policy, and oversized/decompression-bomb requests if compression is enabled.
- Request and response size limits must be explicit and tested. Cover request bodies, non-streaming upstream bodies, SSE line/event/aggregate buffers, management history payloads, titles, project/header values, and diagnostics/log reads. An unbounded `to_bytes(..., usize::MAX)`, decoder buffer, or channel is a release-blocking risk unless replaced or explicitly justified with a compensating bound.
- Response headers across success and error paths: content type, cache policy, connection behavior, CORS, `X-WorkBuddy-Session`, and absence of hop-by-hop or upstream-secret headers.
- Request trace/log behavior: unique correlation without trusting spoofed IDs, safe log levels, no control-character/log injection, no full prompts or credentials by default, and meaningful context for failures without exposing secrets.
- Graceful shutdown with idle and active non-streaming/streaming requests: listeners stop accepting, in-flight work follows a documented bounded policy, tasks and locks are released, and test-created sidecars/children are not leaked.

### Authentication and forwarding

- Valid incoming Bearer token is forwarded to direct HTTP upstream exactly once.
- Redirect policy is explicit and safe. Upstream redirects must not forward Authorization or sensitive headers to a different origin, downgrade HTTPS unexpectedly, loop indefinitely, or bypass the configured base-path/host policy. Test same-origin and cross-origin 301/302/303/307/308 responses.
- Sensitive headers other than the intentionally selected authorization value are not forwarded accidentally (`Cookie`, proxy credentials, internal session headers, forwarding headers, or arbitrary client hop-by-hop headers); explicitly document the forwarding allowlist/denylist.
- Missing authorization uses configured upstream API key.
- Common documented dummy tokens do not replace a configured real upstream key.
- Malformed Authorization headers, multiple headers, alternate schemes, empty Bearer, and whitespace are handled deterministically without leaking credentials.
- Verify whether `PROXY_API_KEY` is intended to protect public OpenAI-compatible routes. If documented/configured, test correct key, missing key, wrong key, management route policy, constant-time comparison where appropriate, and no credential reflection. If intentionally unused, remove or document the dead configuration rather than silently claiming protection.
- ACP requests must not imitate private HTTP authentication or leak configured upstream keys into ACP payloads/logs.

### CORS and loopback policy

- Allowed origin preflight and actual requests.
- Disallowed and malformed origins.
- OPTIONS support for GET, POST, PUT, PATCH, DELETE routes and expected allowed headers, including Authorization, Content-Type, `X-WorkBuddy-Session`, and `X-WorkBuddy-Project`.
- `Vary: Origin` correctness for dynamic allowlists.
- Management endpoints allow IPv4 and IPv6 loopback and reject non-loopback peers consistently on every management route and preflight policy as designed.

## Phase 5: OpenAI Chat Completions

### Request validation and normalization

Cover non-object bodies, invalid JSON, missing/empty/non-array `messages`, malformed message objects, missing/invalid roles, missing content, null/string/array content, supported multimodal text blocks, unsupported blocks, tool messages, assistant tool calls, Unicode, very long input, extra fields, and wrong types for `stream`, model, generation parameters, tools, and tool choice.

Verify model aliases (`gpt-4o`, `gpt-4`, `gpt-5.6`, `opencode-default`, absent model, canonical model, unknown model policy) normalize exactly as intended.

### Direct HTTP non-streaming

With a controlled upstream, assert:

- Path, method, headers, body, model normalization, parameters, tools, and authorization forwarding.
- Successful JSON is returned with compatible status and content type.
- Upstream 400/401/403/404/409/429/5xx statuses and OpenAI/non-OpenAI bodies map without becoming assistant text.
- Malformed success JSON, empty body, wrong response shape, timeout, refused connection, connection reset, DNS-like error simulation, and oversized response fail explicitly.
- No retry occurs unless direct HTTP retry behavior is explicitly designed and tested.

### Direct HTTP streaming and generic SSE decoder

Test arbitrary byte boundaries, one-byte chunks, split UTF-8 code points, CRLF and LF, bare CR if supported, UTF-8 BOM policy, comments, blank lines, multiple `data:` lines according to the decoder's supported contract, `event`/`id`/`retry` fields, fields without colons, unknown fields, leading spaces after `data:`, very long lines, empty data, and a final unterminated line. Compare behavior to the SSE contract the proxy claims; if only the OpenAI single-line `data:` subset is supported, document and reject unsupported framing deterministically rather than silently corrupting it. Enforce and test a maximum buffered line/event size.

For valid Chat streams assert:

- HTTP 200 and SSE headers are returned only after request acceptance.
- Deltas arrive incrementally before upstream completion; do not buffer the full response.
- Role-only chunks, empty content, Unicode, usage chunks, tool-call deltas, and finish chunks are forwarded correctly.
- A non-null finish reason is observed before exactly one `[DONE]`.
- No data is emitted after terminal completion.

For invalid streams assert an OpenAI-style SSE error object and no successful `[DONE]`:

- malformed JSON
- non-object payload
- upstream `error` object
- `[DONE]` before finish reason
- missing choices or invalid choices
- duplicate `[DONE]`
- data after `[DONE]`
- finish chunk followed by EOF without `[DONE]` (must fail; a finish reason alone is not the documented completion marker)
- `[DONE]` with no finish chunk
- premature EOF
- network error after zero, one, or many deltas
- timeout before headers and timeout mid-stream
- invalid/truncated UTF-8 according to the intended policy
- valid finish and `[DONE]` in an unterminated final SSE line

Prove pre-stream failures preserve an HTTP error status and mid-stream failures retain HTTP 200 but emit a terminal error event without `[DONE]`.

## Phase 6: Responses API

### Input and tool translation

Test:

- `instructions` string and supported structured forms.
- `input` as string, message array, item array, empty input, invalid types, and mixed supported/unsupported items.
- User, assistant, system/developer-equivalent instructions, function calls, function-call outputs, IDs/call IDs, Unicode and multiline JSON arguments, empty arguments, malformed arguments policy, and ordering.
- Function tool definitions including name, description, JSON schema, strict flag, multiple tools, unsupported tool types, malformed tools, and tool choice.
- Chat-to-Responses output conversion must preserve assistant tool calls, call IDs, function names, arguments, mixed text plus tools, multiple/interleaved tool calls, tool finish reasons, and usage. It must not silently collapse a tool-call completion into an empty text-only message. ACP transport either implements equivalent tool events or explicitly rejects unsupported tool requests before streaming with a clear client error.
- Generation parameters forwarded intentionally and unsupported Responses-only fields handled explicitly.

### Non-streaming response conversion

- Exact Responses object fields: stable IDs, object type, created timestamp, model, status, output items, content parts, finish/status mapping, and usage.
- Text-only completion, empty text, refusal/error representation, function call output, multiple tool calls, text plus tool calls, multiple choices policy, missing choices, malformed tool calls, and invalid usage values.
- Input/output/total token conversion and absent usage behavior.
- Upstream non-2xx, invalid JSON, wrong shape, timeout, and connection errors preserve failure semantics.

### Streaming Responses events

Assert the exact event state machine and monotonically increasing sequence numbers:

1. exactly one `response.created`
2. output item added events as needed
3. content-part and text-delta events for text
4. function-call item and argument-delta/done events for tools
5. required item/content done events
6. exactly one terminal `response.completed` on success or exactly one `response.failed` on failure

Test text deltas, Unicode splits, role-only chunks, empty chunks, usage, tool-call arguments fragmented across arbitrary chunks, interleaved tool indices, multiple tool calls, finish reasons, and terminal metadata.

Failures before the first emitted event should preserve HTTP status where possible. Failures after streaming begins must produce exactly one `response.failed`, never `response.completed`, never a second failure, and no events afterward. Cover malformed JSON, invalid payload, upstream error object, missing finish, `[DONE]` ordering errors, duplicate completion, premature EOF, body-stream network error, and timeout. Ensure terminal events are emitted even if no text delta was produced but the completion is otherwise valid.

## Phase 7: Proxy session management and routing

### Management API

For every route, test success, missing resources, invalid IDs, malformed JSON, wrong content type, unknown fields, wrong field types, empty bodies, oversized values, Unicode, and loopback restriction:

- `GET /proxy/sessions`
- `POST /proxy/sessions`
- `GET /proxy/sessions/{id}`
- `PATCH /proxy/sessions/{id}`
- `DELETE /proxy/sessions/{id}`
- `POST /proxy/sessions/{id}/history`
- `PUT /proxy/sessions/{id}/history`
- `DELETE /proxy/sessions/{id}/history`
- `GET /proxy/diagnostics`

Verify list filtering and ordering, create status, stable response schemas, title trimming/bounds, duplicate explicit IDs, immutable fields, deletion cleanup, append versus replace semantics, clear idempotency, allowed history roles, malformed messages, history bounds by turn pairs, and timestamps.

Diagnostics must report accurate version, increasing uptime, bind URL, transport, redacted upstream host, paths, active/max sidecars, and defensible RSS behavior. It must not disclose API keys, passwords, bearer tokens, or ACP session tokens.

### Durable session store

- Canonical projects with spaces, Unicode, symlinks, `..`, missing path, file path, and permission errors.
- Session ID regex boundaries and path traversal attempts.
- Stable automatic IDs: identical stable context maps identically; project or earliest stable system/developer/user context changes the ID; later conversation turns do not unexpectedly fork the session.
- Atomic writes and valid JSON after injected write/persist failures where feasible, including stale temporary files, lock acquisition failure, disk-full/short-write simulation where practical, and parent-directory sync failure policy.
- Store and lock permissions (`0600`) and parent/runtime permissions (`0700`).
- Corrupt JSON, unknown version/schema, duplicate IDs, missing fields, wrong field types, and recovery policy.
- Cross-instance/process locking, simultaneous create/update/history/delete, no lost updates, no duplicates, no torn files, and deterministic conflict behavior.
- Path-security behavior for the store, lock, runtime, log, config, and preferences files: hostile symlinks, symlink swaps between validation and write, hard links where relevant, pre-existing world-readable files/directories, ownership mismatch, special files/FIFOs, and parent replacement. The proxy must not overwrite or chmod unrelated targets outside its intended path.
- Bounded history and correct retention of complete user/assistant turn pairs, including system/developer/tool messages.

### Request routing

- Explicit valid session plus matching canonical project.
- Explicit session with missing, relative, symlinked, equivalent, or mismatched project.
- Unknown explicit session.
- Header validation, duplicate headers, non-UTF-8 values, whitespace, traversal-like IDs, and case-insensitive HTTP header names.
- Headerless clients use `PROXY_DEFAULT_PROJECT`, derive a stable automatic session, reuse it for stable context, and return `X-WorkBuddy-Session` on streaming and non-streaming responses.
- Concurrent first requests for the same automatic key create one logical session.
- Different stable contexts and different projects remain isolated.
- A failed request must not corrupt or cross-wire another session.

## Phase 8: WorkBuddy ACP transport and gateway locking

Use a controllable fake ACP SSE/RPC server capable of recording requests, delaying phases, emitting malformed frames, closing connections, and simulating multiple gateway candidates.

### Connection and JSON-RPC lifecycle

Validate:

- ACP SSE connection setup and required `acp-connection-id` extraction.
- Optional session token handling without leakage.
- `initialize` request and response.
- `session/new` request, result validation, and missing/invalid session ID.
- `session/prompt` serialization and streamed `session/update` parsing.
- Text deltas, Unicode, multiline content, ignored/recognized update types, JSON-RPC notifications, errors, malformed JSON, non-object payloads, mismatched IDs, and out-of-order messages.
- A terminal `stopReason` is required. Test normal completion, `max_tokens`, cancellation, refusal, error, unknown stop reason, and EOF without stop.
- Connection close occurs on success, failure, timeout, and cancellation.

### Retry policy and candidate discovery

- Retry only retryable network/timeout/capacity/explicit-refusal failures.
- Never retry authentication, permission, protocol, invalid-request/configuration, user cancellation, or `max_tokens` terminal outcomes.
- Retry only before the first downstream response delta.
- No retry after any emitted content, including content emitted immediately before a disconnect.
- Respect exact maximum attempts, including zero/one boundary validation.
- Rotate across live gateway candidates as documented; skip stale registrations, malformed records, dead PIDs, incompatible processes, duplicate URLs, and inaccessible entries.
- Preserve the most useful final error and do not concatenate secrets.

### Cancellation and backpressure

- Downstream disconnect/cancellation before connection, during initialize, during `session/new`, during prompt, after first delta, and near completion.
- Send `session/cancel` exactly when an ACP session exists and cancellation is possible; do not send it before a session exists or after a terminal result.
- Close the ACP connection even if cancel RPC fails or times out.
- Slow downstream consumers must not cause unbounded buffering or cross-session data loss.
- Dropping the downstream HTTP body and dropping the TUI receiver both trigger bounded cleanup; distinguish cancellation caused by the client from an upstream protocol failure in logs and retry decisions.

### Gateway locks and concurrency

- Same normalized gateway URL is serialized across concurrent requests, including cancellation of a waiter before acquisition and cancellation of the current owner while holding the guard.
- Lock acquisition is cancellation-safe and does not leak permits/map entries or panic if a waiting task is dropped. Where fairness is expected, a bounded contention test proves no practical starvation.
- Different gateway URLs run concurrently.
- Lock is released after success, failure, panic/cancellation, and dropped downstream.
- URL normalization policy is explicit for trailing slash/case/default port.
- No prompt/response crossover under high-concurrency stress.
- Weak-lock cleanup does not permit overlapping critical sections and does not leak unbounded map entries.

## Phase 9: Dedicated sidecars

Use fake executable fixtures that can become healthy, delay startup, exit early, ignore termination, log arguments, and emulate unrelated processes. Never target real user processes.

Test:

- Missing/non-file/non-executable CLI path and actionable errors.
- Exact arguments: loopback host, allocated port, proxy-owned session marker, and permission mode.
- Private working/runtime directories and `0600` logs/metadata.
- Successful startup, health verification, metadata persistence, and returned gateway URL.
- Reuse of one healthy sidecar for repeated/concurrent `ensure` calls on a session.
- Separate sessions receive isolated sidecars and ports.
- Maximum-sidecar limit under serial and concurrent startup.
- Idle timeout disabled/zero semantics, idle reaping, active-use protection, metadata retained after reaping, and relaunch on demand.
- Startup timeout kills/reaps only the created child and removes stale metadata.
- Early child exit captures useful status/log path and cleans state.
- Port allocation race, child becomes healthy at timeout boundary, and deletion racing startup.
- PID reuse and stale metadata.
- Ownership verification requires the expected executable/serve mode/exact session marker; unrelated processes are never signaled.
- Stop is idempotent and safe for missing, dead, mismatched, and already-reaped processes.
- Session deletion stops only its positively owned sidecar.
- Manager shutdown/drop behavior does not orphan test-created children.
- Subprocess environment is minimized and tested: no proxy API key, ACP password, authorization header, unexpected inherited secret variables, or unsafe dynamic-loader variables are passed unless strictly required. Arguments, cwd, stdio, and logs cannot be confused by spaces, Unicode, leading dashes, or control characters.
- Linux `/proc` process matching and signal behavior are tested where supported; non-Linux behavior is either implemented behind explicit platform abstractions or the Linux-only support contract is documented and enforced at build/startup time.

## Phase 10: Rust TUI

### Pure state machine and commands

Cover every `Action`, `Effect`, `Modal`, command, and shortcut, including invalid-state transitions and late events:

- Empty/whitespace submission, regular and multiline Unicode messages, command detection, quoting, escaping, incomplete escape, unclosed quotes, unknown command, completions, case normalization, and extra arguments.
- Connecting, streaming, completion, failure, cancellation, stale request IDs, duplicate terminal events, deltas after terminal, and metrics (TTFB, total, delta count, UTF-8 byte count).
- Busy-state edit restrictions and no second simultaneous send.
- Retry and edit/resend replace the previous transcript turn without duplication, including after failed/cancelled/unsaved states.
- Only complete messages are persisted; history save success/failure and safe-quit confirmation behave correctly.
- Session/model/project picker empty lists, selection bounds, typed custom path, invalid project, cross-project session rejection, rename, clear, delete, and fallback session creation.
- All destructive actions require confirmation and Esc cancels without effect.
- Search set/clear and case-insensitive highlighting.
- Notification queue bounds.
- Sidebar/color settings and recent session/model persistence.
- Key modal never renders the secret and empty key changes nothing.
- Clipboard success/failure through an injectable or safely isolated abstraction; `last` and `all` semantics.

### Composer, input, rendering, and terminal safety

- Character-safe insert/delete/backspace for combining marks, emoji sequences, CJK, RTL text, tabs, and newlines according to intended grapheme policy.
- Cursor home/end/up/down across empty/short/long lines and soft wrapping; no panic at boundaries.
- Key press/repeat/release filtering; Ctrl/Alt/Shift combinations; mouse scroll; modal-specific input.
- Rendering at zero, tiny, narrow, wide, and very large sizes; composer height must remain valid for both terminal dimensions and never create layout constraints whose fixed rows exceed the available height.
- Property/fuzz-style bounded tests exercise random valid Unicode input operations, resize sequences, scroll/modal transitions, command strings, SSE chunk partitions, and app actions. They must be reproducible from a printed seed and never use unbounded cases.
- Long projects/titles/models, long unbroken content, many messages, high scroll values, all modals, no-color mode, and Unicode do not panic or corrupt layout.
- Sanitize ESC, C0/C1 controls, OSC/CSI fragments, BEL, backspace, carriage return, and malicious content from upstream messages, errors, notifications, paths, titles, and logs while preserving safe newlines/tabs.
- Terminal raw mode, alternate screen, mouse capture, and cursor are restored on normal exit, error, and panic. Avoid globally stacking panic hooks on repeated entry.

### TUI HTTP client and setup integration

- Health compatibility rejects unrelated services even if they return HTTP 200.
- All client methods use correct routes, query encoding, headers, JSON, status handling, and response validation.
- Stream client handles arbitrary chunks, final unterminated lines, malformed/error SSE, missing finish, cancellation before connect and mid-stream, dropped UI receiver, timeouts, and exact single terminal event.
- Setup reuses a compatible server, rejects an incompatible occupied port, launches the current binary's `server` subcommand, detects early exit, kills/reaps on startup timeout, and writes actionable logs.
- `0.0.0.0` binds are contacted through loopback; IPv6/host edge cases follow a documented policy.
- Base URL construction and query encoding work for IPv6 literals, trailing slashes, percent-encoded project names, `+`, `&`, `?`, `#`, Unicode, and very long but allowed values.
- Guided project/session selection validates numeric bounds and preferred session fallback.
- Preferences malformed/unknown-version fallback, truncation, atomic save, permissions, and absence of secrets.
- Leaving the TUI intentionally detaches or terminates a TUI-started proxy according to documented behavior; test for accidental orphaning and document the chosen lifecycle.

## Phase 11: Shell launcher and legacy compatibility

Test `start.sh` in an isolated copy/environment:

- Existing release binary versus absent binary.
- Build failure propagates non-zero.
- Default launches `tui`; `--server-only` launches `server`.
- Extra/unknown arguments policy, including arguments after `--server-only`; no option may be silently discarded unless documented.
- Paths containing spaces and invocation from another working directory.
- Existing but stale/wrong-version/wrong-architecture release binaries: define whether the launcher rebuilds based on timestamps/version or intentionally trusts any executable, then test/document that policy.
- Signals and exit codes are propagated by `exec`.

Use the Python implementation only as a differential oracle for stable public behavior. Compare route status codes, response envelopes, model aliases, validation, session API schemas, and representative stream terminal semantics. Where Rust intentionally differs, document the reason and ensure README/tests agree. Do not preserve a Python bug merely for parity.

## Phase 12: Real process and Codex CLI smoke tests

Run these tests only after deterministic suites pass.

1. Start a controlled mock OpenAI-compatible upstream on loopback.
2. Start the compiled Rust proxy on a random free loopback port with temporary config, HOME, session store, runtime directory, and project.
3. Confirm readiness and capture redacted logs.
4. Exercise `/v1/models`, non-streaming and streaming `/v1/chat/completions`, `/v1/responses`, automatic session routing, explicit session routing, and management CRUD with an external HTTP client.
5. Run Codex CLI version `0.146.0` or the installed version in non-interactive mode against the local proxy. Use a temporary `CODEX_HOME`/HOME or supported command-line overrides so the user's real Codex configuration and history are untouched. Configure the OpenAI-compatible base URL to the proxy, use a dummy local test key, choose a supported model, disable unrelated network integrations if possible, and request a deterministic sentinel response from the mock upstream. Before relying on Codex for repair, probe it once and treat account quota/authentication failure as an environmental blocker, not a project defect; continue the deterministic local matrix independently.
6. Assert:
   - Codex reaches the local proxy.
   - the proxy receives the expected endpoint, model, request body, and authorization policy.
   - streamed text reaches Codex intact.
   - process exits successfully within a bounded timeout.
   - no real external endpoint was contacted.
   - no user config/session files changed.
7. Repeat with an upstream error and truncated stream; Codex must fail clearly and the proxy must preserve its documented failure semantics.
8. If Codex cannot send session/project headers, verify automatic routing and returned session behavior from proxy logs/management state without relying on Codex to expose response headers.

If a live official WorkBuddy sidecar is safely available, perform one explicitly separated smoke test using a newly created proxy-owned session and innocuous prompt. Verify cleanup and do not treat live service availability as a deterministic test prerequisite.

## Phase 13: Repair loop

For every failure:

1. Reproduce it with the smallest deterministic test.
2. Classify it as implementation defect, test defect, documentation mismatch, environmental limitation, or unsupported expectation.
3. For an implementation defect, add or tighten a regression test first when practical.
4. Make the smallest coherent fix. Do not refactor unrelated code.
5. Run formatter and the narrow failing suite.
6. Run all directly affected suites.
7. Inspect the diff for accidental behavior changes, secret exposure, unsafe process handling, and weakened assertions.
8. Repeat until all known deterministic failures are resolved.

Never “fix” a test by weakening required semantics, adding unbounded sleeps, ignoring errors, accepting duplicate/missing terminal events, or replacing precise assertions with broad success checks.

## Phase 14: Final regression and adversarial re-review

After the last edit, rerun:

```bash
cargo fmt --check
cargo check --all-targets
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
cargo test --release --all-targets
```

Then rerun the end-to-end HTTP and Codex CLI smoke tests. Review all changed source and tests from scratch and answer:

- Is any route, config field, command, modal, transport phase, error class, or terminal state still untested?
- Can any malformed/truncated stream be mistaken for success?
- Can a Responses stream produce zero or multiple terminal events?
- Can retry occur after content was emitted?
- Can cancellation fail to close ACP or leave a lock/sidecar behind?
- Can two sessions or requests cross-wire content?
- Can a session update be lost under process concurrency?
- Can an unrelated process be signaled?
- Can secrets enter output, logs, diagnostics, preferences, panic messages, or test artifacts?
- Can the TUI leave the terminal corrupted?
- Can Codex CLI use the proxy without mutating real user state?
- Do README claims and actual behavior now agree?
- Are there any release-blocking unbounded allocations, bodies, SSE buffers, channels, logs, or histories?
- Can redirect handling leak credentials or escape the configured upstream origin?
- Can tool calls be silently dropped or converted into an apparently successful empty text response?
- Can legacy/uncompiled source files mislead future maintenance or receive fixes that never ship?
- Are all environmental blockers (including Codex quota/auth, live official gateway availability, terminal/clipboard access, security-audit tooling, and platform-only checks) explicitly distinguished from passing tests?

## Required final report

Produce a concise but complete report containing:

1. Environment and exact commands run.
2. Feature-to-test traceability summary.
3. Baseline failures.
4. Confirmed defects with severity, root cause, affected files/functions, regression test, and fix.
5. All source/test/documentation files changed.
6. Final debug/release/static-check results with test counts.
7. Direct HTTP, ACP fake-server, process-level, TUI, and Codex CLI smoke results.
8. Any live-service checks clearly separated from deterministic local validation.
9. Residual risks, skipped checks, or environmental blockers. Never claim “perfect” if any item remains unverified.
10. Final Git diff/status summary that distinguishes pre-existing changes from changes made during this validation.
