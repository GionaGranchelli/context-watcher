# HARNESS_MATRIX.md — Production Qualification Matrix & Harness Baselines

This document establishes the verified evidence baseline for **CW-110 / CW-110A (Production-Qualified Harness Integrations)** across the three primary target harnesses: **Codex CLI**, **Hermes Agent**, and **OMP (Oh My Pi)**.

Qualification requires proving that contextWatcher (`cw`) observes the correct native session, optimizes the context actually transmitted to the model, preserves agent tool-calling functionality, and survives multi-turn sessions, restarts, and errors without data loss or credential leakage.

---

## 1. Qualification Status Tiers

Every harness integration is categorized according to four qualification tiers:

* 🟢 **Core Proxy Verified:** Protocol normalizer, Canonical IR, mutation overlay engine, credential stripping, byte-for-byte fidelity, and invariant validation tested and passing against mock provider wire contracts in CI without external binaries.
* 🟢 **Harness Binary Verified:** The actual harness binary (`codex`, `hermes`, `omp`) executes end-to-end in automated integration tests against an isolated local mock provider, asserting process exit code 0, session binding, and wire behavior. Tests explicitly fail (`panic!`) if the binary is absent when invoked.
* 🟢 **Real-Provider Verified:** Multi-turn coding sessions and task continuations executed against live cloud inference endpoints (OpenAI, Anthropic, DeepSeek) with committed overlays and observed token/cache telemetry.
* 🟡 / 🔴 **Partial / Blocked:** Functionality that is partially implemented or blocked by known architectural limitations, with tracking tickets and remediation plans.

---

## 2. Summary Status by Harness

| Harness | Core Proxy Verified | Harness Binary Verified | Real-Provider Verified | Live Optimization Status | Active Blocker / Target |
|---|:---:|:---:|:---:|:---:|---|
| **Codex CLI (`0.165.0`)** | 🟢 Verified | 🟢 Verified (Multi-turn) | 🟡 Opt-in Suite C | 🟢 Supported (Responses API) | CW-112 (Real-provider qualification & resume) |
| **Hermes Agent (`0.22.5`)** | 🟢 Verified | 🟢 Verified (Multi-turn resume) | 🟡 Opt-in Suite C | 🟡 Single-session only | CW-113 (Shared override file race condition) |
| **OMP / Oh My Pi (`18.12.6`)** | 🟢 Verified | 🟢 Verified (Native Runtime) | 🟡 Opt-in Suite C | 🟢 Supported (Native `context` hook) | CW-114 (Long-session reliability campaign) |

---

## 3. The 10 Acceptance Criteria Matrix

| # | Acceptance Criterion | Required Invariant | Codex CLI (`0.165.0`) | Hermes Agent (`0.22.5`) | OMP / Oh My Pi (`18.12.6`) |
|---|---|---|:---:|:---:|:---:|
| **AC1** | **Session Identity & Binding** | Correct session bound across multi-turn and resume without crossover | 🟢 **Harness Binary Verified**<br>`x-cw-session-id` header via custom model provider | 🟢 **Harness Binary Verified** (Single Session)<br>Native `local-...` session bound across turn 1 and `--resume` turn 2; concurrent launches collide on shared file (CW-113) | 🟢 **Harness Binary Verified**<br>Session identity bound via `CW_SESSION_ID` / `sessionManager.getSessionId()`; control token read from 0600 file |
| **AC2** | **Committed Overlay Reapplication** | Overlays committed on turn $N$ apply to outbound model payload on turn $N+1$ | 🟢 **Harness Binary Verified**<br>Multi-turn test confirms `PinContext` applied to turn 2 Responses `input` array | 🟢 **Harness Binary Verified**<br>Turn 2 `--resume` outbound Chat Completions payload contains committed `PinContext` policy | 🟢 **Harness Binary Verified**<br>Native `context` hook intercepts in-memory messages, queries `/transform`, and injects committed overlays into outbound prompt |
| **AC3** | **Zero Duplicate Injection** | Overlays applied exactly once across turns $N, N+1, N+2$; no duplicate accumulation | 🟢 **Core Proxy Verified**<br>Tested across turns 1..3 with 0 duplicate pin occurrences | 🟢 **Core Proxy Verified**<br>Overlay deduplicated by `apply_mutations_to_payload` | 🟢 **Core Proxy Verified**<br>Deduplication verified in canonical engine |
| **AC4** | **Tool-Call / Result Pairing** | Tool call IDs and tool result pairings strictly preserved under active mutation | 🟢 **Core Proxy Verified**<br>Tested with active `PinContext` mutation; OpenAI Responses & Chat tool pairings 100% valid | 🟢 **Core Proxy Verified**<br>Chat completion tool call and tool result IDs strictly paired | 🟢 **Core Proxy Verified**<br>Chat completion tool call and tool result IDs strictly paired |
| **AC5** | **Signed / Unsupported Shape Passthrough** | Cryptographic thinking signatures & unparseable shapes forwarded with exact byte & JSON-tree fidelity | 🟢 **Core Proxy Verified**<br>Anthropic `thinking` signatures asserted byte-for-byte and JSON-tree equal | 🟢 **Core Proxy Verified**<br>Byte-for-byte passthrough verified on raw wire payloads | 🟢 **Core Proxy Verified**<br>Byte-for-byte passthrough verified on raw wire payloads |
| **AC6** | **Credential Stripping & Tool Sandboxing** | Control tokens stripped from upstream requests and inaccessible to model tool commands | 🟢 **Core Proxy Verified (Config Sandboxing Contract)**<br>Stripped before upstream; `shell_environment_policy.filters.CW_CONTROL_TOKEN="exclude"` configured in settings; live model shell tool verification in CW-112 | 🟡 **Partial**<br>Stripped before upstream; `CW_CONTROL_TOKEN_FILE` is a disk file accessible to local user | 🟢 **Core Proxy Verified**<br>Stripped before upstream; `cw wrap` removes env tokens |
| **AC7** | **Pristine Source Transcripts** | Harness on-disk transcripts and state databases are never corrupted or rewritten in place | 🟢 **Core Proxy Verified (Transcript Immutability Contract)**<br>Harness ingestion, store indexing, and active proxy requests assert 100% BLAKE3 bit-for-bit hash identity on source files | 🟢 **Core Proxy Verified (Transcript Immutability Contract)**<br>Source transcript files untouched bit-for-bit | 🟢 **Core Proxy Verified (Transcript Immutability Contract)`**<br>Source transcript files untouched bit-for-bit |
| **AC8** | **Safe Fault Recovery & Gate Fail-Open** | Malformed mutations or daemon outages fail open or safe without corrupting agent session | 🟢 **Core Proxy Verified**<br>Corrupting mutation committed; Pre-Forward Validation Gate catches orphan tool invariant violation, logs diagnostic warning, and fails open to raw payload | 🟢 **Core Proxy Verified**<br>Pre-Forward Validation Gate fail-open verified | 🟢 **Core Proxy Verified**<br>Pre-Forward Validation Gate fail-open verified |
| **AC9** | **Live Binary Task Continuation** | Actual agent binary completes prompt/task against isolated proxy | 🟢 **Harness Binary Verified**<br>`codex exec` completes code 0 in automated test across multiple turns | 🟢 **Harness Binary Verified**<br>`hermes chat` completes code 0 across initial turn and resumed turn | 🟢 **Harness Binary Verified**<br>`omp -p` completes code 0 in automated test (passive proxy mode) |
| **AC10** | **Telemetry Provenance & Cache Accounting** | Token usage verified against telemetry provenance; un-telemetred cache strictly un-fabricated | 🟢 **Core Proxy Verified**<br>Observed provider usage (`prompt_tokens`, `completion_tokens`) reconciled against AST token spans; absent cache telemetry strictly reported as 0 tokens (`telemetry_available: false`) | 🟢 **Core Proxy Verified**<br>Usage provenance reconciled against wire telemetry | 🟢 **Core Proxy Verified**<br>Usage provenance reconciled against wire telemetry |

---

## 4. Per-Harness Architecture & Verified Baselines

### 4.1 Codex CLI (`0.165.0`) — Reference Implementation
* **Status:** **Harness Binary Verified** · Ready for CW-112 (Real-Provider Qualification).
* **Wire Protocol:** OpenAI Responses API (`/openai/v1/responses`) with streaming Server-Sent Events (`text/event-stream`).
* **Session Transport:** Native provider config arguments:
  ```toml
  model_provider="context_watcher"
  model_providers.context_watcher.base_url="http://127.0.0.1:4401/openai/v1"
  model_providers.context_watcher.wire_api="responses"
  model_providers.context_watcher.env_http_headers={"x-cw-session-id"="CW_SESSION_ID","x-cw-control-token"="CW_CONTROL_TOKEN"}
  shell_environment_policy.filters.CW_CONTROL_TOKEN="exclude"
  ```
* **Verified Evidence:**
  - `test_harness_binary_codex_cli_multiturn_overlay_reapplication`:
    - Turn 1 executed via `codex exec` with session ID `codex_multiturn_qual_sess`.
    - Active `PinContext` mutation committed to CwStore.
    - Turn 2 executed with same session ID. ContextWatcher proxy intercepted turn 2, injected pinned context into the Responses `input` array.
    - Upstream mock captured request and returned streaming response.
    - Codex exited with status code 0.
  - Inbound `x-cw-control-token` stripped prior to upstream forwarding.
  - Tool sandboxing: `shell_environment_policy.filters.CW_CONTROL_TOKEN="exclude"` guarantees that model-invoked shell commands inside Codex cannot read the control token.
* **Remaining Work (CW-112):**
  - Multi-turn real-provider continuation tests (`CW_QUALIFY_REAL_CODEX=1`).
  - Session resume (`codex resume <id>`) verification with active overlays.
  - Live model-invoked shell command execution asserting token inaccessibility.

### 4.2 Hermes Agent (`0.22.5`) — Concurrency Blocker
* **Status:** **Harness Binary Verified (Multi-Turn Resume)** · Blocked for Concurrent Launches (CW-113).
* **Wire Protocol:** OpenAI / DeepSeek Chat Completions (`/v1/chat/completions`) with delta chunk streaming.
* **Middleware Extension Point:** `plugins/hermes/context-watcher/__init__.py` registers an `llm_request` hook that attaches `x-cw-session-id`, `x-cw-control-token`, and `x-cw-workspace` to requests routed to `_CW_ROUTES`.
* **Verified Evidence:**
  - `test_harness_binary_hermes_middleware_session_binding`:
    - Turn 1: `hermes chat` executed against `http://127.0.0.1:{proxy_port}/deepseek`.
    - Python middleware activated, attached `x-cw-session-id: <native-local-sid>` and `x-cw-control-token`.
    - ContextWatcher proxy authenticated session, stripped control token, and forwarded request.
    - CwStore recorded bound session `local-...` and Turn 1.
    - Hermes received mock response and completed with code 0.
    - Active `PinContext` mutation committed to `local-...` in CwStore.
    - Turn 2: Hermes resumed the session via `hermes chat -r <sid>`.
    - ContextWatcher proxy intercepted turn 2 under the same bound session ID and injected `CRITICAL_HERMES_PINNED_POLICY` into the Chat Completions `messages` array.
    - Upstream mock captured turn 2 request containing the overlay.
    - Hermes completed Turn 2 with exit code 0.
    - CwStore recorded 2 turns under the bound session.
    - Upstream captured 0 internal contextWatcher metadata headers.
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

### 4.3 OMP / Oh My Pi (`18.12.6`) — Native Runtime Context Hook
* **Status:** **Harness Binary Verified (Native Runtime Context Hook Verified)** · Completed in CW-111.
* **Wire Protocol:** OpenAI Chat Completions (`/v1/chat/completions`).
* **Architecture:**
  - `plugins/pi-opencode/index.js` registers native OMP runtime event hooks `pi.on("context", ...)` and `pi.on("before_provider_request", ...)`.
  - When OMP prepares messages for prompt formatting, the `context` hook extracts current in-memory conversation messages and session identity (`ctx.sessionManager.getSessionId()`).
  - Reads the local control token securely from `~/.local/share/context-watcher/control-token` (0600 permissions, zero exposure to child tool environments).
  - Posts messages to `POST /api/v1/sessions/:id/transform` on the local CW daemon/proxy.
  - Active overlays (e.g. `PinContext`, `PruneOrStub`, `ShadowDedupe`) are applied in-memory, validated by the Pre-Forward Validation Gate, and returned to OMP without touching the on-disk session `.jsonl` transcript.
  - Fail-Open Guarantee: Network timeouts, offline daemon, or invalid payloads immediately return `{ messages: event.messages }` unmodified, ensuring OMP never crashes or stalls.
* **Verified Evidence:**
  - `test_harness_binary_omp_native_runtime_overlay_reapplication`:
    - Turn 1: `omp -p "Turn 1 pristine prompt"` executed against isolated proxy endpoint with native extension loaded.
    - Upstream mock captured Turn 1 request; OMP exited with status code 0.
    - Active `PinContext` mutation committed to CwStore (`CRITICAL_OMP_PINNED_POLICY`).
    - Turn 2: `omp -p "Turn 2 follow-up prompt"` executed under the same session.
    - Native `context` hook intercepted prompt, queried `/transform`, and injected pinned policy into outbound message array.
    - Upstream mock captured Turn 2 wire payload containing `CRITICAL_OMP_PINNED_POLICY` and policy content.
    - OMP completed Turn 2 with exit code 0.
    - Invariant: Verified all on-disk session transcripts in `$PI_CODING_AGENT_DIR/sessions/` do NOT contain the injected policy, maintaining 100% bit-for-bit transcript immutability.
    - Upstream captured 0 internal contextWatcher metadata headers (`x-cw-*` stripped).

---

## 5. Upstream Wire Evidence & Captured Requests

### 5.1 Codex CLI Outgoing Request & Forwarding Verification
Captured from `test_harness_binary_codex_cli_multiturn_overlay_reapplication`:
* **Inbound to Proxy (`POST /openai/v1/responses`):**
  ```http
  POST /openai/v1/responses HTTP/1.1
  host: 127.0.0.1:4401
  content-type: application/json
  accept: text/event-stream
  x-cw-session-id: codex_multiturn_qual_sess
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

### 5.2 Overlay Reapplication on Turn 2 (Codex CLI)
Captured from `test_harness_binary_codex_cli_multiturn_overlay_reapplication`:
* **Turn 1 Outbound:** Initial prompt forwarded without mutation.
* **Mutation Committed:** `PinContext { label: "SafetyDirective", content: "CRITICAL_POLICY: MAINTAIN_TOOL_SANDBOXING" }`.
* **Turn 2 Outbound Forwarded to Upstream:**
  ```json
  {
    "model": "gpt-4o",
    "input": [
      {"role": "user", "content": [{"type": "input_text", "text": "Turn 1 prompt"}]},
      {"role": "assistant", "content": [{"type": "output_text", "text": "Mock responses output"}]},
      {"role": "user", "content": [{"type": "input_text", "text": "Turn 2 prompt\n\n<cw_pinned_context>\nCRITICAL_POLICY: MAINTAIN_TOOL_SANDBOXING\n</cw_pinned_context>"}]}
    ]
  }
  ```
* **Result:** Overlay applied to active model context; source session history on disk untouched.

### 5.3 Overlay Reapplication on Resumed Turn 2 (Hermes Agent)
Captured from `test_harness_binary_hermes_middleware_session_binding`:
* **Turn 1 Outbound:** Initial chat completion prompt forwarded cleanly under bound session `local-...`.
* **Mutation Committed:** `PinContext { label: "SafetyPolicy", content: "CRITICAL_HERMES_PINNED_POLICY" }`.
* **Turn 2 Resumed (`hermes chat -r <sid>`):**
  ```json
  {
    "model": "mock-model",
    "messages": [
      {"role": "user", "content": "Say turn 1 prompt"},
      {"role": "assistant", "content": "Mock chat output"},
      {"role": "user", "content": "Say turn 2 follow-up\n\n<cw_pinned_context>\n- [SafetyPolicy]: CRITICAL_HERMES_PINNED_POLICY\n</cw_pinned_context>"}
    ]
  }
  ```
* **Result:** Active `llm_request` middleware attached same session ID on resume; proxy intercepted and applied overlay into chat completion messages.

### 5.4 Overlay Injection via Native Runtime Context Hook (OMP / Oh My Pi)
Captured from `test_harness_binary_omp_native_runtime_overlay_reapplication`:
* **Turn 1 Outbound:** `omp -p "Turn 1 pristine prompt"` forwarded cleanly under session `omp_qual_...`.
* **Mutation Committed:** `PinContext { label: "CRITICAL_OMP_PINNED_POLICY", content: "Zero-hallucination verified native context hook" }`.
* **Turn 2 Outbound Forwarded to Upstream (`POST /v1/chat/completions`):**
  ```json
  {
    "model": "mock-model",
    "messages": [
      {"role": "user", "content": [{"type": "text", "text": "Turn 2 follow-up prompt\n<cw_pinned_context>\n- [CRITICAL_OMP_PINNED_POLICY]: Zero-hallucination verified native context hook\n</cw_pinned_context>"}]}
    ]
  }
  ```
* **Result:** Native OMP `context` hook intercepted in-memory messages before prompt serialization, applied the active overlay via local CW daemon, and sent optimized prompt to provider; on-disk `.jsonl` session files remained 100% immutable and uncorrupted.

---

## 6. Reproducible Qualification Suite Runner

The qualification suite is implemented in [`crates/cw-proxy/tests/qualification.rs`](file:///home/gionag/Development/contextWatcher/crates/cw-proxy/tests/qualification.rs) and organized into three test suites:

### Suite A: Core Proxy & IR Invariant Tests (CI Default)
Runs in standard CI environments without requiring harness binaries or cloud API keys:
```bash
cargo test -p cw-proxy --test qualification
```
Covers:
- AC1: Session identity & binding
- AC2: Committed overlay reapplication
- AC3: Zero duplicate mutation injection
- AC4: Tool-call and tool-result pairing preservation under active mutations
- AC5: Signed Anthropic request byte-for-byte and JSON-tree fidelity
- AC6: Core configuration sandboxing contract (`shell_environment_policy`)
- AC7: Core source transcript immutability contract (BLAKE3 bit-for-bit check across AGY import, OMP, Hermes)
- AC8: Safe fault recovery and Pre-Forward Validation Gate fail-open behavior
- AC10: Telemetry provenance verification and un-fabricated cache accounting

### Suite B: Harness Binary Integration Tests
Requires locally installed agent binaries (`codex`, `hermes`, `omp`). Explicitly fails if a required binary is missing:
```bash
# Run all harness binary integration tests
cargo test -p cw-proxy --test qualification harness_binary -- --ignored --nocapture

# Run specific harness binary test
cargo test -p cw-proxy --test qualification test_harness_binary_codex_cli -- --ignored --nocapture
cargo test -p cw-proxy --test qualification test_harness_binary_hermes -- --ignored --nocapture
cargo test -p cw-proxy --test qualification test_harness_binary_omp -- --ignored --nocapture
```

### Suite C: Live Multi-turn Continuation Tests (Opt-in)
Runs against live cloud inference endpoints with real provider API keys:
```bash
# Live Codex CLI qualification against OpenAI
export OPENAI_API_KEY="sk-..."
export CW_QUALIFY_REAL_CODEX=1
cargo test -p cw-proxy --test qualification test_live_continuation_real_codex -- --ignored --nocapture

# Live Hermes Agent qualification against DeepSeek
export DEEPSEEK_API_KEY="sk-..."
export CW_QUALIFY_REAL_HERMES=1
cargo test -p cw-proxy --test qualification test_live_continuation_real_hermes -- --ignored --nocapture

# Live OMP qualification against OpenAI
export OPENAI_API_KEY="sk-..."
export CW_QUALIFY_REAL_OMP=1
cargo test -p cw-proxy --test qualification test_live_continuation_real_omp -- --ignored --nocapture
```
