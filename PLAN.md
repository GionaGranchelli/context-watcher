# PLAN.md — Engineering & Execution Plan

This document outlines the technical execution strategy, crate responsibilities, build pipeline, and verification plan for `contextWatcher`.

---

## 1. Crate & Module Architecture

### 1.1 Rust Workspace (`crates/`)
| Crate | Responsibility | Key Dependencies |
| :--- | :--- | :--- |
| **`cw-core`** | Canonical Context IR (`ContextTurn`, `ContextNode`), Protocol Normalizers (Anthropic, OpenAI, Gemini), Radix Tree & KV Cache Prefix Diffing, Linter (Stale/Duplicate/Bloat/Attention), Mutation Ledger & Overlay Engine. | `serde`, `serde_json`, `tiktoken-rs`, `blake3`, `similar`, `thiserror` |
| **`cw-store`** | SQLite state store for sessions, raw payloads, normalized graphs, KV cache snapshots, mutation overlays, and replay forks. | `rusqlite` (bundled), `tokio`, `serde_json` |
| **`cw-proxy`** | High-performance HTTP/SSE MITM reverse proxy (`/anthropic/*`, `/openai/*`, `/gemini/*`). Applies active mutations/firewalls on ingress and taps streaming SSE responses on egress. | `axum`, `tokio`, `reqwest`, `tower-http`, `futures`, `eventsource-stream` |
| **`cw-harnesses`** | File watchers and transcript parsers/mutators for **Antigravity (AGY)** (`transcript.jsonl`), **Codex CLI**, **Pi / OpenCode / Aider**, and **Claude Code**. | `notify`, `serde_json`, `glob`, `walkdir` |
| **`cw-mcp`** | JSON-RPC MCP server (`stdio` and `sse`) allowing any agent harness to query its own context health, fold noisy turns, pin facts, or evict stale file reads. | `serde`, `serde_json`, `tokio` |
| **`cw-tui`** | Interactive terminal dashboard for inspecting the context tree, KV cache bar, lint warnings, and toggling mutations via keyboard shortcuts. | `ratatui`, `crossterm` |
| **`cw-cli`** | Unified `cw` binary combining the daemon server (`cw serve`), harness launcher (`cw wrap <cmd>`), CLI inspector (`cw status`, `cw tree`, `cw lint`, `cw fold`), TUI (`cw tui`), and embedded Svelte 5 Web UI (`rust-embed`). | `clap`, `rust-embed`, `tracing`, `anyhow` |

### 1.2 Web UI (`web/`)
* **Framework:** Svelte 5 + SvelteKit (`@sveltejs/adapter-static`) + TypeScript + TailwindCSS.
* **Key Views:**
  1. **Radix Tree / DAG Canvas (`@xyflow/svelte`):** Interactive node graph showing System Prompts, Rules, Skills, MCP Tool Schemas, Conversation Turns, Tool Calls/Outputs, and Forked Replays.
  2. **Token Treemap & KV Cache Bar (`d3-hierarchy`):** Instant visual breakdown of where tokens are spent and where KV cache hits/misses occur.
  3. **Mutation & Firewall Studio:** Drawer to prune, stub, fold subtrees, toggle MCP tools, manage pinned context, and resolve paused Firewall breakpoints.

### 1.3 IntelliJ Plugin (`ide/cw-intellij/`)
* **Framework:** Kotlin + IntelliJ Platform Gradle Plugin 2.x.
* **Features:**
  * Connects to `ws://127.0.0.1:4400/api/v1/ws`.
  * **File Status Decorations:** Highlights open editor tabs and project tree files based on whether they are currently in the active LLM context (`Fresh`, `Stale`, `Pinned`).
  * **Editor Intention Actions:** Right-click code $\rightarrow$ *"Pin selection to LLM Context"* or *"Evict stale file reads from LLM Context"*.
  * **ContextWatcher Tool Window:** Native Kotlin/Swing + embedded JCEF (or native tree table) view of the active session.

---

## 2. Dual-Mode Integration Strategy (Proxy + Transcript Sync)

Some harnesses allow easy base URL redirection (`OPENAI_BASE_URL`, `ANTHROPIC_BASE_URL`, `GEMINI_API_BASE_URL`), while others use custom transports or write detailed local transcripts (like AGY's `transcript.jsonl`).

To ensure **100% compatibility across AGY, Codex, Pi, OpenCode, Aider, and Claude Code**, `contextWatcher` operates in **Hybrid Mode**:
1. **Proxy Mode (Active Intercept):** Intercepts raw HTTP/SSE requests at `http://127.0.0.1:4401/{provider}/*`, giving ground-truth visibility into network headers, exact KV cache tokens, and in-flight mutation interception.
2. **Transcript Watcher Mode (Local Disk Sync):** Watches the harness's native session directory (e.g., `~/.gemini/antigravity-cli/brain/<id>/.system_generated/logs/transcript.jsonl`), reconstructing the context graph in real time and syncing mutations either via disk edits or via the **MCP Self-Management Bridge (`cw mcp`)**.

---

## 3. Verification & Testing Strategy

1. **Golden Fixture Suite (`crates/cw-core/tests/fixtures/`):**
   * Recorded multi-turn payloads for Anthropic (with thinking & `cache_control`), OpenAI Chat/Responses (with reasoning & tool calls), Gemini (with `thoughtSignature`), and AGY `transcript.jsonl`.
   * Assert **lossless round-trip serialization**: `parse_to_ir(raw) -> serialize_from_ir(ir) == raw` when zero mutations are applied.
2. **KV Cache Prefix Diff Tests:**
   * Verify that inserting a dynamic timestamp at token 450 accurately marks tokens `0..450` as `CachedHit` and tokens `450..N` as `CacheInvalidated` with the exact diff cause.
3. **Mutation Overlay Idempotency Tests:**
   * Verify that applying `Fold(Turn 4..7)`, `ShadowDedupe(file="src/main.rs")`, and `DisableTool("chrome_click")` on Turn 10 automatically re-applies cleanly when the harness sends Turn 11, 12, and 13.
4. **End-to-End Proxy Streaming Tests:**
   * Spin up a mock upstream SSE server, send streaming tool-call responses through `cw-proxy`, and verify chunk pass-through latency and SQLite persistence.

---

## 4. Execution Sequence (First Sprint)

1. **Step 1 (`CW-001` – `CW-005`):** Scaffold Rust Cargo workspace and implement `cw-core` Canonical IR, loss-free raw JSON pointer mapping, and tokenizer.
2. **Step 2 (`CW-006` – `CW-011`):** Build `cw-store` (SQLite) and `cw-proxy` (Axum passthrough + SSE tap + REST/WS daemon API) + `cw-harnesses` (AGY, Codex, Pi watchers).
3. **Step 3 (`CW-012` – `CW-016`):** Implement Radix Tree builder, KV Cache boundary analyzer, and Context Health Linter (Stale files, Duplicates, Tool bloat).
4. **Step 4 (`CW-017` – `CW-023`):** Scaffold Svelte 5 Web UI (`web/`) with XYFlow Context Graph & D3 Treemap, plus `cw-tui`.
5. **Step 5 (`CW-024` – `CW-033`):** Implement the Mutation Engine (Overlay ledger, Prune, Fold, Shadow-Dedupe, Tool-Gate, Pin, Firewall, Replay).
6. **Step 6 (`CW-034` – `CW-040`):** Ship `cw-mcp`, AGY/Codex/Pi plugins, and the `cw-intellij` plugin.
