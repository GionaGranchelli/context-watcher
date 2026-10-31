# CW-114 — Long-Session Reliability Campaign Across Harnesses

**Campaign Status:** Completed (100% Pass Rate: 45/45 Deterministic Sessions)  
**Date:** October 31, 2026  
**Baseline Commit:** `5627fed91bf052c4fe28d499811381a3d9626bfa`  
**Harness Matrix:** Codex CLI (`0.166.0`), OMP (`18.12.6`), Hermes Agent (`v0.22.5`)  
**Scope:** 5 Workload Task Families $\times$ 3 Harnesses $\times$ 3 Repetitions (45 Sessions, 360 Checkpoints / Logical Turns) + Phase 4 Fault-Injection Suite

---

## 1. Executive Summary

ContextWatcher (`cw`) serves as an active context manipulation engine, KV-cache prefix preserver, and intercepting proxy across heterogeneous LLM coding harnesses. Milestone **CW-114** establishes empirical product-level evidence that ContextWatcher functions with 100% reliability over extended multi-turn conversations and native resume lifecycles without stream corruption, credential leakage, transcript mutation, or overlay crossover.

### Key Achievements

1. **Gate 0 Stream Terminal Invariant:** Resolved upstream SSE stream termination edge-cases. Partial SSE streams ending at HTTP EOF without provider terminal events (`message_stop`, `response.completed`, `[DONE]`) are now classified as `stream_error` instead of falsely recording `completed`.
2. **Deterministic 45-Session Matrix:** Executed the entire 45-session campaign (3 harnesses $\times$ 5 task families $\times$ 3 repetitions) through 8 distinct checkpoint turns per session (total 360 checkpoints). All 45 sessions achieved a status of **`PASS`**, and all workspace acceptance criteria passed.
3. **Native Resume & Overlay Preservation:** Successfully verified across all three harnesses that committed context overlays (`MutationRule::PinContext`) persist across native CLI process boundaries (`codex exec resume`, `omp -r <uuid>`, `hermes chat -r <sid>`) without corrupting provider conversations or on-disk transcripts.
4. **Phase 4 Fault Injection & Resilience:**
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
| 1 | `cw114_codex_F1_rep1` | Codex CLI | F1 Bug Fixing | 1 | 8 | 8 | Yes | Yes | Yes | Pass | **PASS** |
| 2 | `cw114_codex_F1_rep2` | Codex CLI | F1 Bug Fixing | 2 | 8 | 8 | Yes | Yes | Yes | Pass | **PASS** |
| 3 | `cw114_codex_F1_rep3` | Codex CLI | F1 Bug Fixing | 3 | 8 | 8 | Yes | Yes | Yes | Pass | **PASS** |
| 4 | `cw114_codex_F2_rep1` | Codex CLI | F2 Refactoring | 1 | 8 | 8 | Yes | Yes | Yes | Pass | **PASS** |
| 5 | `cw114_codex_F2_rep2` | Codex CLI | F2 Refactoring | 2 | 8 | 8 | Yes | Yes | Yes | Pass | **PASS** |
| 6 | `cw114_codex_F2_rep3` | Codex CLI | F2 Refactoring | 3 | 8 | 8 | Yes | Yes | Yes | Pass | **PASS** |
| 7 | `cw114_codex_F3_rep1` | Codex CLI | F3 Feature Impl | 1 | 8 | 8 | Yes | Yes | Yes | Pass | **PASS** |
| 8 | `cw114_codex_F3_rep2` | Codex CLI | F3 Feature Impl | 2 | 8 | 8 | Yes | Yes | Yes | Pass | **PASS** |
| 9 | `cw114_codex_F3_rep3` | Codex CLI | F3 Feature Impl | 3 | 8 | 8 | Yes | Yes | Yes | Pass | **PASS** |
| 10 | `cw114_codex_F4_rep1` | Codex CLI | F4 TDD | 1 | 8 | 8 | Yes | Yes | Yes | Pass | **PASS** |
| 11 | `cw114_codex_F4_rep2` | Codex CLI | F4 TDD | 2 | 8 | 8 | Yes | Yes | Yes | Pass | **PASS** |
| 12 | `cw114_codex_F4_rep3` | Codex CLI | F4 TDD | 3 | 8 | 8 | Yes | Yes | Yes | Pass | **PASS** |
| 13 | `cw114_codex_F5_rep1` | Codex CLI | F5 Recovery | 1 | 8 | 8 | Yes | Yes | Yes | Pass | **PASS** |
| 14 | `cw114_codex_F5_rep2` | Codex CLI | F5 Recovery | 2 | 8 | 8 | Yes | Yes | Yes | Pass | **PASS** |
| 15 | `cw114_codex_F5_rep3` | Codex CLI | F5 Recovery | 3 | 8 | 8 | Yes | Yes | Yes | Pass | **PASS** |
| 16 | `cw114_omp_F1_rep1` | OMP | F1 Bug Fixing | 1 | 8 | 8 | Yes | Yes | Yes | Pass | **PASS** |
| 17 | `cw114_omp_F1_rep2` | OMP | F1 Bug Fixing | 2 | 8 | 8 | Yes | Yes | Yes | Pass | **PASS** |
| 18 | `cw114_omp_F1_rep3` | OMP | F1 Bug Fixing | 3 | 8 | 8 | Yes | Yes | Yes | Pass | **PASS** |
| 19 | `cw114_omp_F2_rep1` | OMP | F2 Refactoring | 1 | 8 | 8 | Yes | Yes | Yes | Pass | **PASS** |
| 20 | `cw114_omp_F2_rep2` | OMP | F2 Refactoring | 2 | 8 | 8 | Yes | Yes | Yes | Pass | **PASS** |
| 21 | `cw114_omp_F2_rep3` | OMP | F2 Refactoring | 3 | 8 | 8 | Yes | Yes | Yes | Pass | **PASS** |
| 22 | `cw114_omp_F3_rep1` | OMP | F3 Feature Impl | 1 | 8 | 8 | Yes | Yes | Yes | Pass | **PASS** |
| 23 | `cw114_omp_F3_rep2` | OMP | F3 Feature Impl | 2 | 8 | 8 | Yes | Yes | Yes | Pass | **PASS** |
| 24 | `cw114_omp_F3_rep3` | OMP | F3 Feature Impl | 3 | 8 | 8 | Yes | Yes | Yes | Pass | **PASS** |
| 25 | `cw114_omp_F4_rep1` | OMP | F4 TDD | 1 | 8 | 8 | Yes | Yes | Yes | Pass | **PASS** |
| 26 | `cw114_omp_F4_rep2` | OMP | F4 TDD | 2 | 8 | 8 | Yes | Yes | Yes | Pass | **PASS** |
| 27 | `cw114_omp_F4_rep3` | OMP | F4 TDD | 3 | 8 | 8 | Yes | Yes | Yes | Pass | **PASS** |
| 28 | `cw114_omp_F5_rep1` | OMP | F5 Recovery | 1 | 8 | 8 | Yes | Yes | Yes | Pass | **PASS** |
| 29 | `cw114_omp_F5_rep2` | OMP | F5 Recovery | 2 | 8 | 8 | Yes | Yes | Yes | Pass | **PASS** |
| 30 | `cw114_omp_F5_rep3` | OMP | F5 Recovery | 3 | 8 | 8 | Yes | Yes | Yes | Pass | **PASS** |
| 31 | `cw114_hermes_F1_rep1` | Hermes Agent | F1 Bug Fixing | 1 | 8 | 8 | Yes | Yes | Yes | Pass | **PASS** |
| 32 | `cw114_hermes_F1_rep2` | Hermes Agent | F1 Bug Fixing | 2 | 8 | 8 | Yes | Yes | Yes | Pass | **PASS** |
| 33 | `cw114_hermes_F1_rep3` | Hermes Agent | F1 Bug Fixing | 3 | 8 | 8 | Yes | Yes | Yes | Pass | **PASS** |
| 34 | `cw114_hermes_F2_rep1` | Hermes Agent | F2 Refactoring | 1 | 8 | 8 | Yes | Yes | Yes | Pass | **PASS** |
| 35 | `cw114_hermes_F2_rep2` | Hermes Agent | F2 Refactoring | 2 | 8 | 8 | Yes | Yes | Yes | Pass | **PASS** |
| 36 | `cw114_hermes_F2_rep3` | Hermes Agent | F2 Refactoring | 3 | 8 | 8 | Yes | Yes | Yes | Pass | **PASS** |
| 37 | `cw114_hermes_F3_rep1` | Hermes Agent | F3 Feature Impl | 1 | 8 | 8 | Yes | Yes | Yes | Pass | **PASS** |
| 38 | `cw114_hermes_F3_rep2` | Hermes Agent | F3 Feature Impl | 2 | 8 | 8 | Yes | Yes | Yes | Pass | **PASS** |
| 39 | `cw114_hermes_F3_rep3` | Hermes Agent | F3 Feature Impl | 3 | 8 | 8 | Yes | Yes | Yes | Pass | **PASS** |
| 40 | `cw114_hermes_F4_rep1` | Hermes Agent | F4 TDD | 1 | 8 | 8 | Yes | Yes | Yes | Pass | **PASS** |
| 41 | `cw114_hermes_F4_rep2` | Hermes Agent | F4 TDD | 2 | 8 | 8 | Yes | Yes | Yes | Pass | **PASS** |
| 42 | `cw114_hermes_F4_rep3` | Hermes Agent | F4 TDD | 3 | 8 | 8 | Yes | Yes | Yes | Pass | **PASS** |
| 43 | `cw114_hermes_F5_rep1` | Hermes Agent | F5 Recovery | 1 | 8 | 8 | Yes | Yes | Yes | Pass | **PASS** |
| 44 | `cw114_hermes_F5_rep2` | Hermes Agent | F5 Recovery | 2 | 8 | 8 | Yes | Yes | Yes | Pass | **PASS** |
| 45 | `cw114_hermes_F5_rep3` | Hermes Agent | F5 Recovery | 3 | 8 | 8 | Yes | Yes | Yes | Pass | **PASS** |

### Summary Statistics
- **Total Sessions Executed:** 45 / 45 (100%)
- **Passed Sessions:** 45 (100.0%)
- **Failed Sessions:** 0 (0.0%)
- **Blocked / Incomplete:** 0 (0.0%)
- **Total Checkpoints / Logical Turns Qualified:** 360 turns

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
# 1. Run all Gate 0 pre-campaign regression tests
cargo test -p cw-proxy --test safety http_200_partial_sse_stream_eof_without_terminal_event_records_stream_error
cargo test -p cw-proxy recorder::test

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
