# Spec 03: Web UI Dashboard (Svelte 5 + XYFlow + D3), Ratatui TUI & CLI

> Safety implementation contract (2026-10-29): [COMPATIBILITY.md](../docs/COMPATIBILITY.md) and [REMEDIATION.md](../docs/REMEDIATION.md) supersede conflicting guarantee language below. The remaining text describes the original target design, not proof of completed or measured capabilities.

**Status:** Approved  
**Related Tickets:** `CW-017` – `CW-023`, `CW-033`

---

## 1. Web UI Architecture (`web/`)

* **Framework:** Svelte 5 (Runes mode: `$state`, `$derived`, `$effect`) + SvelteKit with `@sveltejs/adapter-static`.
* **Packaging:** Built into `web/build/` and embedded directly into the `cw` Rust binary via `rust-embed` so `cw serve` requires zero external runtime dependencies.
* **Graph & Chart Libraries:**
  * `@xyflow/svelte` (Svelte Flow) for the interactive **Context Radix Tree / DAG Canvas**.
  * `d3-hierarchy` + SVG/Canvas for the **Token Treemap & KV Cache Flamegraph**.
  * `shiki` or `codemirror` for syntax-highlighted JSON / Markdown inspection and live payload editing.

---

## 2. Web UI Views & Interactive Workflows

### 2.1 Top Telemetry Ribbon (Always Visible)
* **Session Selector:** Switch between live sessions (`AGY #3e54`, `Codex #91a2`, `Pi #04f1`).
* **Context Window Meter:** Live stacked progress bar (`64,210 / 200,000 tokens • 32.1%`).
* **KV Cache Hit Gauge:**
  * 🟢 `88.4% Cached (56.7k tok)` | 🟡 `11.6% New (7.4k tok)` | 🔴 `0 Invalidated`
* **Context Health Score Badge:** e.g., `Health: 78/100 (3 Stale Files, 18 Unused MCP Tools)` with a **"1-Click Optimize"** button.
* **Staged Mutations Queue Indicator:** Shows pending draft mutations and estimated token savings + KV cache impact.

### 2.2 View 1: Context Radix Tree & DAG Canvas (`@xyflow/svelte`)
Visualizes the entire context window and turn history as a hierarchical, zoomable graph:

```
┌─────────────────────────────────────────────────────────────────┐
│ [ Prefix Root ]                                                 │
│   ├── 🛠️ MCP Tools & Native Declarations (32 tools • 9,420 tok) │ 🟢 Cached
│   ├── 📜 System & Rules (AGENTS.md, Skills • 4,110 tok)         │ 🟢 Cached
│   └── 💬 Conversation Spine                                     │
│         ├── Turn 1: User Request + 3 File Reads (6,200 tok)     │ 🟢 Cached
│         ├── Turn 2..6: [📁 Folded Debugging Loop (Saved 18k)]   │ 🟢 Cached
│         ├── Turn 7: Read UserService.kt (2,100 tok) ⚠️ STALE    │ 🟢 Cached
│         ├── Turn 8: Edit UserService.kt + cargo test (8,900 tok)│ 🔴 Cache Break!
│         │     ├── 🌿 Fork A (Original Replay)                   │
│         │     └── 🌿 Fork B (Pruned + Pinned Replay)            │
│         └── 📌 Tail Pinned Context (2 items • 340 tok)          │ ⚪ Tail
└─────────────────────────────────────────────────────────────────┘
```

* **Node Interactions (Right-Click or Action Toolbar on Node):**
  * **Prune / Stub Output** (`PruneOrStub`)
  * **Select Range & Fold Subtree** (`FoldSubtree`)
  * **Shadow-Deduplicate File Reads** (`ShadowDedupe`)
  * **Pin to High-Attention Tail** (`PinContext`)
  * **Fork & Replay from Here** (`ForkAndReplay`)
* **Visual Encodings:**
  * **Border Color:** KV Cache status (`Green` = Hit, `Yellow` = New Write, `Red` = Invalidated, `Gray` = Uncached).
  * **Node Height / Badge:** Proportional to token count.
  * **Warning Pills:** `⚠️ Stale File`, `🔁 Duplicate Read`, `🐘 Bloat (>4k tok)`, `💤 Low Attention Zone`.

### 2.3 View 2: Token Treemap & KV Cache Flamegraph (`d3-hierarchy`)
* **Treemap Mode:** Nested rectangles sized strictly by token count:
  * Outer boxes: `System & Rules`, `MCP Tool Schemas`, `File Reads`, `Command Outputs`, `Assistant Thinking`, `Conversation`.
  * Inner boxes: Individual tools (e.g., `chrome-devtools` server vs. `github` server) or individual files (`src/main.rs` read 3 times = 3 boxes highlighted in orange).
  * Clicking any rectangle opens the Node Inspector or toggles that tool/node off!
* **KV Prefix Flamegraph Mode:** Horizontal token tape from Token `0` to Token `N`, showing exact `cache_control` breakpoints, where the prefix diverged from Turn $N-1$, and the positional attention curve overlay.

### 2.4 View 3: Tool & Skill Gate Matrix
* A dedicated table/tree of all detected **MCP Servers, Native Tools, Rules, and Skills** in the current session.
* Shows:
  * **Token Cost per Turn** (e.g., `chrome-devtools (31 tools): 7,840 tokens/turn`)
  * **Times Called in Session** (`0 calls`)
  * **Toggle Switch:** Flip off unused servers/tools with 1 click (`GateToolsAndSkills`).

### 2.5 View 4: Context Firewall Interceptor Modal & A/B Replay Studio
* **Firewall Banner/Modal:** Flashes when an in-flight request hits a breakpoint (e.g., `cargo test` output is `19,400 tokens`). Allows instant Head/Tail truncation, regex filtering, or manual editing before clicking **"Resume Request"**.
* **A/B Replay Studio:** Split-screen diff comparing two branches from the same historical turn.

---

## 3. Terminal UI (`cw tui` — `ratatui`)

For terminal-centric workflows (tmux, Zellij, SSH), `cw tui` connects to the same daemon WebSocket and provides a 3-pane layout:

```
┌─ contextWatcher v0.1 ── Session: AGY #3e54 ── 64.2k/200k tok ── KV: 88% Hit ── Health: 78 ─┐
│ [1] Context Tree & Turns              │ [2] Node Inspector / Diff / Treemap                │
│ 🟢 🛠  MCP Tools (32)        9,420 tok │ Node: ToolResult (read_file: src/proxy.rs)         │
│ 🟢 📜 System & Rules        4,110 tok │ Status: ⚠️ STALE (Modified at Turn 9)              │
│ 🟢 💬 Turn 1: User + Reads  6,200 tok │ Tokens: 2,410 (Span: 13,530..15,940)               │
│ 🟢 📁 Turn 2..5 [Folded]      180 tok │ KV Cache: 🟢 CachedHit                             │
│ 🟢 ⚠️ Turn 6: read proxy.rs 2,410 tok │ ────────────────────────────────────────────────── │
│ 🟡 💬 Turn 9: edit + test   4,890 tok │ 1 │ use axum::{Router, routing::post};             │
│ ⚪ 📌 Pinned (2 items)        340 tok │ 2 │ // [Stale version from Turn 6...]              │
├───────────────────────────────────────┴────────────────────────────────────────────────────┤
│ [3] Active Lints & Shortcuts: [p] Prune  [f] Fold  [d] Dedupe  [g] Gate Tools  [Space] Pin │
└────────────────────────────────────────────────────────────────────────────────────────────┘
```

---

## 4. CLI Command Reference (`cw`)

```bash
# Start daemon (Proxy on :4401, Web UI + API on :4400)
cw serve [--port 4400] [--proxy-port 4401]

# Update the installed CLI, refresh integrations, and restart the daemon
cw update [--from-source] [--no-restart]

# Launch any harness wrapped with contextWatcher proxy + session tracking
cw wrap agy
cw wrap codex
cw wrap pi
cw wrap claude

# Open Terminal UI
cw tui [--session <id>]

# Quick CLI Inspection & Linting
cw status                        # Print token usage, KV cache hit %, and health score
cw tree [--session <id>]         # Print ASCII Radix Tree of current context
cw lint [--session <id>]         # List stale files, duplicates, and tool bloat

# Surgical Context Manipulation from CLI
cw fold <start-turn>..<end-turn> [--summary "..."]
cw prune <node-id-or-turn>
cw dedupe [--all-stale-files]
cw gate --disable-mcp <server-name>
cw pin "<constraint or file:line>"
cw replay --from-turn <n>
```
