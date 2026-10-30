# CW-112 — Codex CLI Real-Provider Multi-Turn & Native Resume Qualification Report

This document records the empirical verification evidence for **CW-112: Codex CLI Multi-Turn & Native Resume Qualification**, qualifying ContextWatcher's integration with the installed **Codex CLI (`0.165.0`)** through its authenticated OpenAI Responses proxy.

---

## 1. Executive Summary & Qualification Matrix

| Qualification Gate | Target | Status | Evidence / Test Case |
|---|---|:---:|---|
| **Binary Installation** | Codex CLI `0.165.0` | 🟢 **Verified** | Installed at `/home/gionag/.npm-global/bin/codex` (`codex-cli 0.165.0`) |
| **Native Resume Multi-Turn** | 3-Turn Coding Task + Resume | 🟢 **Verified** | `test_harness_binary_codex_cli_native_resume_three_turn_qualification` |
| **Overlay Reapplication** | `PinContext` across Resume | 🟢 **Verified** | Verified overlay injection on Turns 2 and 3; unmutated on Turn 1 |
| **Rollout Integrity** | No Injected Overlays on Disk | 🟢 **Verified** | `CODEX_HOME/sessions` rollouts validated as valid JSONL with 0 injected overlays (in-flight proxying) |
| **Credential Sandboxing** | Upstream Stripping & Tool Isolation | 🟢 **Verified** | Upstream wire requests stripped of `x-cw-*`; tool execution directly verified free of `CW_CONTROL_TOKEN` |
| **Unknown Session Fault** | Non-existent UUID Resume | 🟢 **Verified** | `test_harness_binary_codex_cli_unknown_native_session_id_resume` |
| **Unauthorized Rejection** | Missing / Invalid Token | 🟢 **Verified** | `test_harness_binary_codex_cli_unauthorized_token_rejection` (403 Forbidden) |
| **Upstream 502 Handling** | Gateway Error Recovery | 🟢 **Verified** | `test_harness_binary_codex_cli_upstream_failure_handling` (store uncorrupted) |
| **Session Isolation** | Distinct Sessions Separation | 🟢 **Verified** | `test_harness_binary_codex_cli_session_isolation` (Session B isolated from A) |
| **Live Provider Continuation** | Real OpenAI Responses API | 🟡 **LIVE QUALIFICATION BLOCKED** | Gated on `CW_QUALIFY_REAL_CODEX=1` and spend ceiling authorization |

---

## 2. Multi-Turn Native Resume Lifecycle

Qualification was executed in an isolated temporary directory environment (`CODEX_HOME` and dedicated workspace) against an authenticated ContextWatcher daemon instance with a local Responses API mock provider.

### Turn 1: Initial Session & Tool Execution
1. **Command Execution:**
   ```bash
   codex exec --json --dangerously-bypass-approvals-and-sandbox --skip-git-repo-check --ignore-user-config \
     --config model_provider="context_watcher" \
     --config model_providers.context_watcher.name="contextWatcher" \
     --config model_providers.context_watcher.base_url="http://127.0.0.1:<proxy_port>/openai/v1" \
     --config model_providers.context_watcher.wire_api="responses" \
     --config model_providers.context_watcher.requires_openai_auth=true \
     --config model_providers.context_watcher.env_key="OPENAI_API_KEY" \
     --config model_providers.context_watcher.env_http_headers='{"x-cw-session-id"="CW_SESSION_ID","x-cw-control-token"="CW_CONTROL_TOKEN"}' \
     --config shell_environment_policy.filters.CW_CONTROL_TOKEN="exclude" \
     -m mock-model "CW112_TASK_TURN_1: create calculator.py with add function"
   ```
2. **Native Thread Creation:**
   Codex initiated a persistent native session, emitting:
   ```json
   {"type": "thread.started", "thread_id": "019a6b..."}
   ```
   The native `thread_id` was parsed and captured.
3. **Responses API Tool Interaction & Credential Sandboxing Check:**
   The mock provider returned an SSE streaming function call for `exec_command` with payload:
   ```python
   python3 -c "import os; open('env_check.txt', 'w').write(os.environ.get('CW_CONTROL_TOKEN', 'ABSENT')); open('calculator.py', 'w').write('def add(a, b):\n    return a + b\n')"
   ```
   Codex executed the command in its dedicated workspace and returned `function_call_output`.
4. **Completion & Sandboxing Verification:**
   - Codex concluded Turn 1 with exit code 0.
   - `calculator.py` was verified on disk containing `def add(a, b):`.
   - `env_check.txt` was verified containing `"ABSENT"`, directly proving that the model-invoked tool subprocess did not inherit `CW_CONTROL_TOKEN`.

### Mutation Overlay Commitment
Between Turn 1 and Turn 2, an active overlay rule was saved to the ContextWatcher store:
```rust
f.store.save_mutation(&MutationRule {
    id: "cw112_pin_rule".into(),
    session_id: cw_session_id.clone(),
    op: MutationOp::PinContext {
        label: "Policy".into(),
        content: "CW112_POLICY_ACTIVE".into(),
        source_node_id: None,
    },
    status: MutationStatus::Committed,
    estimated_saved_tokens: 0,
    estimated_kv_break_tokens: 0,
    created_at: Utc::now(),
}).unwrap();
```

### Turn 2: Native Resume & Overlay Injection
1. **Command Execution:**
   ```bash
   codex exec resume --json --dangerously-bypass-approvals-and-sandbox --skip-git-repo-check --ignore-user-config \
     <proxy_config_args> -m mock-model <thread_id> "CW112_TASK_TURN_2: add multiply function to calculator.py"
   ```
2. **Native Resume Verification:**
   Codex emitted `{"type": "thread.started", "thread_id": "019a6b..."}`. Assertions verified that the native `thread_id` on Turn 2 matched Turn 1 exactly.
3. **Overlay Injection Verification:**
   ContextWatcher intercepted the outbound Responses payload and injected the pinned overlay block:
   ```
   <cw_pinned_context>
   - [Policy]: CW112_POLICY_ACTIVE
   </cw_pinned_context>
   ```
   Captured upstream request inspection confirmed `CW112_POLICY_ACTIVE` was present in the outbound payload.
4. **Tool Execution:**
   The mock returned an `exec_command` tool call appending `multiply`:
   ```python
   python3 -c "with open('calculator.py', 'a') as f: f.write('def multiply(a, b):\n    return a * b\n')"
   ```
   `calculator.py` was verified on disk containing both `def add` and `def multiply`.

### Turn 3: Second Native Resume & Verification
1. **Command Execution:**
   ```bash
   codex exec resume --json --dangerously-bypass-approvals-and-sandbox --skip-git-repo-check --ignore-user-config \
     <proxy_config_args> -m mock-model <thread_id> "CW112_TASK_TURN_3: create test_calculator.py and run unit tests"
   ```
2. **Thread Continuity:** Native `thread_id` verified matching Turn 1 and Turn 2.
3. **Overlay Reapplication:** ContextWatcher re-applied the `CW112_POLICY_ACTIVE` overlay across the second native resume boundary.
4. **Unit Test Execution:**
   The mock returned an `exec_command` writing `test_calculator.py` and running unit tests.
   `test_calculator.py` was created and executed directly via `python3 test_calculator.py`, passing all assertions with status code 0.

---

## 3. Security, Protocol & Storage Guarantees

### Credential Sandboxing & Header Stripping
- **Authentication Separation:** ContextWatcher daemon uses `CW_CONTROL_TOKEN` for proxy authentication, passed via `x-cw-control-token` header. The upstream LLM provider uses `OPENAI_API_KEY`, passed via `Authorization: Bearer <key>`.
- **Wire Stripping:** 100% of requests captured by the upstream provider were inspected. No request contained `x-cw-control-token`, `x-cw-session-id`, or any header beginning with `x-cw-`.
- **Subprocess Isolation Directly Tested:** Codex configuration flag `shell_environment_policy.filters.CW_CONTROL_TOKEN="exclude"` was validated in-test: the tool command executed by Codex inspected `os.environ` and wrote `env_check.txt` containing `"ABSENT"`, proving the daemon token is never leaked to model-invoked shell commands.

### Responses API Wire Protocol
- ContextWatcher correctly normalizes and reconstructs the OpenAI Responses wire schema (`input` array containing `message`, `function_call`, and `function_call_output`).
- Inbound SSE chunks (`response.output_item.added`, `response.function_call_arguments.delta`, `response.completed`, `[DONE]`) are streamed with zero drop or corruption.
- All outbound requests satisfy protocol invariants under `validate_protocol_invariants(&json, ProviderKind::OpenAI)`.

### On-Disk Rollout Transcript Integrity
- Codex records session history to `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-*.jsonl`.
- ContextWatcher acts purely as an in-flight wire proxy: it applies overlays in memory to outbound model payloads without altering on-disk files.
- In-depth verification parsed every rollout file generated across turns:
  1. Every non-empty line parsed as valid JSON.
  2. The string `"CW112_POLICY_ACTIVE"` was **completely absent** from all rollout files on disk, proving on-disk transcripts are free of injected proxy overlays.

### Session Identity Disambiguation
- **ContextWatcher Session ID (`CW_SESSION_ID`):** Manages ContextWatcher database records, token accounting, and active mutation overlays.
- **Codex Native Session ID (`thread_id`):** Manages Codex CLI local conversation state and on-disk rollout resume.
- ContextWatcher successfully links multiple native resume turns under a unified ContextWatcher session ID while preserving Codex's native thread UUID.

---

## 4. Failure & Recovery Evidence

The test suite in `crates/cw-proxy/tests/qualification.rs` includes dedicated failure and fault recovery characterization:

1. **Unknown Native Session Resume (`test_harness_binary_codex_cli_unknown_native_session_id_resume`):**
   Resuming with an invalid UUID (`00000000-0000-0000-0000-000000000000`) causes Codex to exit with a non-zero code. ContextWatcher proxy logs cleanly, records no corrupt turns, and remains fully operational.
2. **Unauthorized Token Rejection (`test_harness_binary_codex_cli_unauthorized_token_rejection`):**
   Requests carrying an invalid `CW_CONTROL_TOKEN` on session-bound proxy routes are rejected by the proxy with `403 Forbidden` (`Session-bound requests require the local control credential`). Codex fails immediately without contacting upstream.
3. **Upstream 502 Bad Gateway Handling (`test_harness_binary_codex_cli_upstream_failure_handling`):**
   When the upstream provider returns `502 Bad Gateway`, Codex retries with exponential backoff and exits non-zero. The ContextWatcher database remains uncorrupted and accessible.
4. **Session Isolation (`test_harness_binary_codex_cli_session_isolation`):**
   Session A has a committed secret overlay (`CW112_SESSION_A_SECRET_OVERLAY`). Session B executes independently. Outbound inspection verifies that Session B's requests never receive Session A's overlay, while Session A's subsequent turn correctly receives it.

---

## 5. Live Real-Provider Qualification Status

> [!WARNING]
> **LIVE QUALIFICATION BLOCKED**
>
> In accordance with repository safety guidelines, **no live LLM provider charges may be incurred without explicit opt-in and authorized test budget**.
>
> Although `OPENAI_API_KEY` is present in the local environment, `CW_QUALIFY_REAL_CODEX` is currently unset and no live spend ceiling has been authorized for this task.
>
> Invoking `test_live_continuation_real_codex_multiturn` without `CW_QUALIFY_REAL_CODEX=1` explicitly panics with an actionable error rather than silently returning a passing result.

### How to Run Opt-in Live Qualification
Once spend authorization and budget are confirmed:
```bash
CW_QUALIFY_REAL_CODEX=1 OPENAI_API_KEY="sk-..." cargo test -p cw-proxy --test qualification test_live_continuation_real_codex_multiturn -- --ignored --nocapture
```
This test runs a real 3-turn native session resume (`codex exec` followed by two `codex exec resume <thread_id>` invocations) with `gpt-4o-mini` through the live ContextWatcher proxy:
- Runs in an isolated workspace with `-s workspace-write --approve-for-me` and `-c ask_for_approval="never"`.
- Uses `store.get_turn_mutated_payload` to verify that the committed overlay reaches the actual outbound provider request on Turns 2 and 3.

---

## 6. Test Suite & CI Reconciliation

Reconciliation against CI run 41105119741 and local test runs:

| Test Target | Passed | Failed | Ignored | Notes |
|---|:---:|:---:|:---:|---|
| **Workspace Default Suite** (`cargo test --workspace`) | **100** | **0** | **15** | CI baseline (10 ignored in qualification + 4 in live_provider + 1 other) |
| **Hermes Python Suite** (`test_plugin.py`) | **12** | **0** | **0** | 100% pass |
| **Harness Binary Suite** (`cargo test ... harness_binary -- --ignored`) | **8** | **0** | **13** | 6 Codex tests + 1 Hermes test + 1 OMP test |
| **Opt-in Live Test** (`test_live_continuation_real_codex_multiturn`) | — | — | — | Fails explicitly if invoked without `CW_QUALIFY_REAL_CODEX=1` |

**Milestone Status:** Phase 1 mock qualification complete. CW-112 remains **Blocked / In Progress** pending Phase 2 live spend authorization.
