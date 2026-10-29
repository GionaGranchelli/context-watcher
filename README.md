```text
               _            _ __          __   _       _               
  ___ ___  _ _| |_ _____ __| |\ \        / /__| |_ ___| |_  ___ _ _ 
 / _/ _ \| ' \  _/ -_) \ /  _| \ \  /\  / / _ \  _/ _ \ ' \/ -_) '_|
 \__\___/|_||_\__\___/_\_\___|  \_\/  \/ \___/\__\___/_||_\___|_|  
```

# contextWatcher (`cw`) — Local Context Analyzer, Intercepting Proxy & KV-Cache Optimizer

[![License: MIT / Apache-2.0](https://img.shields.io/badge/License-MIT%20%2F%20Apache--2.0-blue.svg)](LICENSE)
[![Rust: 2024](https://img.shields.io/badge/Rust-2024%20Edition-orange.svg)](https://www.rust-lang.org)
[![Benchmarks](https://img.shields.io/badge/Benchmarks-Reproducible%20Fixtures-brightgreen.svg)](benchmark/README.md)
[![Compatibility](https://img.shields.io/badge/Compatibility-Contract-informational.svg)](docs/COMPATIBILITY.md)
[![Guide](https://img.shields.io/badge/User%20Guide-GUIDE.md-purple.svg)](GUIDE.md)

> **A local-first context observer, analyzer, and opt-in mutation overlay engine for AI coding harnesses.**  
> Inspects token usage, KV-cache prefix boundaries, duplicate file reads, and verbose tool logs across turns—with deterministic, reversible overlays in Rust, SQLite persistence, CLI/MCP tools, a Svelte 5 Web UI, and an IntelliJ plugin.

Supports **Antigravity (AGY)**, **Claude Code**, **Codex CLI**, **Pi / OMP**, **Hermes**, **OpenCode**, and **Aider** (see the [Compatibility Matrix](docs/COMPATIBILITY.md) for per-harness observation, active proxy overlay, and transcript export qualification).

---

## ⚡ Quickstart

### 1. Install & Initialize

Install `cw` and configure detected local coding harnesses:

```bash
# Via GitHub installer script:
curl -fsSL https://raw.githubusercontent.com/GionaGranchelli/context-watcher/main/install.sh | sh

# Or build locally from source:
cargo install --path crates/cw-cli
cw init
```

`cw init` auto-discovers installed harnesses and registers:
* **Antigravity (AGY):** MCP server + `/context-watcher` & `/cw` skills in `~/.gemini/antigravity-cli/`
* **Claude Code:** User-scope MCP server (`contextwatcher`) + slash commands (`/cw`) in `~/.claude/`
* **Codex CLI:** MCP server entry (`[mcp_servers.contextwatcher]`) & `/cw` skills in `~/.codex/`
* **Pi / OMP (oh-my-pi):** Extension hook & commands in `~/.pi/agent/` / `~/.omp/agent/`
* **Hermes Agent:** Middleware plugin & `/cw` skills in `~/.hermes/`
* **OpenCode:** MCP server config & plugin in `~/.config/opencode/`

### 2. Start the Daemon & Web UI Locally

```bash
# Start the local control API / Web UI (port 4400) and intercepting proxy (port 4401)
cw serve --port 4400 --proxy-port 4401

# Print the local control token for Web UI / API authentication
cw auth-token
```

Open `http://127.0.0.1:4400` and enter the token printed by `cw auth-token`. Both listeners bind strictly to loopback (`127.0.0.1`). Configure upstream provider URLs before starting the daemon when routing through custom endpoints:

```bash
export CW_OPENAI_UPSTREAM=http://127.0.0.1:8080
export CW_ANTHROPIC_UPSTREAM=https://api.anthropic.com
export CW_GEMINI_UPSTREAM=https://generativelanguage.googleapis.com
```

### 3. Inspect, Preview & Optimize a Session

```bash
cw status                           # Check token fullness, KV cache hit %, and health score
cw lint                             # Diagnose duplicate file reads, noisy logs, and cache breaks
cw optimize --dry-run               # Preview all optimization strategies without applying changes
cw optimize -s min                  # Apply 0-KV-break candidates (uncached duplicate reads & rule pins)
cw optimize -s clean_code_state     # Deduplicate all covered duplicate/stale file reads
cw optimize -s mid                  # Compact old completed tool outputs and unsigned thinking blocks
cw optimize -s max --sync-back      # Apply full conservative diet + export a compacted .jsonl copy
cw preview                          # Preview the latest turn with active overlays applied in memory
cw gain                             # View token and estimated cost savings across sessions
```

### 4. Keeping `cw` Updated

Update an installed CLI binary at any time with:

```bash
cw update                # Download & SHA-256 verify latest release, refresh plugins, restart daemon
cw update --from-source  # Build latest main branch from source, install, and restart daemon
cw update --no-restart   # Upgrade binary without restarting the background daemon
```

*(Note: The release SHA-256 checksum verifies archive integrity against corruption; it is not a cryptographic publisher signature.)*

---

## 🩺 How `contextWatcher` Works

By default, `cw` operates in **observation-only mode**: it records requests and streaming responses, normalizes supported provider payloads into a **Canonical Context Graph (`ContextTurn` / `ContextNode`)**, and retains the original JSON and wire bytes.

When active optimization is invoked (via CLI, Web UI, or MCP tools), `cw` evaluates three core signals on every turn:

1. **Covered Duplicate File Reads (`CW-L001` / `CW-L002`):** When an agent reads the same file multiple times and a later successful read covers the exact contents/line range of an earlier read, `cw` can replace the older copy with a compact 1-line pointer stub (`[cw:shadow_deduped ...]`) while keeping the newest read verbatim.
2. **Completed Tool Output & Reasoning Noise:** Verbose command outputs (`cargo test`, `npm test`, `git diff`) and unsigned internal `<thinking>` blocks from completed *past* turns can be compacted into 1-line stubs while preserving `tool_use_id` / `call_id` pairing.
3. **Instruction Salience ("Lost in the Middle"):** When long contexts (>30k tokens) push early `InjectedRule` nodes into low-salience positions (<42% salience), `cw` can append a `<cw_pinned_context>` reminder at the prompt tail without modifying the early prefix.

### 🛡️ Built-In Safety Guards

* **Current & Unread Tail Protection:** Tool results at the tail of the prompt (after the last assistant turn or in the current turn), `contextWatcher`'s own MCP outputs, and `<SKILL>` payloads are never auto-pruned.
* **Signed Content Preservation:** Payloads containing cryptographic thinking signatures (Anthropic `signature` / `signature_delta`, Gemini `thoughtSignature`, or OpenAI encrypted reasoning items) are preserved intact and never mutated.
* **Non-Destructive Transcript Export (`--sync-back` / `export_compacted`):** Exporting compactions writes a **separate** `.cw-compacted.jsonl` file and never overwrites the harness's live source transcript in-place.
* **Lossless Original Storage:** Every turn's unmutated raw payload is stored in SQLite (`~/.local/share/context-watcher/cw.db`), so overlays can be inspected, previewed, or cleared (`cw_unpin_context` / API delete) at any time.

---

## 🎛️ Optimization Strategies & Tiers

| Strategy ID | Alias | What It Selects | KV Cache Prefix Impact |
| :--- | :---: | :--- | :--- |
| `cache_safe_boost` | `min` | Zero-KV-break candidates only (`kv_break_tokens == 0`): low-salience rule reminders plus uncached duplicate reads and uncached tail bloat | **0 cached prefix tokens broken** (preserves any `CachedHit` prefix) |
| `clean_code_state` | `clean` | Covered duplicate/stale file reads across the session where a newer read covers identical contents | May modify prefix if earlier read was cached |
| `surgical_log_diet` | `mid` | Completed past-turn tool outputs (>100 tok) and unsigned thinking blocks | May modify prefix |
| `deep_trajectory_fold` | `fold` | Manual turn-range folding only (`cw fold <start> <end> --summary "..."` or `cw_fold_turns`). Automatic folding without a verified summary is disabled. | Modifies prefix across folded turns |
| `max_token_diet` | `max` | Combines `clean_code_state` + `surgical_log_diet` + `pin_rule` reminders | May modify prefix |

### Invoking via MCP Tools (Inside Any Agent Session)

When the `contextwatcher` MCP server (`cw mcp`) is registered, agents can inspect and compact their own context window:
* `cw_context_status`: Inspect token fullness, KV cache hit ratio, health score, and active lints (automatically resolves the current workspace session if `session_id` is omitted and unambiguous).
* `cw_optimize_strategy`: Apply `min`, `mid`, `max`, or `clean_code_state` as a session overlay, and optionally pass `"export_compacted": true` to write a separate compacted `.jsonl` export.
* `cw_evict_stale_reads`: Shadow-deduplicate covered duplicate file reads.
* `cw_prune_tool_output`: Stub a specific noisy tool output by `call_id` or `target_fingerprint`.
* `cw_fold_turns`: Fold a completed range of turns (`from_turn`..`to_turn`) into a caller-supplied outcome `summary`.
* `cw_pin_context` / `cw_unpin_context`: Pin or remove `<cw_pinned_context>` tail reminders.

---

## 📊 Reproducible Benchmarks & Validation

Recorded session fixtures and benchmark harnesses are checked into [`benchmark/`](benchmark/README.md). As documented in [`benchmark/README.md`](benchmark/README.md), we distinguish between:
1. **Deterministic Fixture Replay (`benchmark/fixtures/`):** Measures structural token reductions and overlay behavior on recorded multi-turn coding transcripts.
2. **Hardware Prefill & Continuation Benchmarks (`benchmark/run_benchmark.py`):** Evaluates local LLM prefill latency (TTFT) and post-compaction unit-test pass rates on target hardware.

> Token counts, attention scores, cache-break estimates, model VRAM calculations, and dollar costs are local heuristics unless a field explicitly reports provider-observed usage. No fixed token-reduction percentage, latency speedup, or lossless continuation quality is guaranteed across arbitrary prompts—always validate with [`benchmark/README.md`](benchmark/README.md) on your workload.

### Running the Workspace Quality Gates

```bash
cargo check --workspace
cargo test --workspace
cargo clippy --workspace -- -D warnings

cd web
npm ci
npm run check
npm run build
```

---

## 🏗️ Architecture

```text
[ Coding Harness (AGY, Claude Code, Codex, OMP, Hermes, OpenCode, Aider) ]
                 │
                 ├── 1. Loopback HTTP/SSE Proxy (127.0.0.1:4401) or Transcript Import
                 ▼
 ┌──────────────────────────────────────────────────────────────────────────┐
 │                     contextWatcher Core (Rust 2024)                      │
 │                                                                          │
 │   Canonical Context IR (cw-core)   Radix Tree & Linter (cw-core)         │
 │   • Lossless raw JSON envelope     • KV-cache prefix diffing             │
 │   • Preserves signed thinking      • Attention-salience scoring          │
 │                                                                          │
 │   Mutation Overlay Engine          SQLite Store (cw-store)               │
 │   • Dedupe / Prune / Fold / Pin    • Immutable raw wire blobs            │
 │   • Content-hash keyed rules       • Revision-checked overlay ledger     │
 └──────────────────────────────────────────────────────────────────────────┘
                 │
                 ├── 2. Control API & Web UI (127.0.0.1:4400) / MCP (cw mcp) / TUI
                 └── 3. Non-destructive compacted export (.cw-compacted.jsonl)
```

* **`crates/cw-core`:** Canonical IR, normalizer, KV-cache prefix analyzer, context linter, mutation engine, and optimization planner.
* **`crates/cw-store`:** SQLite persistence (`~/.local/share/context-watcher/cw.db`), content-addressed blob store with automatic orphaned-blob GC, and revision-checked mutation ledger.
* **`crates/cw-proxy`:** Loopback Axum HTTP/SSE proxy and authenticated `/api/v1/*` control API.
* **`crates/cw-harnesses`:** Transcript importers and non-destructive compacted exporters.
* **`crates/cw-mcp`:** Built-in stdio JSON-RPC 2.0 MCP server (`cw mcp`).
* **`crates/cw-tui` & `crates/cw-cli`:** Ratatui terminal dashboard and main `cw` binary.
* **`web/` & `ide/cw-intellij/`:** Svelte 5 + XYFlow Web UI and Kotlin IntelliJ Platform plugin.

See [`specs/`](specs/) for technical specifications, [`GUIDE.md`](GUIDE.md) for local operations, and [`docs/COMPATIBILITY.md`](docs/COMPATIBILITY.md) for the full compatibility and security contract.

---

## 🔒 Data, Security & Storage Retention

* **Local Plaintext Store:** SQLite (`~/.local/share/context-watcher/cw.db`) and exported `.jsonl` files store plaintext conversation history under private user file permissions (`0600` / `0700`). Provider API keys in `Authorization` headers are forwarded upstream and stripped from stored payload history.
* **Control Token Authentication:** Control API routes require `Authorization: Bearer <token>` (stored in a private per-user token file, inspectable via `cw auth-token`). Proxy requests only bind to an active overlay session when carrying both `x-cw-session-id` and `x-cw-control-token`. The token is a same-user credential, not an OS sandbox boundary—see [`docs/COMPATIBILITY.md`](docs/COMPATIBILITY.md) for per-harness wrapper isolation details.
* **Safety Migration & Auto-Pilot Notice:** Upgrading an older database (`safety_version < 2`) automatically disables legacy background Auto-Pilot settings until explicitly re-enabled via `cw autopilot --enable`. Before applying the additive safety schema migration, `cw` creates a private `cw.pre-safety-*.db` snapshot **only if the existing database is $\le 64\text{ MB}$**; databases larger than `64 MB` skip the automatic file copy to avoid exhausting disk space (it does not create a truncated backup).
* **Purging History:** Use `cw purge --session <id>` to remove a session's stored turns and unchanged exports, or `cw purge --session <id> --keep-turns N` to retain only the newest $N$ turns and clean up unreferenced blobs.

---

## 📄 License

Dual-licensed under either of:
* **Apache License, Version 2.0** ([LICENSE-APACHE](LICENSE-APACHE) or [apache.org](https://www.apache.org/licenses/LICENSE-2.0))
* **MIT License** ([LICENSE-MIT](LICENSE-MIT) or [opensource.org](https://opensource.org/licenses/MIT))

at your option.
