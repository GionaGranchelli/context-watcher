# CW-114 — Long-Session Reliability Campaign Across Harnesses

**Campaign Status:** Completed (100% Pass Rate: 45/45 Deterministic Sessions)  
**Date:** October 31, 2026  
**Final Commit:** `915bf9b4185db350db74ec3332e1db372088e48b`  
**Execution Duration:** 353.26s (5m 53s)  
**Harness Matrix:** Codex CLI (`0.166.0`), OMP (`18.12.6`), Hermes Agent (`v0.22.5`, commit `8ec749f`, Python 3.14.9, OpenAI SDK 2.54.0)  
**Scope:** 5 Workload Task Families $\times$ 3 Harnesses $\times$ 3 Repetitions (45 Sessions, 360 Checkpoints / Logical Turns, 405 Model Requests) + Phase 4 Fault-Injection Suite

---

## 1. Executive Summary

ContextWatcher (`cw`) serves as an active context manipulation engine, KV-cache prefix preserver, and intercepting proxy across heterogeneous LLM coding harnesses. Milestone **CW-114** establishes empirical product-level evidence that ContextWatcher functions with 100% reliability over extended multi-turn conversations and native resume lifecycles without stream corruption, credential leakage, transcript mutation, or overlay crossover.

### Key Achievements

1. **Deterministic Protocol & Agentic Integration Qualification (CW-114 Scope vs CW-115 Scope):**
   Milestone **CW-114** qualifies deterministic integration and protocol reliability across multi-turn sessions and native resume boundaries using scripted tool executions. It verifies that ContextWatcher's proxy, canonical IR, and mutation overlay engines handle long sessions, process restarts, tool call pairing, and fault recoveries with 100% reliability. This deterministic baseline lays the foundation for **CW-115**, which performs empirical qualification of autonomous model problem-solving, live-provider token savings, and KV-cache performance.
2. **Gate 0 Stream Terminal Invariant:** Resolved upstream SSE stream termination edge-cases. Partial SSE streams ending at HTTP EOF without provider terminal events (`message_stop`, `response.completed`, `[DONE]`) are now classified as `stream_error` instead of falsely recording `completed`.
3. **Deterministic 45-Session Matrix:** Executed the entire 45-session campaign (3 harnesses $\times$ 5 task families $\times$ 3 repetitions) through 8 distinct checkpoint turns per session (total 360 checkpoints, 405 captured & audited model requests in 353.26s on commit `915bf9b4185db350db74ec3332e1db372088e48b`). All 45 sessions achieved a status of **`PASS`**, and all workspace acceptance criteria passed. The campaign suite enforces strict zero-tolerance assertions: `passed_sessions == 45`, `failed_sessions == 0`, `blocked_sessions == 0`, and `total_sessions == 45`.
4. **Native Resume & Per-Turn Overlay Reapplication:** Successfully verified across all three harnesses that committed context overlays (`MutationRule::PinContext`) persist across native CLI process boundaries (`codex exec resume`, `omp -r <uuid>`, `hermes chat -r <sid>`) without corrupting provider conversations or on-disk transcripts. Every post-mutation turn (turns 5..8 across process restarts) is individually verified to contain the committed overlay, while all pre-mutation turns are verified clean.
5. **Phase 4 Fault Injection & Resilience:**
   - **Mid-stream drop:** Proxy records `stream_error` without corrupting downstream client parsers.
   - **HTTP 502 / 429 recovery:** Proxy records `upstream_error` on transient failures and recovers cleanly upon retry (`completed`).
   - **Daemon crash / restart:** Sessions and committed mutations survive full proxy process termination and restart using persistent SQLite storage.
   - **Concurrent session isolation:** Two simultaneous active sessions with distinct pinned policies exhibited zero cross-session contamination or crossover.

---

## 2. Gate 0 Pre-Campaign Fixes

Before initiating the long-session campaign, two critical recorder defects were identified and resolved with deterministic unit and integration tests:

| Defect | Severity | Root Cause | Resolution | Test Verification |
|---|---|---|---|---|
| **Premature EOF on HTTP 200** | P1 | If an SSE stream terminated cleanly at HTTP EOF without delivering a provider terminal event, `forwarding.rs` defaulted to `"completed"`. | Enforced `"stream_error"` when an SSE stream finishes without an explicit terminal outcome. Non-SSE responses continue to complete cleanly. | `test_sse_eof_without_terminal_event_records_stream_error`, `http_200_partial_sse_stream_eof_without_terminal_event_records_stream_error` |
| **Premature Anthropic Stream Delta Termination** | P2 | In `Recorder::handle_sse_data_json`, encountering `stop_reason` in `message_delta` marked the turn completed before the required `message_stop` event arrived. Also partial frames (`event: message_stop` without body) could trigger premature completion. | Required explicit body receipt for `message_stop` and removed `stop_reason` as a completion trigger. | `test_anthropic_stop_reason_does_not_mark_completed_before_message_stop`, `test_incomplete_terminal_sse_frame_does_not_establish_completion` |

### 2.1 Review Findings Remediation & Rigorous Harness Contracts

Following qualification review, the test harness runner in `crates/cw-proxy/tests/long_session_reliability.rs` and the Hermes integration plugin in `plugins/hermes/context-watcher/__init__.py` were hardened against 8 critical qualification invariants:

1. **Zero Provider-Side Filesystem Writes (Harness-Executed Tools Only):**
   - In earlier mock implementations, the mock provider simulated task completion by writing solution files directly to disk.
   - **Correction:** All source code edits, file creation, and environment inspection are now executed strictly through harness tool calls dispatched to the agent process:
     - **Codex CLI:** Dispatches `exec_command` tool calls with inline python hex payloads executing within Codex's `workspace-write` sandbox.
     - **OMP:** Dispatches `write` tool calls writing solution files into the workspace.
     - **Hermes Agent:** Dispatches `terminal` tool calls (`python3 -c ...`) writing solution files into the workspace.
   - 0 provider-side filesystem writes take place; 100% of modifications originate from real agent tool invocations.

2. **Strict Verification of Every Checkpoint & Non-Zero Exit Detection:**
   - The test runner asserts `output.status.success()` on every turn. If any harness process returns a non-zero exit code or times out, the session immediately fails.
   - Acceptance tests are executed and asserted passing after every tool execution before the logical turn count is incremented.

3. **Tool-Pairing Multiplicity Ledger Validation & Defect Rejection:**
   - Rather than relying on `HashSet` set equality (`total_emitted_calls == total_results`), which inadvertently discards duplicate tool call IDs or results (e.g. 2 emitted calls for `call_123` and only 1 result), `verify_captured_tool_pairing_with_error` maintains exact occurrence counts per request using `HashMap<String, usize>` and evaluates a formal `ToolPairingError` ledger:
     - **Responses API (Codex):** Audits `function_call` vs `function_call_output` call IDs within the `input` array.
     - **Chat Completions (OMP & Hermes):** Audits `tool_calls` `id` vs `role: "tool"` `tool_call_id` within the `messages` array.
   - Enforces 6 strict validation invariants:
     - Rejects zero-tool runs: `ToolPairingError::NoToolCallsObserved`.
     - Rejects duplicate emitted call IDs in the same request: `ToolPairingError::DuplicateEmittedCallId`.
     - Rejects duplicate tool results for the same call ID: `ToolPairingError::DuplicateResult`.
     - Rejects orphaned tool results without an emitted call: `ToolPairingError::OrphanedResult`.
     - Rejects missing results for an emitted tool call: `ToolPairingError::MissingResult`.
     - Rejects count mismatches hidden by set equality: `ToolPairingError::MultiplicityMismatch`.
   - **Legitimate Historical Replay Support:** Correctly accounts for multi-turn LLM harnesses that replay earlier conversation turns on every subsequent model request. Replayed historical pairs maintain 1:1 balance in subsequent requests and do not register spurious new tool invocations.
   - Verified by 10 negative regression and replay unit tests covering both OpenAI Responses and Chat Completions protocols.

4. **Per-Turn Native Session ID Resume Verification Across Process Boundaries:**
   - Native session ID continuity is enforced on every single resume turn across process restarts:
     - **Codex CLI:** Asserts `thread.started` `thread_id` on every turn (turns 2..8) exactly matches the initial `thread_id` established on turn 1.
     - **OMP:** Asserts the persistent session JSONL file `<temp_agent_dir>/sessions/*<native_sid>*.jsonl` is preserved and appended to across all turns 2..8.
     - **Hermes Agent:** Asserts the returned session identifier in `hermes chat -r <sid>` output exactly matches `native_sid` on every turn 2..8.
   - Failure to preserve session identity on any turn fails the session qualification immediately with a descriptive failure class.

5. **Individual Per-Turn Overlay Reapplication Across Process Restarts:**
   - Instead of checking if an overlay appears on any turn, the suite enforces exact lifecycle timing:
     - All pre-mutation turns (prior to overlay commitment) are asserted clean: `!payload.contains("CW114_POLICY_ACTIVE")`.
     - All post-mutation turns (turns 5..8 across process restarts) are individually asserted to contain `"CW114_POLICY_ACTIVE"`, verifying that the deterministic overlay is reapplied on every subsequent request across CLI process invocations.

6. **Complete Hermes Isolation & Launcher Fallback Elimination:**
   - The test runner executes Hermes by resolving the committed venv interpreter (`.../environments/*/venv/bin/python -m hermes_cli.main`) directly from the binary's installation directory.
   - Because `sys.prefix != sys.base_prefix` in the committed venv, Hermes skips its internal `publish_launchers` and package-manager synchronization routine.
   - Zero writes to user launcher scripts (`~/.hermes/hermes-agent/.hermes/bin/*`) occur.
   - **No Fallback to Installed Launcher:** If virtual-environment resolution fails (`resolve_hermes_venv_python` returns `None`), the runner immediately aborts with `status: "BLOCKED"` and an actionable failure message, never falling back to executing the installed launcher (`hermes_bin`). Verified by unit test `test_hermes_venv_resolution_failure_blocks_qualification_without_executing_launcher`.
   - RAII cleanup (`HermesCleanupGuard`) deletes only temporary test-created directories (`temp_home` and `temp_trusted_dir`), guaranteeing 100% isolation of user installation state.

7. **Zero Invented Session ID Fallbacks:**
   - If a native session ID cannot be recovered from harness output, the qualification runner immediately fails qualification (`status: "FAIL"`) with an explicit failure class rather than falling back to an invented ID.

8. **Strict Zero-Tolerance Campaign Assertions:**
   - The 45-session campaign suite in `test_cw114_deterministic_45_session_campaign` explicitly asserts:
     ```rust
     assert_eq!(report.passed_sessions, 45);
     assert_eq!(report.failed_sessions, 0);
     assert_eq!(report.blocked_sessions, 0);
     assert_eq!(report.total_sessions, 45);
     ```
   - Guaranteeing that skipped or blocked sessions immediately fail qualification.

---

## 3. Workload Task Family Specifications

The campaign qualified agent reliability across 5 deterministic coding-task families, each designed with failing initial conditions, specific code repair steps, and automated acceptance test suites:

### F1: Bug Fixing (`F1_BugFixing`)
- **Target File:** `math_ops.py`
- **Initial Defect:** Unhandled `ZeroDivisionError` in `divide_or_fallback(a, b, fallback=0.0)` and `ZeroDivisionError` on empty lists in `safe_mean(numbers)`.
- **Repair Objective:** Guard division by zero and handle empty sequences by returning fallback values.
- **Acceptance Criterion:** `python3 test_math_ops.py` exits with status `0`.

### F2: Refactoring (`F2_Refactoring`)
- **Target File:** `string_utils.py`
- **Initial Defect:** Duplicate truncation and ellipsis logic across `format_header` and `format_footer`.
- **Repair Objective:** Extract common private helper `_truncate_with_ellipsis(text, max_len)` and delegate formatting without breaking case transformations.
- **Acceptance Criterion:** `python3 test_string_utils.py` verifies both formatting functions and direct helper assertions.

### F3: Feature Implementation (`F3_FeatureImplementation`)
- **Target File:** `cache.py`
- **Initial Defect:** `CachedStore` class is absent or empty.
- **Repair Objective:** Implement `CachedStore(backing_store)` with `get(k)` (caching non-None values from backing store) and `put(k, v)` (writing to both memory cache and backing store).
- **Acceptance Criterion:** `python3 test_cache.py` asserts cache hits, backing store delegation, and cache invalidation.

### F4: Test-Driven Development (`F4_Tdd`)
- **Target File:** `validator.py`
- **Initial Defect:** `validate_registration(username, email, age)` is not implemented.
- **Repair Objective:** Implement validation rules: non-empty username, valid email containing `@` and `.`, and age range $18 \le \text{age} \le 120$.
- **Acceptance Criterion:** `python3 test_validator.py` passes 6 exhaustive test cases covering boundary values.

### F5: Investigation & Recovery (`F5_InvestigationRecovery`)
- **Target File:** `pipeline.py`
- **Initial Defect:** `run_pipeline(records)` crashes or returns corrupted records.
- **Repair Objective:** Implement robust record extraction filtering `score >= 70` and sorting descending by score.
- **Acceptance Criterion:** `python3 test_pipeline.py` verifies filtering threshold, descending order, and missing key resilience.

---

## 4. Multi-Harness 45-Session Matrix Results

All 45 sessions were executed against installed harness binaries (`codex`, `omp`, `hermes`) over an isolated loopback proxy and deterministic mock provider:

| # | Session ID | Harness | Task Family | Rep | Logical Turns | Model Requests | Resume Preserved | Overlay Applied | Tool Pairing | Acceptance | Status |
|:---:|:---|:---|:---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| 1 | `cw114_codex_F1_rep1` | Codex CLI | F1 Bug Fixing | 1 | 8 | 9 | Yes | Yes | Yes | Pass | **PASS** |
| 2 | `cw114_codex_F1_rep2` | Codex CLI | F1 Bug Fixing | 2 | 8 | 9 | Yes | Yes | Yes | Pass | **PASS** |
| 3 | `cw114_codex_F1_rep3` | Codex CLI | F1 Bug Fixing | 3 | 8 | 9 | Yes | Yes | Yes | Pass | **PASS** |
| 4 | `cw114_codex_F2_rep1` | Codex CLI | F2 Refactoring | 1 | 8 | 9 | Yes | Yes | Yes | Pass | **PASS** |
| 5 | `cw114_codex_F2_rep2` | Codex CLI | F2 Refactoring | 2 | 8 | 9 | Yes | Yes | Yes | Pass | **PASS** |
| 6 | `cw114_codex_F2_rep3` | Codex CLI | F2 Refactoring | 3 | 8 | 9 | Yes | Yes | Yes | Pass | **PASS** |
| 7 | `cw114_codex_F3_rep1` | Codex CLI | F3 Feature Impl | 1 | 8 | 9 | Yes | Yes | Yes | Pass | **PASS** |
| 8 | `cw114_codex_F3_rep2` | Codex CLI | F3 Feature Impl | 2 | 8 | 9 | Yes | Yes | Yes | Pass | **PASS** |
| 9 | `cw114_codex_F3_rep3` | Codex CLI | F3 Feature Impl | 3 | 8 | 9 | Yes | Yes | Yes | Pass | **PASS** |
| 10 | `cw114_codex_F4_rep1` | Codex CLI | F4 TDD | 1 | 8 | 9 | Yes | Yes | Yes | Pass | **PASS** |
| 11 | `cw114_codex_F4_rep2` | Codex CLI | F4 TDD | 2 | 8 | 9 | Yes | Yes | Yes | Pass | **PASS** |
| 12 | `cw114_codex_F4_rep3` | Codex CLI | F4 TDD | 3 | 8 | 9 | Yes | Yes | Yes | Pass | **PASS** |
| 13 | `cw114_codex_F5_rep1` | Codex CLI | F5 Recovery | 1 | 8 | 9 | Yes | Yes | Yes | Pass | **PASS** |
| 14 | `cw114_codex_F5_rep2` | Codex CLI | F5 Recovery | 2 | 8 | 9 | Yes | Yes | Yes | Pass | **PASS** |
| 15 | `cw114_codex_F5_rep3` | Codex CLI | F5 Recovery | 3 | 8 | 9 | Yes | Yes | Yes | Pass | **PASS** |
| 16 | `cw114_omp_F1_rep1` | OMP | F1 Bug Fixing | 1 | 8 | 9 | Yes | Yes | Yes | Pass | **PASS** |
| 17 | `cw114_omp_F1_rep2` | OMP | F1 Bug Fixing | 2 | 8 | 9 | Yes | Yes | Yes | Pass | **PASS** |
| 18 | `cw114_omp_F1_rep3` | OMP | F1 Bug Fixing | 3 | 8 | 9 | Yes | Yes | Yes | Pass | **PASS** |
| 19 | `cw114_omp_F2_rep1` | OMP | F2 Refactoring | 1 | 8 | 9 | Yes | Yes | Yes | Pass | **PASS** |
| 20 | `cw114_omp_F2_rep2` | OMP | F2 Refactoring | 2 | 8 | 9 | Yes | Yes | Yes | Pass | **PASS** |
| 21 | `cw114_omp_F2_rep3` | OMP | F2 Refactoring | 3 | 8 | 9 | Yes | Yes | Yes | Pass | **PASS** |
| 22 | `cw114_omp_F3_rep1` | OMP | F3 Feature Impl | 1 | 8 | 9 | Yes | Yes | Yes | Pass | **PASS** |
| 23 | `cw114_omp_F3_rep2` | OMP | F3 Feature Impl | 2 | 8 | 9 | Yes | Yes | Yes | Pass | **PASS** |
| 24 | `cw114_omp_F3_rep3` | OMP | F3 Feature Impl | 3 | 8 | 9 | Yes | Yes | Yes | Pass | **PASS** |
| 25 | `cw114_omp_F4_rep1` | OMP | F4 TDD | 1 | 8 | 9 | Yes | Yes | Yes | Pass | **PASS** |
| 26 | `cw114_omp_F4_rep2` | OMP | F4 TDD | 2 | 8 | 9 | Yes | Yes | Yes | Pass | **PASS** |
| 27 | `cw114_omp_F4_rep3` | OMP | F4 TDD | 3 | 8 | 9 | Yes | Yes | Yes | Pass | **PASS** |
| 28 | `cw114_omp_F5_rep1` | OMP | F5 Recovery | 1 | 8 | 9 | Yes | Yes | Yes | Pass | **PASS** |
| 29 | `cw114_omp_F5_rep2` | OMP | F5 Recovery | 2 | 8 | 9 | Yes | Yes | Yes | Pass | **PASS** |
| 30 | `cw114_omp_F5_rep3` | OMP | F5 Recovery | 3 | 8 | 9 | Yes | Yes | Yes | Pass | **PASS** |
| 31 | `cw114_hermes_F1_rep1` | Hermes Agent | F1 Bug Fixing | 1 | 8 | 9 | Yes | Yes | Yes | Pass | **PASS** |
| 32 | `cw114_hermes_F1_rep2` | Hermes Agent | F1 Bug Fixing | 2 | 8 | 9 | Yes | Yes | Yes | Pass | **PASS** |
| 33 | `cw114_hermes_F1_rep3` | Hermes Agent | F1 Bug Fixing | 3 | 8 | 9 | Yes | Yes | Yes | Pass | **PASS** |
| 34 | `cw114_hermes_F2_rep1` | Hermes Agent | F2 Refactoring | 1 | 8 | 9 | Yes | Yes | Yes | Pass | **PASS** |
| 35 | `cw114_hermes_F2_rep2` | Hermes Agent | F2 Refactoring | 2 | 8 | 9 | Yes | Yes | Yes | Pass | **PASS** |
| 36 | `cw114_hermes_F2_rep3` | Hermes Agent | F2 Refactoring | 3 | 8 | 9 | Yes | Yes | Yes | Pass | **PASS** |
| 37 | `cw114_hermes_F3_rep1` | Hermes Agent | F3 Feature Impl | 1 | 8 | 9 | Yes | Yes | Yes | Pass | **PASS** |
| 38 | `cw114_hermes_F3_rep2` | Hermes Agent | F3 Feature Impl | 2 | 8 | 9 | Yes | Yes | Yes | Pass | **PASS** |
| 39 | `cw114_hermes_F3_rep3` | Hermes Agent | F3 Feature Impl | 3 | 8 | 9 | Yes | Yes | Yes | Pass | **PASS** |
| 40 | `cw114_hermes_F4_rep1` | Hermes Agent | F4 TDD | 1 | 8 | 9 | Yes | Yes | Yes | Pass | **PASS** |
| 41 | `cw114_hermes_F4_rep2` | Hermes Agent | F4 TDD | 2 | 8 | 9 | Yes | Yes | Yes | Pass | **PASS** |
| 42 | `cw114_hermes_F4_rep3` | Hermes Agent | F4 TDD | 3 | 8 | 9 | Yes | Yes | Yes | Pass | **PASS** |
| 43 | `cw114_hermes_F5_rep1` | Hermes Agent | F5 Recovery | 1 | 8 | 9 | Yes | Yes | Yes | Pass | **PASS** |
| 44 | `cw114_hermes_F5_rep2` | Hermes Agent | F5 Recovery | 2 | 8 | 9 | Yes | Yes | Yes | Pass | **PASS** |
| 45 | `cw114_hermes_F5_rep3` | Hermes Agent | F5 Recovery | 3 | 8 | 9 | Yes | Yes | Yes | Pass | **PASS** |

### Summary Statistics
- **Total Sessions Executed:** 45 / 45 (100%)
- **Passed Sessions:** 45 (100.0%)
- **Failed Sessions:** 0 (0.0%)
- **Blocked / Incomplete:** 0 (0.0%)
- **Total Checkpoints / Logical Turns Qualified:** 360 turns
- **Total Model Requests Captured & Audited:** 405 requests
- **Total Execution Duration:** 353.26s (5m 53s)

---

## 5. Phase 4 Fault-Injection & Resilience Suite Results

| Test Case | Description | Expected Outcome | Observed Outcome | Status |
|---|---|---|---|:---:|
| `test_cw114_fault_injection_mid_stream_drop_recorded_as_stream_error` | Mock upstream closes SSE connection mid-stream after sending partial chunks without terminal `[DONE]` or finish event. | Proxy marks turn outcome as `stream_error`. Client receives EOF without crash or corrupt state. | `turn.outcome == Some("stream_error")` confirmed after stream consumption. | **PASS** |
| `test_cw114_fault_injection_http_502_and_429_recovery` | Mock upstream returns 502 Bad Gateway on turn 1, 429 Too Many Requests on turn 2, then recovers with 200 OK on turn 3. | Turns 1 & 2 record `upstream_error`; Turn 3 records `completed` cleanly without state corruption. | Outbound turn status array: `["upstream_error", "upstream_error", "completed"]`. | **PASS** |
| `test_cw114_resilience_daemon_restart_persisted_state` | Proxy instance A executes turn 1 and commits an active pin overlay in persistent SQLite storage. Proxy A is dropped. Proxy instance B starts up on the same SQLite file and receives resumed turn 2. | Persistent sessions and committed overlays survive daemon process crash/restart; overlay applies to post-restart request. | Turn 2 outbound payload contains `CW114_RESTART_ACTIVE_PIN`. Both turns preserved in SQLite. | **PASS** |
| `test_cw114_resilience_concurrent_sessions_distinct_overlays` | Two sessions (`sess_A` and `sess_B`) dispatch concurrent requests through the proxy with distinct policy pins (`POLICY_A_ONLY` vs `POLICY_B_ONLY`). | Strict zero-crossover isolation between concurrent sessions. | Session A payload contains Policy A only; Session B payload contains Policy B only. Zero leak detected. | **PASS** |

---

## 6. Harness-Specific Integration Invariants & Learnings

### Codex CLI (`codex-cli 0.166.0`)
- **Responses API Invariant:** Codex CLI expects the standard OpenAI Responses streaming event sequence:
  `response.created` $\rightarrow$ `response.output_item.added` $\rightarrow$ `response.content_part.added` $\rightarrow$ `response.output_text.delta` $\rightarrow$ `response.output_text.done` $\rightarrow$ `response.content_part.done` $\rightarrow$ `response.output_item.done` $\rightarrow$ `response.completed` $\rightarrow$ `[DONE]`.
  Omitting intermediate content-part delimiters or the final `output` array inside `response.completed` can cause the CLI parser to await chunks indefinitely.
- **Environment Isolation:** Setting `approval_policy="never"` and `sandbox_mode="workspace-write"` ensures completely unattended multi-turn execution without interactive prompt stalls.

### OMP (`omp 18.12.6`)
- **JSON Completions Mode:** In default `openai-completions` mode, OMP dispatches non-streaming JSON payloads (`stream: false`). The mock/proxy upstream must return a full `chat.completion` JSON object rather than SSE chunks.
- **Runtime Hook Context Transform:** The native Pi extension (`plugins/pi-opencode/index.js`) intercepts the in-memory context via `pi.on("context")`, calls `POST /api/v1/sessions/:sid/transform` on the local daemon, and applies active overlays before model dispatch. This leaves disk session transcripts 100% immutable and intact.
- **Session Identification:** OMP writes sessions to `<agent_dir>/sessions/<hash>/<timestamp>_<uuid>.jsonl`. The canonical ContextWatcher session identifier is `format!("omp_{uuid}")`.

### Hermes Agent (`v0.22.5`)
- **Middleware Passthrough:** Hermes forwards client environment variables declared in `terminal.client_env_passthrough`. The middleware plugin (`plugins/hermes/context-watcher/__init__.py`) reads `CW_CONTROL_TOKEN_FILE` or falls back to the default `~/.local/share/context-watcher/control-token`. Initializing proxy daemon state with this token guarantees seamless 403-free session binding.
- **Native Resume Identification:** Hermes outputs its native session ID as `session_id: local-<hash>` on Turn 1 stderr/stdout. Subsequent turns cleanly resume using `hermes chat -r <session_id>`.

---

## 7. Command Reference for Reproducibility

```bash
# 1. Run all Gate 0 pre-campaign regression tests and tool-pairing ledger tests
cargo test -p cw-proxy --test safety http_200_partial_sse_stream_eof_without_terminal_event_records_stream_error
cargo test -p cw-proxy recorder::test
cargo test -p cw-proxy --test long_session_reliability test_tool_pairing
cargo test -p cw-proxy --test long_session_reliability test_hermes_venv_resolution_failure

# 2. Run deterministic single-session tests for each harness
cargo test -p cw-proxy --test long_session_reliability test_cw114_single_session_codex_deterministic -- --nocapture
cargo test -p cw-proxy --test long_session_reliability test_cw114_single_session_omp_deterministic -- --nocapture
cargo test -p cw-proxy --test long_session_reliability test_cw114_single_session_hermes_deterministic -- --nocapture

# 3. Run Phase 4 Fault-Injection and Resilience suite
cargo test -p cw-proxy --test long_session_reliability test_cw114_fault_injection -- --nocapture
cargo test -p cw-proxy --test long_session_reliability test_cw114_resilience -- --nocapture

# 4. Run the full deterministic 45-session campaign (3 harnesses x 5 task families x 3 reps)
cargo test -p cw-proxy --test long_session_reliability test_cw114_deterministic_45_session_campaign -- --ignored --nocapture

# 5. Full workspace verification checks
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
python3 plugins/hermes/context-watcher/test_plugin.py
```
