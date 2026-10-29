# HARNESS_MATRIX.md — Production Qualification Matrix & Harness Baselines

This document establishes the verified baseline for **CW-110 (Production-Qualified Harness Integrations)** across the three primary target harnesses: **Codex CLI**, **Hermes Agent**, and **OMP (Oh My Pi)**.

Qualification requires proving that contextWatcher (`cw`) observes the correct native session, optimizes the context actually transmitted to the model, preserves agent tool-calling functionality, and survives multi-turn sessions, restarts, and errors without data loss or credential leakage.

---

## 1. The 10 Acceptance Criteria Matrix

| # | Acceptance Criterion | Required Invariant | Codex CLI (`0.165.0`) | Hermes Agent (`0.22.5`) | OMP / Oh My Pi (`18.12.6`) |
|---|---|---|:---:|:---:|:---:|
| **AC1** | **Session Identity & Binding** | Correct session bound across multi-turn and resume without crossover | **Verified** 🟢<br>`x-cw-session-id` header via custom model provider | **Blocked** 🔴<br>Single-session works; concurrent launches collide on `hermes-session-override.json` | **Partial** 🟡<br>Passive observation works; no live authenticated binding in baseline |
| **AC2** | **Committed Overlay Reapplication** | Overlays committed on turn $N$ apply to outbound model payload on turn $N+1$ | **Verified** 🟢<br>Applied via `/openai/v1/responses` wire | **Partial** 🟡<br>Applied via `llm_request` when bound, but unstable under concurrency | **Blocked** 🔴<br>No live proxy mutation in baseline (`cw wrap` strips auth credentials) |
| **AC3** | **Zero Duplicate Injection** | Overlays applied exactly once across turns $N, N+1, N+2$; no duplicate accumulation | **Verified** 🟢<br>Tested across turns 1..3 with 0 duplicate pins | **Verified** 🟢<br>Overlay deduplicated by `apply_mutations_to_payload` | **Verified** 🟢<br>Deduplication verified in canonical engine |
| **AC4** | **Tool-Call / Result Pairing** | Tool call IDs and tool result pairings strictly preserved in sequence | **Verified** 🟢<br>Responses API output items & function calls intact | **Verified** 🟢<br>Chat completion tool calls intact | **Verified** 🟢<br>Chat completion tool results intact |
| **AC5** | **Signed / Unsupported Shape Passthrough** | Cryptographic thinking signatures & unparseable shapes forwarded with 100% byte fidelity | **Verified** 🟢<br>Unmodified passthrough | **Verified** 🟢<br>Unmodified passthrough | **Verified** 🟢<br>Unmodified passthrough |
| **AC6** | **Credential Stripping** | Control tokens stripped from upstream requests and inaccessible to model tool commands | **Verified** 🟢<br>Stripped before upstream; excluded from tools via `shell_environment_policy` | **Partial** 🟡<br>Stripped before upstream; `CW_CONTROL_TOKEN_FILE` is a disk file | **Verified** 🟢<br>Stripped before upstream; `cw wrap` removes env tokens |
| **AC7** | **Pristine Source Transcripts** | Harness on-disk transcripts and state databases are never corrupted or rewritten in place | **Verified** 🟢<br>`--ephemeral` & disk session files untouched | **Verified** 🟢<br>`~/.hermes/` session state DB untouched | **Verified** 🟢<br>`~/.omp/agent/sessions/` untouched |
| **AC8** | **Safe Fault Recovery** | Malformed mutations or daemon outages fail open or safe without corrupting agent session | **Verified** 🟢<br>Bad request rejected with HTTP 400; healthy turns succeed | **Verified** 🟢<br>Fails safe to unmutated payload on error | **Verified** 🟢<br>Fails safe to unmutated payload on error |
| **AC9** | **Live Binary Task Continuation** | Actual agent binary completes prompt/task against isolated proxy | **Verified** 🟢<br>`codex exec` completes code 0 in automated test | **Verified** 🟢<br>`hermes chat` completes code 0 in automated test | **Verified** 🟢<br>`omp -p` completes code 0 in automated test |
| **AC10** | **Deterministic Savings Accounting** | Token and latency savings measured from real usage telemetry; no fabricated cache hits | **Verified** 🟢<br>Exact token accounting; unknown cache preserved | **Verified** 🟢<br>Exact token accounting; unknown cache preserved | **Verified** 🟢<br>Exact token accounting; unknown cache preserved |

---

## 2. Per-Harness Analysis & Blockers

### 2.1 Codex CLI (`0.165.0`) — Reference Architecture
* **Status:** **Contract Verified (Mock Provider)** · Ready for CW-112 (Real-Provider Qualification).
* **Wire Protocol:** OpenAI Responses API (`/openai/v1/responses`) with streaming Server-Sent Events (`text/event-stream`).
* **Session Transport:** Passed via native provider config arguments:
  ```toml
  model_provider="context_watcher"
  model_providers.context_watcher.base_url="http://127.0.0.1:4401/openai/v1"
  model_providers.context_watcher.wire_api="responses"
  model_providers.context_watcher.env_http_headers={"x-cw-session-id"="CW_SESSION_ID","x-cw-control-token"="CW_CONTROL_TOKEN"}
  shell_environment_policy.filters.CW_CONTROL_TOKEN="exclude"
  ```
* **Security & Credential Boundary:**
  - `x-cw-control-token` and `x-cw-session-id` headers are injected into outgoing provider requests by Codex itself.
  - ContextWatcher proxy validates the token, binds the session, strips both headers, and forwards the clean request to upstream.
  - `shell_environment_policy.filters.CW_CONTROL_TOKEN="exclude"` guarantees that model-invoked shell commands inside Codex cannot read the control token.
* **Remaining Work (CW-112):**
  - Multi-turn real-provider continuation tests (`CW_QUALIFY_REAL_CODEX=1`).
  - Session resume (`codex resume <id>`) verification with active overlays.
  - Verification across tool execution and file modifications.

### 2.2 Hermes Agent (`0.22.5`) — Hardening Required
* **Status:** **Partial / Blocked on Concurrency** · Target for CW-113.
* **Wire Protocol:** OpenAI / DeepSeek Chat Completions (`/v1/chat/completions`) with delta chunk streaming.
* **Middleware Extension Point:** `plugins/hermes/context-watcher/__init__.py` registers an `llm_request` hook that attaches `x-cw-session-id`, `x-cw-control-token`, and `x-cw-workspace` to requests routed to `_CW_ROUTES`.
* **Identified Blocker (P0 — Race Condition in Wrap Override):**
  - In `plugins/hermes/context-watcher/__init__.py`:
    ```python
    def _consume_wrap_override(hermes_session_id):
        path = _data_dir() / "hermes-session-override.json"
        ...
        payload = json.loads(path.read_text(encoding="utf-8"))
        path.unlink(missing_ok=True)
    ```
  - The handoff between `cw wrap hermes` and the Python middleware relies on a single shared file: `~/.local/share/context-watcher/hermes-session-override.json`.
  - The first process reads and deletes the file. If two Hermes instances launch concurrently (e.g. gateway dispatch or background cron), the second process fails to find the override or receives the wrong session ID.
  - **Resolution Plan for CW-113:** Replace the single static file with per-PID or per-invocation files (e.g. `hermes-session-override.<pid>.json` or explicit environment binding `CW_HERMES_SESSION_FILE`).

### 2.3 OMP / Oh My Pi (`18.12.6`) — Native Integration Required
* **Status:** **Blocked for Live Context Optimization in Baseline** · Target for CW-111.
* **Wire Protocol:** OpenAI Chat Completions (`/v1/chat/completions`).
* **Current Baseline State:**
  - `plugins/pi-opencode/index.js` only provides terminal slash commands (`/cw status`, `/cw gain`, `/cw optimize`).
  - When `/cw optimize` is invoked, it spawns `cw optimize -s <level> --sync-back` as a subshell command.
  - `cw wrap pi` intentionally strips `CW_*` auth environment variables so child tools cannot access credentials.
  - Consequently, OMP runs in **passive observation mode only**. Outgoing model prompts are not intercepted or optimized in-flight.
* **Resolution Plan for CW-111:**
  - Implement native OMP runtime integration using OMP's `context` or `before_provider_request` extension hooks.
  - Pass the active context through contextWatcher's transformation engine before prompt transmission, leaving the on-disk conversation history untouched.

---

## 3. Upstream Wire Evidence & Captured Requests

### 3.1 Codex CLI Outgoing Request & Forwarding Verification
Captured from `test_ac9_binary_task_continuation_codex_cli`:
* **Inbound to Proxy (`POST /openai/v1/responses`):**
  ```http
  POST /openai/v1/responses HTTP/1.1
  host: 127.0.0.1:4401
  content-type: application/json
  accept: text/event-stream
  x-cw-session-id: codex_qual_ac9
  x-cw-control-token: <redacted-32-char-token>
  authorization: Bearer mock-openai-key
  ```
* **Outbound Forwarded by Proxy to Upstream (`POST /v1/responses`):**
  ```http
  POST /v1/responses HTTP/1.1
  host: 127.0.0.1:19284
  content-type: application/json
  accept: text/event-stream
  authorization: Bearer mock-openai-key
  ```
* **Header Stripping Assertion:**
  - `headers.get("x-cw-control-token") == None` ✅
  - `headers.get("x-cw-session-id") == None` ✅
  - Upstream receives 0 internal contextWatcher metadata headers.

### 3.2 Overlay Reapplication on Turn $N+1$
Captured from `test_ac2_committed_overlay_on_next_model_request`:
* **Turn 1 Outbound:** Clean unmutated user prompt.
* **Mutation Committed:** `PinContext { label: "Policy", content: "CRITICAL_PINNED_DIRECTIVE: Preserved across turns" }`.
* **Turn 2 Outbound Forwarded to Upstream:**
  ```json
  {
    "model": "gpt-4o",
    "messages": [
      {"role": "user", "content": "initial prompt"},
      {"role": "assistant", "content": "Mock chat output"},
      {"role": "user", "content": "follow-up prompt\n\n<cw_pinned_context>\nCRITICAL_PINNED_DIRECTIVE: Preserved across turns\n</cw_pinned_context>"}
    ]
  }
  ```
* **Result:** Overlay applied to active model context; source session history on disk untouched.

---

## 4. Reproducible Qualification Suite Runner

The qualification suite is implemented as an automated Rust integration test suite in [`crates/cw-proxy/tests/qualification.rs`](file:///home/gionag/Development/contextWatcher/crates/cw-proxy/tests/qualification.rs).

### Running Local Mock Contract Tests
```bash
# Run all 12 contract & binary continuation tests
cargo test -p cw-proxy --test qualification

# Run with verbose real-time logging
cargo test -p cw-proxy --test qualification -- --nocapture
```

### Running Opt-in Real-Provider Continuation Tests
Live provider tests are disabled by default (`#[ignore]`) and can be enabled individually:
```bash
# Live Codex CLI qualification against OpenAI
export OPENAI_API_KEY="sk-..."
export CW_QUALIFY_REAL_CODEX=1
cargo test -p cw-proxy --test qualification -- test_live_continuation_real_codex --ignored --nocapture

# Live Hermes Agent qualification against DeepSeek
export DEEPSEEK_API_KEY="sk-..."
export CW_QUALIFY_REAL_HERMES=1
cargo test -p cw-proxy --test qualification -- test_live_continuation_real_hermes --ignored --nocapture

# Live OMP qualification against OpenAI
export OPENAI_API_KEY="sk-..."
export CW_QUALIFY_REAL_OMP=1
cargo test -p cw-proxy --test qualification -- test_live_continuation_real_omp --ignored --nocapture
```
