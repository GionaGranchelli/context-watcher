# AGENTS.md — AI Agent & Contributor Guide for `contextWatcher`

Welcome to `contextWatcher` (`cw`). Every human or AI agent working in this repository **must** read and follow these guidelines before planning, writing, or modifying code.

---

## 0. Active milestone: CW-117 Cache-Aware Optimization Policy Engine (Phase 1)

**Before coding**, read [BOARD.md](BOARD.md), [docs/COMPATIBILITY.md](docs/COMPATIBILITY.md), and [docs/HARNESS_MATRIX.md](docs/HARNESS_MATRIX.md). Checked historical board tasks are not current product qualification. Ground claims in code, fixtures, CI and reproducible evidence.

- **Completed:** CW-116 Maintainability Gate (PRs #13, #14, #16); CW-112 Codex CLI qualification (PRs #17, #18); CW-114 Long-Session Reliability Campaign across harnesses (PR #20); CW-115 Performance and Savings Qualification (Phase 1 merged via PR #21; Phase 2 four-mode isolation merged via PR #22).
- **Active:** CW-117 Cache-Aware Optimization Policy Engine (Phase 1 shadow-mode decision engine, deterministic qualification, and pure economic model).
- **Next Backlog:** CW-103 broader Codex/Hermes continuation, CW-104 local model qualification, CW-R11 large-history IO qualification.
- Preserve public CLI/API, session identity, authenticated vs observation-only behavior, raw provider wire bytes/streaming, signed and unknown content, transcript originals, mutation eligibility and fail-open behavior. Do not claim observed cache savings where only estimates exist.
- Use temporary isolated HOME/config and data directories, mock ports and test stores. Do not edit actual user Hermes config, live session transcripts, installed agent settings or running services.
- Explicitly distinguish default CI tests, separately invoked ignored harness-binary tests and opt-in live-provider tests. Never report a skipped or unavailable test as passed. Avoid duplicating production algorithms or guards inside tests.

---

## 1. Mission & Core Philosophy

`contextWatcher` is a **local-first, ultra-low-latency intercepting proxy, analyzer, and active manipulation engine** for LLM agent harnesses (Antigravity/AGY, Codex CLI, Pi, OpenCode, Aider, Claude Code).

### Golden Architectural Invariants
1. **Zero-Drop Proxy Transparency:** In passthrough mode (no active mutations or breakpoints), the proxy must never corrupt provider-specific fields, SSE streaming chunks, thinking signatures (e.g., Anthropic `signature` in `thinking` blocks or Gemini `thoughtSignature`), or cache control headers.
2. **Canonical Context IR + Lossless Raw Envelope:** Every intercepted request or imported session transcript is parsed into a normalized **Canonical Context Graph (`ContextTurn` / `ContextNode`)** for analysis and UI rendering, while preserving a **lossless JSON pointer map** to the original raw payload so mutations only touch targeted JSON paths.
3. **Deterministic Mutation Overlay:** Harnesses hold their own history in memory and re-send the full unmutated conversation on every turn. Therefore, all proxy mutations (Prune, Fold, Shadow-Dedupe, Pin, Tool-Gate) are stored as **deterministic overlays keyed by content/structural hashes** (`NodeFingerprint`) and automatically re-applied on every subsequent request—and optionally synced back to the harness's disk transcript.
4. **KV-Cache Awareness First:** Every mutation must calculate its **KV Cache Impact** before execution. We distinguish between *Prefix-Mutating Operations* (which invalidate the KV cache after token index $k$) and *Tail-Only / Cache-Safe Operations* (which preserve the cached prefix).

---

## 2. Tech Stack & Workspace Layout

```
contextWatcher/
├── Cargo.toml                  # Rust workspace root
├── crates/
│   ├── cw-core/                # Canonical IR, Radix Tree, KV Cache diffing, Linter, Mutation Engine
│   ├── cw-proxy/               # Axum HTTP/SSE intercepting proxy (Anthropic, OpenAI, Gemini)
│   ├── cw-store/               # SQLite persistence, session logs, mutation ledger
│   ├── cw-harnesses/           # Transcript watchers & adapters (AGY, Codex, Pi, OpenCode, Claude)
│   ├── cw-mcp/                 # Built-in MCP server so agents can inspect/fold their own context
│   ├── cw-tui/                 # Ratatui terminal UI dashboard
│   └── cw-cli/                 # Main `cw` binary (daemon, proxy, CLI commands, embedded Web UI)
├── web/                        # Svelte 5 + SvelteKit + TypeScript + XYFlow + D3 Web UI
├── ide/
│   └── cw-intellij/            # Kotlin IntelliJ Platform Plugin
├── plugins/
│   ├── agy/                    # Antigravity skill & MCP config
│   ├── codex/                  # Codex CLI wrapper & hooks
│   ├── pi-opencode/            # Pi / OpenCode / Aider integration scripts
│   └── claude/                 # Claude Code hooks & slash commands
└── specs/                      # Source-of-truth technical specifications
```

---

## 3. Specification-Driven Workflow

Before implementing any feature or ticket from [`BOARD.md`](BOARD.md):

1. **Read the Relevant Spec:**
   * Architecture & APIs $\rightarrow$ [`specs/00-architecture.md`](specs/00-architecture.md)
   * Graph, Radix Tree, KV Cache & Linter $\rightarrow$ [`specs/01-context-graph-and-kv-cache.md`](specs/01-context-graph-and-kv-cache.md)
   * Mutation Engine & Overlays $\rightarrow$ [`specs/02-mutation-engine.md`](specs/02-mutation-engine.md)
   * Web UI (Svelte 5) & TUI/CLI $\rightarrow$ [`specs/03-web-ui-and-tui.md`](specs/03-web-ui-and-tui.md)
   * Harness Adapters & IntelliJ Plugin $\rightarrow$ [`specs/04-harness-and-ide-plugins.md`](specs/04-harness-and-ide-plugins.md)
2. **Update `BOARD.md`:**
   * Move the current ticket to **In Progress** when starting; don't activate later CW-116 subtasks prematurely.
   * Move it to **Done** only after applicable tests pass, optional tests are accounted for, and evidence/docs are updated.
   * Record unsupported or unverified scenarios explicitly rather than treating them as passed.

---

## 4. Coding Conventions & Quality Gates

### Rust (`crates/*`)
* **Edition:** Rust 2024 (or latest stable 2021+ toolchain).
* **Error Handling:** Use `thiserror` in library crates (`cw-core`, `cw-store`, `cw-harnesses`) and `anyhow` in binary/application crates (`cw-cli`). Never `.unwrap()` on untrusted network payloads or transcript JSON lines.
* **Async & Streaming:** Use `tokio` and `axum`. Proxy streaming responses (`text/event-stream`) must stream chunks directly to the harness with $<2\text{ms}$ overhead while tapping chunks asynchronously to the state engine.
* **Testing:** Every mutation operator, KV cache prefix differ, and protocol normalizer in `cw-core` must have deterministic unit tests using recorded real-world fixtures (`tests/fixtures/{anthropic,openai,gemini,agy}/`).

### Web UI (`web/`)
* **Framework:** Svelte 5 (Runes: `$state`, `$derived`, `$effect`), SvelteKit (`adapter-static` for embedding into `cw-cli` via `rust-embed`).
* **Visualization:** `@xyflow/svelte` for the interactive Context DAG / Radix Tree; `d3-hierarchy` for the Token Treemap & KV Cache Flamegraph.
* **State Sync:** Single WebSocket connection (`/api/v1/ws`) driving reactive Svelte 5 stores.

### IntelliJ Plugin (`ide/cw-intellij/`)
* **Language:** Kotlin (JVM 17/21), IntelliJ Platform Gradle Plugin (2.x).
* **Design:** Lightweight client connecting to the local `cw` daemon (`http://127.0.0.1:4400/api/v1/ws`). No heavy tokenization inside the IDE process—delegate all analysis to the Rust daemon.

---

## 5. Essential Commands

```bash
# Rust workspace checks & tests
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
python3 plugins/hermes/context-watcher/test_plugin.py

# Web UI development
cd web && npm install && npm run dev
cd web && npm run build && npm run check

# Run contextWatcher daemon locally (once scaffolded)
cargo run -p cw-cli -- serve --port 4400 --proxy-port 4401
```
