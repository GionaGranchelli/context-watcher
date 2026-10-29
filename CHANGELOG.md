# Changelog

All notable changes to `contextWatcher` (`cw`) are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.4] - 2026-10-29

### Changed
- **Web UI Proportional Token Treemap & "Lost-in-the-Middle Trough" Redesign (`web/src/App.svelte`):**
  - Expanded output/context classification into **14 distinct color-coded types** (`Terminal & Shell Outputs`, `Search & Grep Outputs`, `File Reads (Fresh)`, `File Reads (Stale / Dup)`, `File Edits & Diffs`, `MCP & Other Tool Outputs`, `User Prompts`, `Assistant Replies & Calls`, `Assistant Thinking`, `System & Identity`, `Rules & Instructions`, `MCP & Native Tool Schemas`, `Skills Catalog`, and `Pinned & Folded Summaries`).
  - Added **Balanced Perceptual Scale vs. Exact Linear Scale** toggle and **`By Output Type` vs. `By Attention Zone (Trough Split)`** grouping so large 350k+ token conversations remain 100% legible without dominant tool outputs squashing smaller categories into unlabeled slivers.
  - Added diagonal amber hatching on treemap tiles showing the exact share of each category trapped in the **Lost-in-the-Middle Trough (`<42% Salience`)**, plus a full-width 100% stacked composition ribbon and 3-zone attention distribution strip.
  - Redesigned the **Positional Context Map & "Lost-in-the-Middle Trough" U-Curve** with token-balanced auto-binning (`positionalContextBins`), a dedicated top 3-zone header bar, an external `54px` left Y-axis gutter (`100%` / `42%` / `0%`), a bottom token position ruler, and a filterable **Context Block Explorer** with 1-click **`📌 Pin to High-Attention Tail`**.
  - Removed the `1120px` max-width cap on the central workspace so analytics, treemap, and timeline views utilize the full screen width.

## [0.2.3] - 2026-10-29

### Added
- **Self-Update Command (`cw update` / CW-090):** Added `cw update` (`--check`, `--from-source`, `--no-restart`, `--version <tag>`) in `crates/cw-cli/src/update.rs` to download the latest GitHub release archive, verify its SHA-256 checksum, atomically replace `~/.local/bin/cw`, refresh harness integrations (`cw init`), and restart the local daemon.
- **Unpin Context MCP Tool (`cw_unpin_context`):** Added `cw_unpin_context` MCP tool and `DELETE /api/v1/sessions/{id}/mutations` (`?only_pins=true&label=...`) so agents and users can remove pinned `<cw_pinned_context>` reminders.
- **Local Control Token & Loopback Hardening (`cw-core::control`):** Added per-user private control token (`cw auth-token`), `Authorization: Bearer <token>` and `If-Match` revision concurrency checks across control routes, and strict loopback `Host` / `Origin` validation.
- **Hermes Middleware Plugin (`plugins/hermes/context-watcher/`):** Added authenticated `llm_request` middleware plugin, gateway session/workspace resolution (`gateway.session_policy._ACTIVE_POLICY` + `agent.session_client_env`), and `cw wrap hermes chat` session-override handoff.
- **Agent Setup & Architecture Docs:** Added `docs/AGENT_SETUP.md`, `docs/HOW_IT_WORKS.md`, `docs/COMPATIBILITY.md`, `docs/REMEDIATION.md`, and safety/live-provider test suites (`crates/cw-core/tests/safety_test.rs`, `crates/cw-proxy/tests/safety.rs`, `crates/cw-proxy/tests/live_provider.rs`, `crates/cw-store/tests/safety.rs`).

### Changed
- **Auto-Pilot Low-Health Self-Healer Fallback (`cw-core`):** Updated default `heal_strategy` to `"surgical_log_diet"` (`mid`) and added automatic fallback across `clean_code_state` -> `surgical_log_diet` -> `max_token_diet` when a session's health drops below `min_health_score`, ensuring harnesses that rely on terminal/shell tools (like Hermes) are compacted when tool logs bloat context.
- **Hermes `cw wrap` & `cw status` Resolution (`cw-cli`):**
  - `cw wrap` now accepts `--session-id <id>` either before or after `hermes chat`, writes a `0600` session-override handoff for the background Hermes gateway, and avoids injecting conflicting `--base-url` CLI flags on `hermes chat`.
  - `cw status` (and `resolve_session`) now supports prefix matching on `--session <id>` (e.g., short `local-XXXXXXXX` IDs) and automatically selects the most recently updated session in the current workspace (or latest overall).
  - `cw init` now restarts `contextwatcher.service` and triggers `reload-plugins` on a running Hermes gateway.
- **Non-Destructive Transcript Export (`export_compacted` / `--sync-back`):** `cw sync-back`, `cw optimize --sync-back`, and MCP `cw_optimize_strategy` (`export_compacted: true`, with `sync_to_disk: true` retained as a legacy alias) now export a separate `.cw-compacted.jsonl` copy and leave the source harness transcript unmodified.
- **Zero-KV-Break Minimum Strategy (`min` / `cache_safe_boost`):** `min` selects only zero-KV-break candidates (`kv_break_tokens == 0`: low-salience rule reminders plus uncached duplicate reads and uncached tail bloat) while preserving any `CachedHit` prefix. Full covered duplicate-read deduplication across cached prefixes is available via `clean_code_state` (`clean`).
- **Explicit Summary Required for Turn Folding:** Automatic trajectory folding without a verified summary is disabled; `cw optimize -s fold` directs callers to `cw fold <start_turn> <end_turn> --summary "..."` or `cw_fold_turns`.
- **Fail-Closed Safety Migration for Auto-Pilot (`safety_version = 2`):** Legacy Auto-Pilot configurations (`safety_version < 2`) are disabled on upgrade and surfaced with an explicit CLI migration notice (`cw status` / `cw autopilot`) instructing users to run `cw autopilot --enable` to re-enable background optimization.
- **MCP Workspace Session Resolution:** `cw mcp` tools resolve omitted `session_id` parameters via `CW_SESSION_ID` or an unambiguous workspace match against the current working directory, and return candidate session IDs when multiple sessions match.

### Fixed
- **Proxy Non-Turn Probe Filtering & Session Resume (`cw-proxy`):** Filtered out non-conversation JSON probe requests (such as model metadata probes without `messages`/`input`/`contents`/`prompt`) from creating empty `0`-token sessions, and allowed workspace/model updates when resuming an existing session under the same provider.
- **SQLite Blob Garbage Collection & Migration Backup Cap (`cw-store`):**
  - Kept imported transcript turn IDs stable (`turn_{session_id}_{seq}`) across re-imports so updated snapshots replace the existing turn row in-place.
  - Added automatic reference-counted cleanup of superseded `raw_request_hash`, `mutated_request_hash`, and `raw_response_hash` blobs in `CwStore::save_turn`, preventing multi-gigabyte database growth during live transcript watching.
  - Capped automatic `cw.pre-safety-*.db` migration snapshots to existing databases `<= 64 MB` (skipping the file copy for larger databases to avoid exhausting disk space).
- **OpenCode `<cw_pinned_context>` Duplication & Unread Tail Pruning (`cw-core`):**
  - Stopped auto-pinning `UserMessage` turns in the optimizer/auto-pilot and added idempotent deduplication + inner-tag stripping when injecting `<cw_pinned_context>`.
  - Protected unread tool results at the prompt tail, current-turn tool results, `cw_*` MCP inspection outputs, and `<SKILL>` payloads from automatic pruning.

## [0.2.2] - 2026-10-28

### Added
- OpenCode V2 SQLite storage importer (`opencode.db`) and cross-platform home directory discovery in `cw-harnesses`.
- Reproducible benchmark runner (`benchmark/run_benchmark.py`) and recorded `.jsonl` session fixtures (`benchmark/fixtures/`).

### Fixed
- Validated MCP tool names before making daemon network calls in `cw-mcp`.
- Replaced retired `macos-13` GitHub Actions runner in `.github/workflows/ci.yml`.
