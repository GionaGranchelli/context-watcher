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
| **Codex CLI (`0.166.0`)** | 🟢 Verified | 🟢 Verified (3-Turn Native Resume & Overlays) | 🟢 Verified (`gpt-4o-mini` multi-turn live continuation) | 🟢 Supported (Responses API) | Qualified in CW-112 & CW-114 |
| **Hermes Agent (`0.22.5`)** | 🟢 Verified | 🟢 Verified (Multi-turn resume & concurrency hardened) | 🟡 Opt-in Suite C | 🟢 Supported (OpenAI / DeepSeek middleware) | Qualified in CW-113 & CW-114 |
| **OMP / Oh My Pi (`18.12.6`)** | 🟢 Verified | 🟢 Verified (Native Runtime Hardened) | 🟡 Opt-in Suite C | 🟢 Supported (Native `context` hook) | Qualified in CW-111B & CW-114 |

---

## 3. The 10 Acceptance Criteria Matrix

| # | Acceptance Criterion | Required Invariant | Codex CLI (`0.166.0`) | Hermes Agent (`0.22.5`) | OMP / Oh My Pi (`18.12.6`) |
|---|---|---|:---:|:---:|:---:|
| **AC1** | **Session Identity & Binding** | Correct session bound across multi-turn and resume without crossover | 🟢 **Harness Binary Verified**<br>`x-cw-session-id` header via custom model provider | 🟢 **Harness Binary Verified (Concurrency Hardened)**<br>Native `local-...` session bound across turn 1 and `--resume` turn 2; concurrent launches isolated via UUID-scoped handoff files (`CW_HERMES_OVERRIDE_FILE`) and per-session gateway tracking (CW-113) | 🟢 **Harness Binary Verified**<br>Session identity bound via `CW_SESSION_ID` / `sessionManager.getSessionId()`; control token read from 0600 file |
| **AC2** | **Committed Overlay Reapplication** | Overlays committed on turn $N$ apply to outbound model payload on turn $N+1$ | 🟢 **Harness Binary Verified**<br>Multi-turn test confirms `PinContext` applied to turn 2 Responses `input` array | 🟢 **Harness Binary Verified**<br>Turn 2 `--resume` outbound Chat Completions payload contains committed `PinContext` policy | 🟢 **Harness Binary Verified**<br>Native `context` hook intercepts in-memory messages, queries `/transform`, and injects committed overlays into outbound prompt |
| **AC3** | **Zero Duplicate Injection** | Overlays applied exactly once across turns $N, N+1, N+2$; no duplicate accumulation | 🟢 **Core Proxy Verified**<br>Tested across turns 1..3 with 0 duplicate pin occurrences | 🟢 **Core Proxy Verified**<br>Overlay deduplicated by `apply_mutations_to_payload` | 🟢 **Core Proxy Verified**<br>Deduplication verified in canonical engine |
| **AC4** | **Tool-Call / Result Pairing** | Tool call IDs and tool result pairings strictly preserved under active mutation | 🟢 **Core Proxy Verified**<br>Tested with active `PinContext` mutation; OpenAI Responses & Chat tool pairings 100% valid | 🟢 **Core Proxy Verified**<br>Chat completion tool call and tool result IDs strictly paired | 🟢 **Core Proxy Verified**<br>Chat completion tool call and tool result IDs strictly paired |
| **AC5** | **Signed / Unsupported Shape Passthrough** | Cryptographic thinking signatures & unparseable shapes forwarded with exact byte & JSON-tree fidelity | 🟢 **Core Proxy Verified**<br>Anthropic `thinking` signatures asserted byte-for-byte and JSON-tree equal | 🟢 **Core Proxy Verified**<br>Byte-for-byte passthrough verified on raw wire payloads | 🟢 **Core Proxy Verified**<br>Byte-for-byte passthrough verified on raw wire payloads |
| **AC6** | **Credential Stripping & Tool Sandboxing** | Control tokens stripped from upstream requests and inaccessible to model tool commands | 🟢 **Core Proxy Verified (Config Sandboxing Contract)**<br>Stripped before upstream; `shell_environment_policy.filters.CW_CONTROL_TOKEN="exclude"` configured in settings; live model shell tool verification in CW-112 | 🟢 **Harness Binary Verified**<br>Stripped before upstream; UUID-scoped `hermes-override-<uuid>.json` created with `0600` permissions (`create_new(true)`) and atomically unlinked on consumption; `CW_CONTROL_TOKEN_FILE` read from 0600 file; same-user file boundary documented | 🟢 **Harness Binary Verified**<br>Stripped before upstream; `models.json` has 0 headers; `CW_CONTROL_TOKEN` purged from `process.env` on startup; same-user file boundary documented |
| **AC7** | **Pristine Source Transcripts** | Harness on-disk transcripts and state databases are never corrupted or rewritten in place | 🟢 **Core Proxy Verified (Transcript Immutability Contract)**<br>Harness ingestion, store indexing, and active proxy requests assert 100% BLAKE3 bit-for-bit hash identity on source files | 🟢 **Core Proxy Verified (Transcript Immutability Contract)**<br>Source transcript files untouched bit-for-bit | 🟢 **Harness Binary Verified**<br>On-disk transcript preserves original turns without in-place mutation; in-memory overlays never touch disk transcript |
| **AC8** | **Safe Fault Recovery & Gate Fail-Open** | Malformed mutations or daemon outages fail open or safe without corrupting agent session | 🟢 **Core Proxy Verified**<br>Corrupting mutation committed; Pre-Forward Validation Gate catches orphan tool invariant violation, logs diagnostic warning, and fails open to raw payload | 🟢 **Core Proxy Verified**<br>Pre-Forward Validation Gate fail-open verified | 🟢 **Core Proxy Verified**<br>Pre-Forward Validation Gate fail-open verified |
| **AC9** | **Live Binary Task Continuation** | Actual agent binary completes prompt/task against isolated proxy | 🟢 **Harness Binary Verified**<br>`codex exec` completes code 0 in automated test across multiple turns | 🟢 **Harness Binary Verified**<br>`hermes chat` completes code 0 across initial turn and resumed turn | 🟢 **Harness Binary Verified**<br>`omp -p` completes code 0 in automated test (passive proxy mode) |
| **AC10** | **Telemetry Provenance & Cache Accounting** | Token usage verified against telemetry provenance; un-telemetred cache strictly un-fabricated | 🟢 **Core Proxy Verified**<br>Observed provider usage (`prompt_tokens`, `completion_tokens`) reconciled against AST token spans; absent cache telemetry strictly reported as 0 tokens (`telemetry_available: false`) | 🟢 **Core Proxy Verified**<br>Usage provenance reconciled against wire telemetry | 🟢 **Core Proxy Verified**<br>Usage provenance reconciled against wire telemetry |

---

## 4. Per-Harness Architecture & Verified Baselines

### 4.1 Codex CLI (`0.166.0`) — Reference Implementation
* **Status:** **Harness Binary Verified (3-Turn Native Resume & Overlays Qualified)** · Completed in CW-112 (see [CW-112-CODEX-QUALIFICATION.md](CW-112-CODEX-QUALIFICATION.md)).
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
  - `test_harness_binary_codex_cli_native_resume_three_turn_qualification`:
    - Turn 1: `codex exec` starts persistent native thread (`thread.started`), writes `calculator.py` with `add` function via `exec_command`.
    - Turn 2: `codex exec resume <thread_id>` resumes same thread UUID, ContextWatcher injects committed `CW112_POLICY_ACTIVE` overlay, tool appends `multiply`.
    - Turn 3: Second native resume re-applies overlay across resume boundary, tool creates `test_calculator.py` and runs unit tests; verified exit code 0.
    - Rollouts in `CODEX_HOME/sessions` confirmed free of injected overlay content (proxy operates strictly in-flight without modifying persisted transcripts).
  - `test_harness_binary_codex_cli_unknown_native_session_id_resume`: Resuming unknown UUID exits non-zero; daemon remains healthy.
  - `test_harness_binary_codex_cli_unauthorized_token_rejection`: Invalid control token rejected with HTTP 403 Forbidden.
  - `test_harness_binary_codex_cli_upstream_failure_handling`: Upstream 502 Bad Gateway handled with retry backoff; store uncorrupted.
  - `test_harness_binary_codex_cli_session_isolation`: Session B isolated from Session A's active mutation overlay.
  - Inbound `x-cw-control-token` and `x-cw-session-id` stripped prior to upstream forwarding.
  - Tool sandboxing directly tested: model-invoked tool commands inside Codex confirmed to execute without `CW_CONTROL_TOKEN` in environment (`shell_environment_policy.filters.CW_CONTROL_TOKEN="exclude"`).
* **Live Qualification Status:**
  - `test_live_continuation_real_codex_multiturn` executed and verified against live OpenAI endpoint with `gpt-4o-mini` (19.26s, 100% pass) under authorized spend budget (<$0.01 spent).
  - Multi-turn execution proved:
    - Slices requests by invocation snapshot intervals (capturing all turn subrequests).
    - Session continuity preserved across native `codex exec resume <thread_id>`.
    - Real prompt caching observed on Turn 2 and Turn 3 (11,904 cached prompt tokens).
    - Committed overlay `CW112_POLICY_ACTIVE` verified in outbound wire egress bytes (`store.get_wire_egress`).
    - Tool subprocesses isolated from credentials (`CW_CONTROL_TOKEN="ABSENT"`).
    - Persisted rollout JSONL history remains pristine without proxy overlay leakage.

### 4.2 Hermes Agent (`0.22.5`) — Concurrency Hardened & Resume Verified
* **Status:** **Harness Binary Verified (Multi-Turn Resume & Concurrency Hardened)** · Completed in CW-113.
* **Wire Protocol:** OpenAI / DeepSeek Chat Completions (`/v1/chat/completions`) with delta chunk streaming.
* **Middleware Extension Point & Concurrency Hardening (CW-113):**
  - `plugins/hermes/context-watcher/__init__.py` registers an `llm_request` hook that attaches `x-cw-session-id`, `x-cw-control-token`, and `x-cw-workspace` to requests routed to `_CW_ROUTES`.
  - **UUID-Scoped Override Handoff:** Replaced the global `hermes-session-override.json` file with unique per-invocation files: `~/.local/share/context-watcher/hermes-override-<uuid>.json`.
  - **Atomic File Creation & Strict Permissions:** Files are created using `OpenOptions::new().create_new(true).mode(0o600)`, ensuring no symlink or pre-existing file takeover and restricting access strictly to the invoking user.
  - **Environment-Directed Reference:** The wrapper passes the unique path via `CW_HERMES_OVERRIDE_FILE`. Hermes gateway allows this through via `terminal.client_env_passthrough`.
  - **Handoff Validation & Atomic Unlink:** The middleware verifies that the target path is a regular file (not a symlink), owned with mode `0600` (`mode & 077 == 0`), created within 120 seconds, and contains valid JSON with a non-empty session ID. It unlinks the file immediately on read (`unlink()`), so each handoff file is consumed exactly once.
  - **Gateway Daemon Session Scoping:** The Hermes gateway service (`hermes-gateway`) runs as a single persistent daemon serving multiple concurrent client sessions. The middleware indexes consumed overrides strictly by `hermes_sid` in `_SESSION_OVERRIDES`, eliminating shared module-level state and cross-session contamination.
  - **Same-User Security Boundary:** Handoff files and control token files are created with `0600` permissions, protecting credentials from other user accounts on the host. In an uncontained execution environment, processes running under the same user UID share filesystem read privileges; isolation across processes requires OS containers (e.g. bubblewrap/docker). ContextWatcher unlinks handoff files immediately to minimize exposure windows.
  - **Fail-Safe Operation:** Invalid, expired, unreadable, or symlinked override files fail safe: the middleware ignores the invalid handoff and does not attach spoofed or arbitrary session headers.
* **Verified Evidence (7-Gate Multi-Process Qualification Test):**
  - `test_harness_binary_hermes_middleware_session_binding` exercises 7 comprehensive verification gates:
    - **Gate 1 (Handoff Consumption & Cleanup):** Two concurrent Hermes processes (Process A and Process B) launched simultaneously against the proxy; each consumed and atomically unlinked its own UUID-scoped override file.
    - **Gate 2 (Session Identity Isolation):** Process A and Process B bound to two distinct native Hermes session IDs (`local-...`), with no cross-talk.
    - **Gate 3 (Upstream Overlay Isolation):** Upstream mock captured model requests from both processes. Turn A received policy `CRITICAL_POLICY_A_ONLY`, Turn B received policy `CRITICAL_POLICY_B_ONLY`. Cross-contamination asserted to be exactly 0 (Turn A never saw B's policy; Turn B never saw A's policy).
    - **Gate 4 (Native Resume Verification):** Session A was resumed via `hermes chat -r <sid_A>`. Turn 2 executed cleanly, maintained session identity `sid_A`, and carried active policy `CRITICAL_POLICY_A_ONLY`.
    - **Gate 5 (Invalid Handoff Fail-Safe):** Tested with a pre-existing invalid handoff file; Hermes completed cleanly and the middleware failed safe without attaching unauthorized overlays.
    - **Gate 6 (Credential Stripping):** Captured wire requests across all turns verified that `x-cw-control-token` and `x-cw-session-id` were stripped prior to reaching the upstream model endpoint.
    - **Gate 7 (20-Session Concurrency Stress Test):** Python test suite `plugins/hermes/context-watcher/test_plugin.py` executed a 20-thread concurrent invocation stress test asserting 0 collisions, 0 race conditions, and complete file cleanup across all 20 threads.
* **Remaining Work (CW-112):**
  - Opt-in live multi-turn cloud provider qualification (`CW_QUALIFY_REAL_HERMES=1` against live DeepSeek endpoint).

### 4.3 OMP / Oh My Pi (`18.12.6`) — Native Runtime Context Hook & Hardened Isolation
* **Status:** **Harness Binary Verified (Native Runtime Hardened & Multi-Turn Resume Verified)** · Completed in CW-111 / CW-111B.
* **Wire Protocol:** OpenAI Chat Completions (`/v1/chat/completions`) & Anthropic Messages (`/v1/messages`).
* **Architecture & Security Boundary (CW-111B Hardening):**
  - `plugins/pi-opencode/index.js` registers native OMP runtime event hooks `pi.on("context", ...)` and `pi.on("before_provider_request", ...)`.
  - **Zero Credential Exposure in Config:** `models.json` contains zero custom headers or privileged CW control tokens. The unsafe helper `omp_proxy_models_config_with_headers()` has been completely removed from `cw-harnesses`.
  - **Process Environment Sanitization & Same-User Boundary:** Private control tokens are read securely from `~/.local/share/context-watcher/control-token` (`0600` permissions). The extension immediately purges `CW_CONTROL_TOKEN` and `CW_CONTROL_TOKEN_FILE` from `process.env` in memory on startup so child processes spawned by OMP tools (bash, scripts) cannot inherit them.
    > [!NOTE] Same-User Security Boundary: Files with `0600` permissions protect credentials against unauthorized user accounts on the host. However, any uncontained tool running under the *same* OS user account can access user-readable files unless enclosed in an OS sandbox/container (e.g. bubblewrap/docker). ContextWatcher purges the token from the process environment so standard tool executions do not inherit it.
  - **Loopback Enforcement:** `CW_DAEMON_URL` strictly validates hostname against loopback addresses (`127.0.0.1`, `localhost`, `::1`, `[::1]`). Any non-loopback destination fails closed back to local default `http://127.0.0.1:4400`.
  - **Canonical Session Identity & Isolation:** Prioritizes native session identity (`ctx.sessionManager.getSessionId()`) over ambient environment variables (`CW_SESSION_ID`), guaranteeing that switching native sessions within one process does not cross-bind to an initial wrapper session.
  - **Single-Pass Transformation & Reference Idempotency:** Employs reference-based `WeakSet` tracking on both transformed message arrays and constituent message objects. Eliminates heuristic time/length windows, guaranteeing that subsequent requests bypassing the context hook are safely transformed while turns already transformed by the context hook are never double-mutated.
  - **Dynamic Provider Resolution:** Automatically resolves provider (`anthropic`, `gemini`, `deepseek`, `ollama`, `openai`) from `ctx.model.provider` or payload shape.
  - **Fail-Open Guarantee:** If the CW daemon is offline, unauthenticated, or returns an error, the extension fails open instantly, returning pristine messages so agent workflows are never interrupted.
* **Verified Evidence (3-Turn Native Resume & Fail-Open Test):**
  - `test_harness_binary_omp_native_runtime_overlay_reapplication`:
    - Turn 1: `omp -p "Turn 1 pristine prompt"` executed with tools enabled against isolated proxy endpoint without `--no-session`. Native OMP writes session transcript to `$PI_CODING_AGENT_DIR/sessions/<slug>/<timestamp>_<uuid>.jsonl`.
    - Turn 1 completed with exit code 0.
    - Active `PinContext` mutation committed to CwStore (`CRITICAL_OMP_PINNED_POLICY`). (Safety contract enforces that user and system prompts are never pruned or stubbed).
    - Turn 2: Native OMP resume executed via `omp -r <uuid> -p "Turn 2 resume prompt"` with tools enabled.
    - Native `context` hook intercepted in-memory messages, queried `/transform`, and injected the pinned policy into outbound wire messages.
    - Upstream mock captured Turn 2 wire payload containing `CRITICAL_OMP_PINNED_POLICY`.
    - On-Disk Transcript Evidence: The `.jsonl` transcript file legitimately grew by appending Turn 2; asserted on-disk transcript preserves original Turn 1 prompt, records Turn 2, and does NOT contain the injected pinned policy (in-memory overlay only).
    - Turn 3: Fail-Open verification under offline daemon (`CW_DAEMON_URL="http://127.0.0.1:19999"`). OMP completed cleanly with exit code 0 and pristine prompt forwarded.
    - Upstream captured 0 internal contextWatcher metadata headers (`x-cw-*` stripped).
    - Node.js contract test verified session precedence (native session taking priority over `CW_SESSION_ID`), session switching between native sessions, and reference-based single-pass idempotency.

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

The qualification suite is implemented in [`crates/cw-proxy/tests/qualification.rs`](../crates/cw-proxy/tests/qualification.rs) and organized into three test suites:

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

---

## 7. Long-Session Reliability Campaign (CW-114)

Implemented in [`crates/cw-proxy/tests/long_session_reliability.rs`](../crates/cw-proxy/tests/long_session_reliability.rs) and documented in [`docs/CW-114-LONG-SESSION-RELIABILITY.md`](CW-114-LONG-SESSION-RELIABILITY.md).

* **Scope:** 5 realistic coding-task families (`F1_BugFixing`, `F2_Refactoring`, `F3_FeatureImplementation`, `F4_Tdd`, `F5_InvestigationRecovery`) evaluated across 3 repetitions on all 3 target harnesses (Codex CLI, OMP, Hermes Agent) = **45 total sessions, 360 logical checkpoints/turns**.
* **Results:**
  - **Pass Rate:** **100% (45/45 passed, 0 failed, 0 degraded)**.
  - **Tool Pairing Validity:** 100% across all 360 turns.
  - **Session Isolation & Transcript Integrity:** 100% across all sessions; 0 credential leaks; 0 transcript corruptions.
  - **Fault Injection & Resilience:** Verified mid-stream drop detection (`stream_error`), HTTP 502/429 retry backoff recovery, daemon restart state preservation, and concurrent session overlay isolation.
* **Execution:**
  ```bash
  # Deterministic 45-session matrix
  cargo test -p cw-proxy --test long_session_reliability test_cw114_deterministic_45_session_campaign -- --ignored --nocapture

  # Fault injection & resilience suite
  cargo test -p cw-proxy --test long_session_reliability test_cw114_fault_injection -- --ignored --nocapture
  cargo test -p cw-proxy --test long_session_reliability test_cw114_resilience -- --ignored --nocapture
  ```

---

## 8. Empirical Performance & Savings Qualification (CW-115)

Implemented in [`crates/cw-proxy/tests/performance_qualification.rs`](../crates/cw-proxy/tests/performance_qualification.rs) and documented in [`docs/CW-115-PERFORMANCE-SAVINGS-QUALIFICATION.md`](CW-115-PERFORMANCE-SAVINGS-QUALIFICATION.md), with raw machine-readable telemetry recorded in [`docs/cw115_benchmark_data.json`](cw115_benchmark_data.json).

* **Scope & Methodology:** Evaluates 3 distinct experiment modes across real coding-agent workloads with real provider telemetry and strict micro-dollar spend guardrails in the **Codex CLI (`0.166.0`) F1 Bug-Fixing Live Pilot**:
  - **Mode A (Native Baseline):** Harness connects directly to provider without proxy or intermediate hops.
  - **Mode B (CW Passthrough):** Harness routes through `cw-proxy` with active mutations disabled (`AutoPilotConfig.enabled = false`, 0 mutations).
  - **Mode C (CW Optimized):** Harness routes through `cw-proxy` with active committed context optimizations (historical tool output pruning and tail pin overlays).
* **Observed Empirical Findings:**
  - **Passthrough Overhead (Mode B vs Mode A):** Mean streaming overhead $< 2\text{ms/chunk}$; byte-level forwarding fidelity on intercepted payloads; TTFB averaging ~628ms; wall time within normal provider endpoint variation (-17.8% net delta due to cloud inference cluster timing); token totals reflect stochastic agent tool iteration paths (-90,190 mean delta).
  - **Headline Aggregate Finding (Mode C vs Mode B, Both Repetitions):** Across both repetitions, optimization increased total input tokens by **+50.0%** (451,353 vs 300,976) and calculated cost by **+46.2%** ($0.03950 vs $0.02702) due to mid-session prompt cache invalidation in Repetition 1.
  - **Cache-Aligned Counterexample (Repetition 2):** When optimizations preserved the prompt cache prefix, Mode C reduced input tokens by **-21.3%** (144,528 $\to$ 113,725), wall clock execution time by **-29.2%** (25.89s $\to$ 18.34s), and calculated cost by **-20.7%** ($0.01300 $\to$ $0.01031).
  - **Architectural Validation:** Confirms ContextWatcher's core architectural principle: **KV-Cache Awareness First**. Pruning historical turns when a warm cache exists destroys the 50% prompt-cache discount and induces exploratory requests; mutations must operate strictly in tail/cache-safe mode (`min` strategy) whenever a warm prefix is detected.
* **Execution:**
  ```bash
  # Deterministic cost model, budget guards, and regression math (CI test suite)
  cargo test -p cw-proxy --test performance_qualification

  # Opt-in live provider benchmark (requires OPENAI_API_KEY and CW_QUALIFY_REAL_CODEX=1)
  export OPENAI_API_KEY="sk-..."
  export CW_QUALIFY_REAL_CODEX=1
  cargo test -p cw-proxy --test performance_qualification test_live_codex_cw115_empirical_performance_qualification -- --ignored --nocapture
  ```


