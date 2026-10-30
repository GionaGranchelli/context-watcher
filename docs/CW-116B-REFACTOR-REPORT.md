# CW-116B Refactoring Report: CLI & Harness Lifecycle Modularization

- **Baseline Commit:** `e0c7b5b69846051b2f657be153d170d00f452754` on `main` (post-CW-116A merge, PR #13)
- **Branch:** `feat/cw-116b-cli-harness-modularization`
- **Milestone:** CW-116 (Maintainability Gate)
- **Scope:** Decompose monolithic `crates/cw-cli/src/main.rs` and `crates/cw-harnesses/src/lib.rs` while preserving exact observable behavior, public CLI flags, process lifecycle, environment scrubbing invariants, and API contracts.

---

## 1. Executive Summary

Prior to CW-116B, ContextWatcher concentrated its CLI command dispatch, process management, report formatting, and multi-harness transcript lifecycles inside two massive monolithic files:
- `crates/cw-cli/src/main.rs`: 3,210 lines
- `crates/cw-harnesses/src/lib.rs`: 2,385 lines

These files exhibited high cognitive load, mixed domain responsibilities (CLI argument parsing, filesystem traversal, SQLite queries, process spawning, terminal ANSI formatting, and mutation execution), and tight coupling.

Under CW-116B, both files were decomposed into cohesive, single-responsibility modules:
1. `crates/cw-cli`: `main.rs` is reduced from 3,210 to 618 lines (-80.7%), functioning purely as Clap CLI struct definitions and a thin dispatch switch. All operational commands have been moved to `commands/` (`daemon.rs`, `init.rs`, `reports.rs`, `session.rs`, `slot.rs`, `web.rs`, `wrap.rs`).
2. `crates/cw-harnesses`: `lib.rs` is reduced from 2,385 to 277 lines (-88.4%), functioning purely as clean re-exports and integration tests. Lifecycles, error definitions, and adapters have been extracted into `error.rs`, `qualification.rs`, `sync.rs`, `auto_heal.rs`, `watcher.rs`, and `adapters/` (`agy.rs`, `generic.rs`, `hermes.rs`, `opencode.rs`, `common.rs`).

Zero behavioral or API breaking changes were introduced. All 100 workspace unit/integration tests, 12 Hermes Python tests, and 3 ignored harness binary integration tests continue to pass identically.

---

## 2. Before-and-After Responsibility Map

### `crates/cw-cli`

| File | Before Responsibilities | After Responsibilities |
|---|---|---|
| `src/main.rs` | Clap CLI definition, command dispatch, daemon process spawning/killing, static Web UI serving, session status, Radix tree printing, linter fix execution, optimization execution, transcript import, SQLite queries, HUD reporting, Swarm analysis, Slot bridge inspection, Init installer, Hook checks, header scrubbing, and process wrapping. | Pure entry point: Clap argument structs (`Cli`, `Commands`, `DaemonAction`), default DB path resolver, and thin async dispatch to dedicated `commands::*` modules. |
| `src/commands/wrap.rs` | Mixed inside `main.rs`. | Subprocess proxy wrapping (`cw wrap`), session ID validation, CW header stripping, credential scrubbing (`CW_CONTROL_TOKEN`, `CW_CONTROL_TOKEN_FILE`), RAII temp override file cleanup. |
| `src/commands/daemon.rs` | Mixed inside `main.rs`. | Daemon process health checking (`is_daemon_running`), detached child process spawning (`start_daemon_detached`), PID management, and process termination (`stop_daemon`, `restart_daemon`, `status_daemon`). |
| `src/commands/init.rs` | Mixed inside `main.rs`. | Global CLI binary installation (`~/.local/bin/cw`), harness integration installation for Antigravity, Codex, Claude Code, Hermes, Pi, and OpenCode, systemd user service setup, and safe non-destructive config merges. |
| `src/commands/session.rs` | Mixed inside `main.rs`. | Interactive session operations (`status`, `tree`, `lint`, `optimize`, `fold`, `prune`, `dedupe`, `gate`, `pin`, `import`, `sync-back`, `local`, `autopilot`, `pulse`, `hook`, `purge`, `preview`), session resolution (`resolve_session`), and mutation staging (`save_cli_mutation`). |
| `src/commands/reports.rs` | Mixed inside `main.rs`. | Analytical reporting: Gain scoreboard (`run_gain_report`), Multi-Agent Swarm prefix sharing (`run_swarm_report`), Attention ECG / Context HUD (`run_hud_report`), and workspace project label derivation (`project_label_from_workspace`). |
| `src/commands/slot.rs` | Mixed inside `main.rs`. | Engine-level PagedAttention inspection and KV cache slot splicing report (`run_slot_bridge_report`). |
| `src/commands/web.rs` | Mixed inside `main.rs`. | Embedded static asset handler (`WebAssets`) and HTTP asset server fallback (`serve_embedded_web_ui`). |
| `tests/init_characterization.rs` | Mixed inside `main.rs` test module. | Subprocess dry-run characterization asserting that `cw init --dry-run` leaves the host filesystem, directories, and systemd completely untouched. |

### `crates/cw-harnesses`

| File | Before Responsibilities | After Responsibilities |
|---|---|---|
| `src/lib.rs` | All harness imports (AGY, Codex, Claude, Pi, OMP, Hermes, OpenCode), background transcript filesystem watching, auto-heal loop, mutation disk syncing, and proxy qualification helpers in one file. | Re-export facade maintaining 100% backwards compatibility for callers, plus regression and qualification unit tests. |
| `src/error.rs` | Mixed in `lib.rs`. | Strongly typed `HarnessError` enumeration (`thiserror`). |
| `src/qualification.rs` | Mixed in `lib.rs`. | Proxy configuration generators (`codex_proxy_config_args`, `omp_proxy_models_config`). |
| `src/sync.rs` | Mixed in `lib.rs`. | Compaction export engine (`sync_mutations_to_transcript_on_disk`) writing private `.cw-export-<uuid>.jsonl` copies while guaranteeing original transcripts remain bit-for-bit unchanged. |
| `src/auto_heal.rs` | Mixed in `lib.rs`. | Autopilot low-health evaluation and background remediation loop (`check_and_apply_auto_heal`). |
| `src/watcher.rs` | Mixed in `lib.rs`. | Bounded snapshot file scanner (`spawn_transcript_watcher`, `scan_and_import_all_transcripts_once`, `scan_jsonl_dir_recursively`). |
| `src/adapters/agy.rs` | Mixed in `lib.rs`. | Antigravity transcript parser (`import_agy_transcript`), hydration from `transcript_full.jsonl`, user request extraction, and model settings parser. |
| `src/adapters/generic.rs` | Mixed in `lib.rs`. | Codex rollout, Claude Code, Pi, and OMP JSONL stream importer (`import_generic_jsonl_transcript`) and text extraction (`extract_codex_content_text`). |
| `src/adapters/hermes.rs` | Mixed in `lib.rs`. | Hermes Agent SQLite state importer (`import_hermes_state_db`). |
| `src/adapters/opencode.rs` | Mixed in `lib.rs`. | OpenCode V2 SQLite database importer (`import_opencode_state_db`). |
| `src/adapters/common.rs` | Mixed in `lib.rs`. | Reusable workspace path inference from tool arguments and text (`infer_workspace_from_tool_args`, `infer_workspace_from_text`, `extract_project_root_from_path_str`). |

---

## 3. Line Count and Complexity Reduction Table

| Component / File | Original Lines | Refactored Lines | Net Change | Relative Reduction |
|---|---|---|---|---|
| `cw-cli/src/main.rs` | 3,210 | 618 | -2,592 | **-80.7%** |
| `cw-cli/src/commands/wrap.rs` | — | 362 | +362 | *New Module* |
| `cw-cli/src/commands/daemon.rs` | — | 152 | +152 | *New Module* |
| `cw-cli/src/commands/init.rs` | — | 598 | +598 | *New Module* |
| `cw-cli/src/commands/reports.rs` | — | 479 | +479 | *New Module* |
| `cw-cli/src/commands/session.rs` | — | 935 | +935 | *New Module* |
| `cw-cli/src/commands/slot.rs` | — | 282 | +282 | *New Module* |
| `cw-cli/src/commands/web.rs` | — | 27 | +27 | *New Module* |
| `cw-cli/src/commands/mod.rs` | — | 7 | +7 | *New Module* |
| `cw-cli/tests/init_characterization.rs` | — | 74 | +74 | *New Integration Test* |
| **`cw-cli` Subtotal** | **3,210** | **3,534** | **+324** | *Decomposed into 9 files* |
| `cw-harnesses/src/lib.rs` | 2,385 | 277 | -2,108 | **-88.4%** |
| `cw-harnesses/src/adapters/agy.rs` | — | 362 | +362 | *New Module* |
| `cw-harnesses/src/adapters/generic.rs` | — | 561 | +561 | *New Module* |
| `cw-harnesses/src/adapters/hermes.rs` | — | 341 | +341 | *New Module* |
| `cw-harnesses/src/adapters/opencode.rs` | — | 356 | +356 | *New Module* |
| `cw-harnesses/src/adapters/common.rs` | — | 68 | +68 | *New Module* |
| `cw-harnesses/src/adapters/mod.rs` | — | 10 | +10 | *New Module* |
| `cw-harnesses/src/auto_heal.rs` | — | 115 | +115 | *New Module* |
| `cw-harnesses/src/error.rs` | — | 13 | +13 | *New Module* |
| `cw-harnesses/src/qualification.rs` | — | 37 | +37 | *New Module* |
| `cw-harnesses/src/sync.rs` | — | 171 | +171 | *New Module* |
| `cw-harnesses/src/watcher.rs` | — | 170 | +170 | *New Module* |
| **`cw-harnesses` Subtotal** | **2,385** | **2,481** | **+96** | *Decomposed into 12 files* |
| **Total Monolithic Lines** | **5,595** | **895** | **-4,700** | **-84.0% root file reduction** |

---

## 4. Dependency Graphs and Module Direction

### CLI Module Architecture

```
                    ┌─────────────────────────┐
                    │      cw-cli::main       │
                    └────────────┬────────────┘
                                 │
     ┌──────────┬──────────┬─────┴────┬──────────┬──────────┬──────────┐
     ▼          ▼          ▼          ▼          ▼          ▼          ▼
 commands:: commands:: commands:: commands:: commands:: commands:: commands::
   daemon      wrap       init     session    reports      slot       web
     │                     │          ▲          ▲          ▲
     │                     │          │          │          │
     │                     └──────────┼──────────┴──────────┘
     │                                │ (project_label_from_workspace,
     └────────────────────────────────┘  resolve_session)
       (is_daemon_running, start_daemon_detached)
```

- **Top-Down Control Flow:** `main.rs` parses CLI arguments into `Cli` / `Commands` and routes execution into individual command modules.
- **Acyclic Seams:** Reusable session resolution (`resolve_session`) and mutation persistence (`save_cli_mutation`) are co-located in `commands::session`. `commands::reports` exports `project_label_from_workspace`, cleanly consumed by both `session` (for batch project filtering) and `reports` (for analytics), eliminating circular dependencies.

### Harness Lifecycle Architecture

```
                          ┌───────────────────────────┐
                          │    cw-harnesses::lib      │ (Facade re-exports)
                          └─────────────┬─────────────┘
                                        │
           ┌────────────┬───────────────┼───────────────┬────────────┐
           ▼            ▼               ▼               ▼            ▼
        error     qualification       sync          watcher      auto_heal
                                        │               │            ▲
                                        │               ▼            │
                                        │          adapters::*       │
                                        │         (agy, generic,     │
                                        │          hermes, opencode) ─┘
                                        │               │
                                        └───────────────┘
```

- **Clean Adapter Isolation:** Harness adapters focus exclusively on deserializing harness-specific wire or SQLite representations into ContextWatcher's canonical `ContextTurn` / `ContextNode` IR.
- **Independent Execution Layers:** File watching (`watcher.rs`) and disk syncing (`sync.rs`) depend on adapters and the store, but adapters have no dependency on the watcher.

---

## 5. Seam Analysis: Operational Contracts

1. **Session Resolution Seam (`resolve_session`):**
   - Invariant: Explicit session flags take priority, followed by the `CW_SESSION_ID` environment variable, then workspace path match against current working directory, and finally fallback to the most recent active session.
   - Preserved contract: Prefix matching behavior (resolving short 8-character hashes if unique, erroring if ambiguous) is completely preserved.
2. **Mutation Storage Seam (`save_cli_mutation`):**
   - Interactive CLI commands (`fold`, `prune`, `dedupe`, `gate`, `pin`) create a typed `MutationRule` with unique UUID and persist it via `store.save_mutation`.
3. **Hermes Configuration Seam (`merge_hermes_client_env_passthrough`):**
   - Safe merge contract established in CW-113 and characterized in CW-116A is maintained: existing environment variables are preserved, missing required variables are appended, and failure to read returns an error without clobbering existing configuration.
4. **Credential Isolation Seam (`run_wrap`):**
   - For all non-Codex wrappers, `CW_CONTROL_TOKEN` and `CW_CONTROL_TOKEN_FILE` are removed from the child process environment to enforce harness sandboxing.

---

## 6. Coupling Reduction & Interface Isolation

- **Axum Web Server Independence:** Axum HTTP router construction for `cw serve` remains in `main.rs` while the static file asset handler is encapsulated in `commands::web`.
- **Systemd & Subprocess Isolation:** Detached daemon management (`systemctl` fallback to fork/exec with PID tracking) is encapsulated in `commands::daemon`.
- **SQL Parsing Separation:** Direct SQLite database querying for OpenCode and Hermes is restricted to their respective adapter modules (`adapters::hermes`, `adapters::opencode`), completely removed from the root harness library.

---

## 7. API & CLI Backwards Compatibility Verification

All public entry points, binary argument signatures, environment variables, and exit codes remain strictly identical:
- Public binary: `cw [serve|daemon|wrap|tui|status|tree|lint|optimize|fold|prune|dedupe|gate|pin|import|sync-back|local|autopilot|gain|pulse|swarm|hud|slot|init|hook|update|auth-token|purge|preview]`
- Crate exports:
  - `cw_harnesses::import_agy_transcript`
  - `cw_harnesses::import_generic_jsonl_transcript`
  - `cw_harnesses::import_hermes_state_db`
  - `cw_harnesses::import_opencode_state_db`
  - `cw_harnesses::sync_mutations_to_transcript_on_disk`
  - `cw_harnesses::spawn_transcript_watcher`
  - `cw_harnesses::scan_and_import_all_transcripts_once`
  - `cw_harnesses::check_and_apply_auto_heal`
  - `cw_harnesses::qualification::codex_proxy_config_args`
  - `cw_harnesses::qualification::omp_proxy_models_config`
  - `cw_harnesses::TranscriptSyncReport`
  - `cw_harnesses::HarnessError`

---

## 8. Qualification Test Evidence

### Workspace Unit & Integration Tests (`cargo test --workspace`)

```
     Running unittests src/main.rs (target/debug/deps/cw-34ca8eab73e98af3)
test commands::reports::tests::project_label_from_workspace_contract ... ok
test commands::init::tests::parse_json_string_array_handles_multiline_and_rejects_malformed ... ok
test commands::wrap::tests::codex_wrapper_uses_proxy_provider_and_environment_backed_headers ... ok
test commands::wrap::tests::comma_separated_header_stripping_edge_cases ... ok
test commands::wrap::tests::header_stripping_preserves_complex_non_cw_headers ... ok
test commands::wrap::tests::session_id_validation_contract ... ok
test commands::wrap::tests::wrapper_removes_cw_credentials_from_harness_environment ... ok
test commands::wrap::tests::regression_test_override_file_cleanup_on_normal_exit_without_consumption ... ok
test update::tests::release_target_matches_supported_release_matrix ... ok
test commands::wrap::tests::regression_test_override_file_cleanup_on_write_failure ... ok
test update::tests::verifies_release_checksum_and_rejects_mismatch ... ok
test update::tests::atomically_replaces_existing_binary ... ok
test update::tests::extracts_only_the_top_level_regular_cw_file ... ok
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

     Running tests/init_characterization.rs (target/debug/deps/init_characterization-a2fa1e8272c3f0a3)
test init_dry_run_leaves_filesystem_and_systemd_untouched ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

     Running unittests src/lib.rs (target/debug/deps/cw_harnesses-ed23c03fb45190d5)
test tests::test_import_omp_transcript ... ok
test tests::test_import_opencode_sqlite_db ... ok
test tests::test_import_agy_transcript_fixture ... ok
test tests::test_sync_mutations_to_transcript_on_disk ... ok
test tests::test_auto_heal_low_health_transcript ... ok
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

Total Workspace Test Summary:
100 passed; 0 failed; 10 ignored
```

### Hermes Plugin Concurrency & Isolation Tests (`test_plugin.py`)

```
Ran 12 tests in 0.033s
OK
```

### Ignored Harness Binary Qualification Tests (`qualification.rs`)

```
cargo test -p cw-proxy --test qualification harness_binary -- --ignored --nocapture

running 3 tests
test test_harness_binary_codex_cli_multiturn_overlay_reapplication ... ok
test test_harness_binary_omp_native_runtime_overlay_reapplication ... ok
test test_harness_binary_hermes_middleware_session_binding ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 13 filtered out; finished in 13.79s
```

---

## 9. Risk Analysis & Mitigations

| Risk | Mitigation | Verification |
|---|---|---|
| Inadvertent CLI argument parsing regression | Left Clap enum definitions and derive attributes strictly intact in `main.rs`. | `cargo test -p cw-cli` and manual `--help` parse verification. |
| Relative path failures in `include_str!` macros | Updated paths from `../../../plugins/...` to `../../../../plugins/...` to account for `commands/` directory nesting. | Verified compilation and byte inclusion in `init.rs`. |
| Loss of destructor execution in wrapper | Retained the `OverrideFileCleanup` RAII guard and early drop pattern established in CW-113. | Characterization tests `regression_test_override_file_cleanup_on_normal_exit_without_consumption` and `regression_test_override_file_cleanup_on_write_failure` pass. |
| Subprocess pollution during tests | Maintained dedicated temporary `HOME`, configuration, and data directories in all tests. | Verified no residual artifacts in user home directory. |

---

## 10. Architectural Boundary Enforcement

In strict compliance with the CW-116 maintainability charter:
- **No changes were made to `crates/cw-proxy`**: Proxy consolidation is deferred to CW-116C.
- **No changes were made to `crates/cw-store` schema**: Database tables, migrations, and SQLite persistence structures remain untouched.
- **No changes were made to `crates/cw-core`**: Core IR, normalizers, and optimization algorithms remain unchanged.
- **Zero-Drop Proxy Transparency**: Raw wire byte forwarding and thinking block signatures remain preserved.

---

## 11. Handoff to CW-116C

With the CLI and harness lifecycles cleanly modularized, the repository is prepared for **CW-116C: Proxy & Transformation Boundary Consolidation**:
1. `crates/cw-proxy` can now be refactored into cohesive routing, streaming, and mutation boundary modules without concern for CLI dispatch entanglement.
2. The qualification tests (`qualification.rs` and `safety.rs`) provide immediate regression defense for proxy streaming and header transformation.
3. Upon completion of CW-116C, the maintainability gate will be complete, allowing the resumption of CW-112 (Aider Integration) and subsequent Horizon 5 initiatives.
