# contextWatcher (`cw`)

> **The Wireshark, Chrome DevTools, and Surgical Debugger for LLM Agent Harnesses.**

`contextWatcher` (`cw`) is an intercepting proxy, state engine, analyzer, and active context manipulation suite for AI coding harnesses (**Antigravity / AGY**, **Codex CLI**, **Pi**, **OpenCode**, **Aider**, **Claude Code**, and any OpenAI/Anthropic/Gemini-compatible agent).

Instead of treating an LLM harness's context window as an opaque, append-only black box, `contextWatcher` turns the context into an **inspectable, evaluatable, and mutable Radix Tree / DAG**.

---

## Why `contextWatcher`?

1. **See the Ground Truth (Check & Analyze)**
   * Inspect the **exact raw payload** sent by any harness: hidden system prompts, injected rules, skill descriptions, MCP tool schemas, file reads, tool outputs, and thinking blocks.
   * **KV Cache X-Ray:** Reconstructs token-prefix Radix Trees across turns to visualize what is **Cached in KV (Green)**, **Newly Written (Yellow)**, or **Cache-Invalidated (Red)**—pinpointing the exact token index and cause of cache breaks.

2. **Diagnose Context Rot (Evaluate)**
   * **Stale File Detection:** Flags when the context contains an older `read_file` of a file that has since been modified on disk or by later tool calls.
   * **Duplicate Read & Output Detection:** Identifies redundant file reads or repeated command outputs eating up the window.
   * **Attention & Bloat Linting:** Measures MCP tool schema overhead, detects "Lost in the Middle" risk for early instructions, and scores overall Context Health.

3. **Surgical Manipulation (Interact & Mutate)**
   * **Deterministic Mutation Ledger (Overlay Engine):** Mutate past turns in the proxy without losing synchronization with the harness's in-memory history.
   * **Subtree Folding:** Collapse a 10-turn failed debugging rabbit hole into a single 80-token outcome node so the model stops imitating prior mistakes.
   * **Shadow Deduplication:** Automatically replace stale/duplicate earlier file reads with lightweight pointers to the latest version.
   * **Dynamic Tool & Rule Gating:** Toggle MCP servers, individual tool schemas, or skills on/off mid-session without restarting the harness.
   * **Context Firewall (Breakpoints):** Pause requests automatically when a tool output exceeds a token threshold (e.g., a 30k-token stack trace), edit/filter it in the UI, and forward.
   * **Context Pinning (Attention Boosting):** Pin critical architectural constraints so they stay in the high-attention tail of the prompt without invalidating the KV cache prefix.
   * **Time-Travel Forking & A/B Replay:** Branch from any historical turn, prune or edit nodes, and replay side-by-side to evaluate model behavior.

---

## System Architecture

```
  [ Antigravity (AGY) | Codex CLI | Pi | OpenCode | Aider | Claude Code ]
                │                                       ▲
                │ 1. HTTP/SSE Proxy                     │ 2. Transcript Sync /
                │    (OpenAI / Anthropic / Gemini)      │    Native Hooks / MCP
                ▼                                       ▼
┌─────────────────────────────────────────────────────────────────────────┐
│                       contextWatcher Core Daemon                        │
│                         (Rust • Axum • Tokio)                           │
│                                                                         │
│  ┌─────────────────────┐  ┌─────────────────────┐  ┌─────────────────┐  │
│  │ Protocol Normalizer │  │ Radix & KV Cache    │  │ Mutation Ledger │  │
│  │ (Anthropic/OAI/Gem) │─▶│ Engine + Linter     │─▶│ & Overlay Engine│  │
│  └─────────────────────┘  └─────────────────────┘  └─────────────────┘  │
│                                    │                                    │
│                        ┌───────────▼───────────┐                        │
│                        │   SQLite State Store  │                        │
│                        └───────────┬───────────┘                        │
└───────────────┬────────────────────┼────────────────────┬───────────────┘
                │ REST + WebSocket   │ Local IPC / REST   │ STDIO / SSE
                ▼                    ▼                    ▼
     ┌────────────────────┐ ┌─────────────────┐ ┌─────────────────────────┐
     │  Web UI Dashboard  │ │   CLI & TUI     │ │ IDE & Harness Plugins   │
     │  (Svelte 5 +       │ │   (Ratatui +    │ │ • IntelliJ Plugin       │
     │   XYFlow + D3)     │ │    Clap)        │ │ • AGY / Codex / Pi MCP  │
     └────────────────────┘ └─────────────────┘ └─────────────────────────┘
```

---

## Tech Stack

* **Core Daemon, Proxy, CLI & TUI:** Rust (`axum`, `tokio`, `reqwest`, `ratatui`, `clap`, `rusqlite`, `tiktoken-rs`)
* **Web UI Dashboard:** Svelte 5 (SvelteKit static build embedded via `rust-embed`), TypeScript, TailwindCSS, `@xyflow/svelte` (Svelte Flow), `d3-hierarchy` (Treemap & Flamegraph)
* **IDE Integration:** Kotlin + IntelliJ Platform SDK (`cw-intellij`)
* **Harness Integration:** Native MCP Server (`cw mcp`), transcript watchers (`AGY`, `Codex`, `Pi`, `OpenCode`, `Claude`), and slash-command skills.

---

## Repository Structure & Documentation

* [`AGENTS.md`](file:///home/gionag/Development/contextWatcher/AGENTS.md) — Engineering guidelines, architecture invariants, and workflow rules for AI agents and contributors.
* [`ROADMAP.md`](file:///home/gionag/Development/contextWatcher/ROADMAP.md) — Phased product and engineering roadmap (Phases 0–6).
* [`PLAN.md`](file:///home/gionag/Development/contextWatcher/PLAN.md) — Execution strategy, Cargo workspace layout, testing methodology, and delivery plan.
* [`BOARD.md`](file:///home/gionag/Development/contextWatcher/BOARD.md) — Active Kanban task board (`CW-001` through `CW-040`).
* **Technical Specifications (`specs/`):**
  * [`specs/00-architecture.md`](file:///home/gionag/Development/contextWatcher/specs/00-architecture.md) — Daemon architecture, protocol normalization, storage schema, and REST/WS API.
  * [`specs/01-context-graph-and-kv-cache.md`](file:///home/gionag/Development/contextWatcher/specs/01-context-graph-and-kv-cache.md) — Context Graph, Radix Prefix Tree, KV Cache boundary math, and Context Health Linter.
  * [`specs/02-mutation-engine.md`](file:///home/gionag/Development/contextWatcher/specs/02-mutation-engine.md) — Mutation Ledger, Overlay Engine, Fold/Prune/Dedupe/Pin/Gate/Firewall operations.
  * [`specs/03-web-ui-and-tui.md`](file:///home/gionag/Development/contextWatcher/specs/03-web-ui-and-tui.md) — Svelte 5 Web UI views, XYFlow graph interactions, and Ratatui TUI / CLI commands.
  * [`specs/04-harness-and-ide-plugins.md`](file:///home/gionag/Development/contextWatcher/specs/04-harness-and-ide-plugins.md) — Adapters for Antigravity (AGY), Codex CLI, Pi/OpenCode/Aider, MCP Server, and IntelliJ Plugin.
