# CW-116A Baseline Evidence & Test Inventory

**Milestone:** CW-116A (Characterization Tests & Architectural Contracts)  
**Branch:** `feat/cw-116a-characterization-contracts`  
**Starting Commit SHA:** `f87da23b8f2c382f7c0a6b7d29486c9d5ad50fe9` (Merge PR #12: CW-116 maintainability roadmap on `main`)  
**Timestamp:** 2026-10-30T14:18:00+01:00  

---

## 1. Executive Summary

This document establishes the verified, reproducible characterization baseline for the ContextWatcher (`cw`) codebase prior to beginning the CW-116B (CLI/Harness Modularization) and CW-116C (Proxy/Transformation Boundaries) maintainability refactoring.

In accordance with the **CW-116A Behavior Freeze**:
- No production redesign, speculative abstractions, or optimizer changes have been introduced.
- Existing behavioral contracts across CLI wrapping, session identity, credential stripping, proxy routing, mutation validation, and persistence safety have been characterized through deterministic automated tests.
- One minor production extraction was made to enable regression testing: [`CwStore::get_wire_request`](file:///home/gionag/Development/contextWatcher/crates/cw-store/src/lib.rs#L478-L493) was exposed as a read-only query for previously saved wire request bytes.

---

## 2. Baseline Verification Commands & Results

| Check / Suite | Command | Result | Notes |
|---|---|:---:|---|
| **Rust Formatting** | `cargo fmt --all -- --check` | 🟢 **PASS** | 0 formatting diffs across workspace |
| **Rust Linter** | `cargo clippy --workspace --all-targets -- -D warnings` | 🟢 **PASS** | 0 warnings / 0 errors |
| **Workspace Tests (CI Suite)** | `cargo test --workspace` | 🟢 **PASS** | 94 passed, 0 failed, 6 ignored (qualification Suite B & C) |
| **Python Hermes Plugin Tests** | `python3 plugins/hermes/context-watcher/test_plugin.py` | 🟢 **PASS** | 12 passed in 0.032s (including 20-worker barrier concurrency stress test) |
| **Installed Binary Qualification** | `cargo test -p cw-proxy --test qualification harness_binary -- --ignored` | 🟢 **PASS** | 3 passed in 13.57s (Codex CLI, OMP, Hermes Agent) against isolated local mocks |
| **Live Cloud Provider Suite** | `cargo test -p cw-proxy --test live_provider -- --ignored` | ⚪ **SKIPPED** | Opt-in suite requiring live cloud credentials and incurring inference cost |

---

## 3. Comprehensive Test Inventory

Every test in the repository is explicitly categorized into one of three tiers. Skipped or ignored tests are never conflated with passing qualification evidence.

### 3.1 Suite A: Default Workspace Tests (Run by CI without external dependencies)

Total: **94 passing tests**.

#### Crate: `cw-cli` (14 tests in [`main.rs`](file:///home/gionag/Development/contextWatcher/crates/cw-cli/src/main.rs))
1. `update::tests::release_target_matches_supported_release_matrix` — Validates cross-platform release target triple mapping.
2. `update::tests::verifies_release_checksum_and_rejects_mismatch` — Verifies SHA-256 verification and tampering rejection.
3. `update::tests::atomically_replaces_existing_binary` — Tests atomic file replacement with temp files.
4. `update::tests::extracts_only_the_top_level_regular_cw_file` — Validates archive extraction sandbox safety.
5. `wrap_tests::codex_wrapper_uses_proxy_provider_and_environment_backed_headers` — Validates Codex CLI provider config and shell environment filter (`exclude`).
6. `wrap_tests::wrapper_removes_cw_credentials_from_harness_environment` — Tests credential purging from environment.
7. `wrap_tests::regression_test_override_file_cleanup_on_normal_exit_without_consumption` — Confirms RAII guard removes unconsumed override file on child normal exit.
8. `wrap_tests::regression_test_override_file_cleanup_on_write_failure` — Confirms RAII guard removes override file if write_all fails immediately after open.
9. `wrap_tests::parse_json_string_array_handles_multiline_and_rejects_malformed` — Tests JSON array parsing across Bitwarden Secrets Manager banner prefixes.
10. `wrap_tests::init_dry_run_leaves_filesystem_and_systemd_untouched` *(Added in CW-116A)* — Characterizes `cw init --dry-run` performing 0 filesystem/service side effects.
11. `wrap_tests::header_stripping_preserves_complex_non_cw_headers` *(Added in CW-116A)* — Tests `without_cw_headers` preserving non-CW headers while stripping `x-cw-*`.
12. `wrap_tests::comma_separated_header_stripping_edge_cases` *(Added in CW-116A)* — Tests `without_cw_comma_separated_headers` handling colon-separated values and commas.
13. `wrap_tests::session_id_validation_contract` *(Added in CW-116A)* — Asserts strict character bounds on session IDs (1..=128, ASCII alphanumeric, `_`, `-`, rejecting traversal and null bytes).
14. `wrap_tests::project_label_from_workspace_contract` *(Added in CW-116A)* — Asserts workspace name extraction falling back to `"General Chat"`.

#### Crate: `cw-core` (38 tests)
- **Unit tests (24 tests):**
  - Attention salience U-curve & HUD summary formatting (`hud.rs`).
  - Block map, block boundary alignment, deterministic slot assignment, locked prefix protection, physical page unmap simulation, tail truncation (`slot_bridge.rs`).
  - Ambient pulse generation, Gemini tool gating/sorting, local LLM KV optimization (`lib.rs`).
  - Anthropic consecutive roles, orphan tool result detection, thinking order validation, valid payload (`validator.rs`).
  - OpenAI orphan tool call ID detection (`validator.rs`).
  - Pin injection into Responses input messages, unsupported tail reporting (`mutation.rs`).
  - Pin context & tool gating, KV cache prefix diff & invalidation cause (`lib.rs`).
  - Swarm prefix grouping & savings calculation (`swarm.rs`).
  - Stale & duplicate file linter, shadow deduplication (`linter.rs`).
  - OpenCode pin growth regression, unread tool output regression (`lib.rs`).
  - Anthropic normalization & system deconstruction (`normalizer.rs`).
- **Fixture tests (3 tests in `fixtures_test.rs`):**
  - Recorded Anthropic fixture normalization.
  - Recorded OpenAI fixture & failed loop folding.
  - Recorded Gemini fixture preserving protected reads.
- **Safety tests (11 tests in `safety_test.rs`):**
  - Metadata stripping preserves application keys.
  - Signed requests and no-rule requests remain opaque.
  - Responses inputs and call metadata recognized.
  - Changed/failed read cannot replace verified contents.
  - Consecutive requests telemetry & suffix invalidation matrix.
  - User and system prompts are never pruned or stubbed by pointer.
  - Covered read deduplication requires a retained covering range.
  - Unknown cache is not a hit; minimum plan preserves history.
  - Pins with quotes and backslashes are idempotent.
  - Selectors reject stale content and system fragments.
  - Native tool text arrays targeted without dropping metadata.

#### Crate: `cw-harnesses` (5 tests)
1. `test_import_omp_transcript` — Imports native OMP JSONL session transcript.
2. `test_import_agy_transcript_fixture` — Imports Antigravity JSONL session transcript.
3. `test_import_opencode_sqlite_db` — Imports OpenCode SQLite storage snapshot.
4. `test_sync_mutations_to_transcript_on_disk` — Validates atomic file write (`.tmp` + rename) and `.bak` creation.
5. `test_auto_heal_low_health_transcript` — Validates background auto-healing transcript trigger.

#### Crate: `cw-mcp` (2 tests)
1. `test_mcp_unknown_method_error_code` — Asserts JSON-RPC 2.0 error code `-32601`.
2. `test_mcp_unknown_tool_diagnostic_error` — Asserts structured diagnostic error on unknown tool invocation.

#### Crate: `cw-proxy` (26 tests)
- **Characterization Contracts (6 tests in [`characterization_contracts.rs`](file:///home/gionag/Development/contextWatcher/crates/cw-proxy/tests/characterization_contracts.rs), *Added in CW-116A*):**
  1. `contract_transform_context_api_auth_validation_and_fail_open` — Exercises `/api/v1/sessions/:id/transform` across 401 unauth, 401 bad token, 400 bad request, 200 clean pass-through, 200 pin injection with custom metadata preservation, and 200 `fallback_raw` on invariant violation.
  2. `contract_proxy_routing_and_fallback_provider_selection` — Characterizes route prefixing (`/deepseek`, `/openai/v1`, `/anthropic`, `/gemini`) and unprefixed fallback matching (`/v1/messages`, `generateContent`, `/v1/chat/completions`).
  3. `contract_proxy_strips_internal_cw_headers_before_forwarding` — Asserts that `x-cw-session-id`, `x-cw-control-token`, `x-cw-workspace` are completely stripped from upstream requests while client headers (`authorization`, custom trace headers) are preserved.
  4. `contract_proxy_dry_run_preview_never_calls_upstream` — Characterizes `x-cw-dry-run: 1` returning preview JSON (`cw_dry_run: true`, outcome `"preview"`) without reaching the upstream network provider.
  5. `contract_unauthenticated_request_generates_synthetic_session` — Proves unauthenticated observation requests pass raw bytes through to upstream while generating an audit record under a synthetic `sess_<uuid>` session ID.
  6. `contract_sse_streaming_chunk_accumulator_and_cached_tokens` — Characterizes `SseChunkAccumulator` extracting prompt and cached token telemetry from fragmented SSE stream packets.
- **Safety Suite (10 tests in [`safety.rs`](file:///home/gionag/Development/contextWatcher/crates/cw-proxy/tests/safety.rs)):**
  - Malformed preview never calls upstream.
  - Signed Anthropic SSE response forwarded unchanged.
  - Control requires auth; override never receives engine key.
  - Approval timeout blocks signed requests without enabling mutation.
  - DeepSeek route uses upstream and preserves chat completions endpoint.
  - Signed Anthropic payload forwarded unchanged with committed overlay.
  - Concurrent requests keep both records and SSE final usage.
  - Preview does not rewrite actual turn; stale plan rejected.
  - Observer forwards original bytes and keeps unrelated sessions separate.
  - Authenticated session headers apply overlay and are stripped before upstream.
- **Qualification Core Contracts (10 tests in [`qualification.rs`](file:///home/gionag/Development/contextWatcher/crates/cw-proxy/tests/qualification.rs)):**
  - AC1 Session Identity & Binding (403 unauth rejection, authenticated multi-turn binding).
  - AC2 Context Transform API & Committed Overlay Reapplication.
  - AC3 Zero duplicate mutation injection across consecutive turns.
  - AC4 Tool-call / result pairing strictly preserved under active mutation.
  - AC5 Signed request byte-for-byte and JSON-tree fidelity.
  - AC6 Control token stripped before upstream and config sandboxing assertions.
  - AC7 Original transcripts untouched bit-for-bit (BLAKE3 hash identity).
  - AC8 Safe fault recovery and Pre-Forward Validation Gate fail-open.
  - AC10 Token accounting and telemetry reconciliation.

#### Crate: `cw-store` (9 tests)
- **Unit tests (2 tests):** SQLite roundtrip and autopilot/effectiveness records.
- **Safety tests (7 tests in `safety.rs`):**
  - `wire_requests_are_immutable_and_blob_storage_is_content_addressed` *(Added in CW-116A)* — Characterizes BLAKE3 content-addressed blob deduplication and immutable turn wire request storage.
  - Revision-checked changes atomic and session-scoped.
  - Revision tracks new snapshots with same sequence.
  - Derived updates preserve originals; missing stays missing.
  - Draft does not replace committed rule; sequences unique.
  - Migration backup is private; purge preserves changed exports.
  - List sessions keeps legacy callers complete and pageable.

---

### 3.2 Suite B: Installed Harness Binary Integration Tests (Ignored by Default, Requires Local Harnesses)

Total: **3 tests** (Executed explicitly via `cargo test -p cw-proxy --test qualification harness_binary -- --ignored`).

| Test Name | Target Binary | Test Strategy | Status |
|---|---|---|:---:|
| `test_harness_binary_codex_cli_multiturn_overlay_reapplication` | `codex` (v0.165.0) | Multi-turn `codex exec` through custom Responses provider; asserts Turn 2 injects committed `PinContext` policy; status 0 | 🟢 **PASS** |
| `test_harness_binary_omp_native_runtime_overlay_reapplication` | `omp` (v18.12.6) | 3-turn native resume (`omp -r <id>`); native `context` hook queries `/transform`; Turn 2 injects pin; source transcript untouched; status 0 | 🟢 **PASS** |
| `test_harness_binary_hermes_middleware_session_binding` | `hermes` (v0.22.5) | 7-gate test: concurrent launches Process A & B; distinct UUID override files; native resume; fail-safe invalid handoff; config restoration guard; status 0 | 🟢 **PASS** |

*Note: All three tests run against local loopback HTTP mock providers and perform zero external cloud API requests.*

---

### 3.3 Suite C: Opt-In Live Cloud Provider Tests (Ignored, Requires API Keys)

Total: **7 tests**.

| Test File | Test Name | Required Variable | Target Model | Status |
|---|---|---|---|:---:|
| `cw-proxy/tests/qualification.rs` | `test_live_continuation_real_codex_multiturn` | `CW_QUALIFY_REAL_CODEX=1`, `OPENAI_API_KEY` | `gpt-4o` | ⚪ **SKIPPED (Opt-in)** |
| `cw-proxy/tests/qualification.rs` | `test_live_continuation_real_omp` | `CW_QUALIFY_REAL_OMP=1`, `OPENAI_API_KEY` | `gpt-4o-mini` | ⚪ **SKIPPED (Opt-in)** |
| `cw-proxy/tests/qualification.rs` | `test_live_continuation_real_hermes_multiturn` | `CW_QUALIFY_REAL_HERMES=1`, `DEEPSEEK_API_KEY` | `deepseek-chat` | ⚪ **SKIPPED (Opt-in)** |
| `cw-proxy/tests/live_provider.rs` | `real_openrouter_gemini_reasoning_details_survive_proxy_forwarding` | `OPENROUTER_API_KEY` | `google/gemini-3.8-flash` | ⚪ **SKIPPED (Opt-in)** |
| `cw-proxy/tests/live_provider.rs` | `real_openrouter_anthropic_signature_event_survives_proxy_streaming` | `OPENROUTER_API_KEY` | `anthropic/claude-haiku-5.5` | ⚪ **SKIPPED (Opt-in)** |
| `cw-proxy/tests/live_provider.rs` | `real_anthropic_signature_event_survives_proxy_streaming` | `ANTHROPIC_API_KEY`, `CW_R7_ANTHROPIC_MODEL` | `claude-3-5-sonnet-20241022` | ⚪ **SKIPPED (Opt-in)** |
| `cw-proxy/tests/live_provider.rs` | `real_gemini_function_signature_survives_proxy_forwarding` | `GEMINI_API_KEY`, `CW_R7_GEMINI_MODEL` | `gemini-1.5-pro` | ⚪ **SKIPPED (Opt-in)** |

---

## 4. Codebase Line-Count & Hotspot Inventory

```
      57  crates/cw-core/src/repository.rs
      81  crates/cw-core/src/control.rs
     110  crates/cw-core/src/tokenizer.rs
     255  crates/cw-core/src/hud.rs
     285  crates/cw-core/src/validator.rs
     309  crates/cw-core/src/autopilot.rs
     356  crates/cw-core/src/slot_bridge.rs
     367  crates/cw-cli/src/update.rs
     375  crates/cw-core/src/swarm.rs
     416  crates/cw-core/src/radix.rs
     422  crates/cw-core/src/local_llm.rs
     512  crates/cw-tui/src/lib.rs
     585  crates/cw-proxy/src/forwarding.rs
     599  crates/cw-core/src/linter.rs
     601  crates/cw-core/src/optimizer.rs
     652  crates/cw-mcp/src/lib.rs
     657  crates/cw-core/src/ir.rs
     667  crates/cw-core/src/lib.rs
     990  crates/cw-core/src/mutation.rs
   1,100  crates/cw-core/src/normalizer.rs
   1,407  crates/cw-store/src/lib.rs
   1,974  crates/cw-proxy/src/lib.rs
   2,385  crates/cw-harnesses/src/lib.rs
   3,216  crates/cw-cli/src/main.rs
  18,376  total Rust source lines
```

### Major Complexity Hotspots
1. **`cw-cli/src/main.rs` (3,216 lines):**
   - Monolithic `main()` function containing an over 1,000-line `match cli.command` block.
   - Harness wrapping logic (`Commands::Wrap`), argument manipulation, environment construction, UUID override handoff file creation, and cleanup guards mixed directly with terminal reporting, daemon management, and update commands.
   - Target for **CW-116B**.
2. **`cw-harnesses/src/lib.rs` (2,385 lines):**
   - Houses transcript parsers for four distinct formats (Antigravity, Codex, OMP/Pi, Claude), SQLite imports (Hermes, OpenCode), disk sync-back, background file watching, auto-healing heuristics, and qualification config generators.
   - Target for **CW-116B**.
3. **`cw-proxy/src/lib.rs` (1,974 lines) & `forwarding.rs` (585 lines):**
   - Combines HTTP reverse proxying, REST API handlers, WebSocket event fanout, firewall intercept resolution, context transformation, pre-forward validation gating, and SSE stream parsing in single files.
   - Target for **CW-116C**.
