# Spec 04: Harness Integrations (AGY, Codex, Pi, OpenCode, Claude), MCP Server & IntelliJ Plugin

**Status:** Approved  
**Related Tickets:** `CW-008` – `CW-010`, `CW-034` – `CW-040`

---

## 1. Multi-Harness Adapter Architecture (`cw-harnesses`)

Different agent harnesses expose different extension points. `contextWatcher` uses a **3-pronged integration model** so every harness gets maximum visibility and manipulation capability:

| Harness | 1. HTTP/SSE Proxy | 2. Local Disk Transcript Sync | 3. Native Plugin / MCP / Skill |
| :--- | :--- | :--- | :--- |
| **Antigravity (AGY)** | Via proxy config / env | `~/.gemini/antigravity-cli/brain/<id>/.system_generated/logs/transcript.jsonl` & `transcript_full.jsonl` | Native Skill (`skills/context-watcher/SKILL.md`) + `cw mcp` server |
| **Codex CLI** | `OPENAI_BASE_URL=http://127.0.0.1:4401/openai/v1` | `~/.codex/sessions/**/*.jsonl` | Wrapper `cw wrap codex` + `cw mcp` server |
| **Pi / OpenCode / Aider** | `OPENAI_BASE_URL` / `ANTHROPIC_BASE_URL` | `.opencode/`, `.aider.chat.history.md`, Pi session store | Plugin hooks + `cw mcp` server |
| **Claude Code** | `ANTHROPIC_BASE_URL=http://127.0.0.1:4401/anthropic` | `~/.claude/projects/**/*.jsonl` | PreToolUse / PostToolUse hooks + `/cw` commands + `cw mcp` |

---

## 2. Antigravity (AGY) Deep Integration (`cw-harnesses::agy` & `plugins/agy/`)

### 2.1 Real-Time Transcript Watcher
Antigravity stores structured JSONL transcripts at:
* `<appDataDir>/brain/<conversation-id>/.system_generated/logs/transcript.jsonl`
* `<appDataDir>/brain/<conversation-id>/.system_generated/logs/transcript_full.jsonl`

The `AgyTranscriptAdapter`:
1. Watches `<appDataDir>/brain/*/.system_generated/logs/transcript.jsonl` using `notify` (inotify on Linux).
2. Parses each step (`step_index`, `source`, `type` (`USER_INPUT`, `PLANNER_RESPONSE`, tool responses), `thinking`, `tool_calls`, `media`, `truncated_fields`).
3. **Automatic Truncation Resolution:** When `truncated_fields` is present on line $L$ of `transcript.jsonl`, automatically reads line $L$ of `transcript_full.jsonl` to hydrate the exact full token payload!
4. **Subagent Graph Linking:** Detects `invoke_subagent` tool calls and links child `<conversation-id>` trees to the parent session in the Web UI DAG so you can inspect an entire multi-agent swarm in one unified graph!

### 2.2 Antigravity Skill (`plugins/agy/skills/context-watcher/SKILL.md`)
Provides an AGY Skill that teaches the agent how to query `cw status`, fold its own dead-end exploration branches via `cw mcp`, and keep its context healthy during long `/goal` runs.

---

## 3. Built-In MCP Server (`cw mcp` — `crates/cw-mcp`)

By registering `cw mcp` in any MCP-capable harness (AGY, Codex, Claude Code, OpenCode, Cursor), **the LLM agent itself gains self-awareness and surgical control over its own context window**.

### 3.1 Exposed MCP Tools

| MCP Tool Name | Description | Parameters |
| :--- | :--- | :--- |
| **`cw_context_status`** | Returns current context fullness %, KV cache hit rate, token breakdown by category, and active lint warnings (stale files, duplicate reads, bloated outputs). | `{ "session_id"?: string }` |
| **`cw_evict_stale_reads`** | Automatically shadow-deduplicates all older `read_file` outputs in the context that have been superseded by newer reads or edits. | `{ "paths"?: string[] }` |
| **`cw_fold_turns`** | Folds a completed or failed debugging/exploration turn range (`start_turn..end_turn`) into a compact summary node on the next proxy turn. | `{ "start_turn": number, "end_turn": number, "outcome_summary": string }` |
| **`cw_prune_tool_output`** | Replaces a large tool output from an earlier turn with a concise summary stub. | `{ "call_id": string, "summary": string }` |
| **`cw_pin_context`** | Pins a critical architectural invariant, requirement, or file range to the high-attention tail of the prompt. | `{ "content": string, "label": string }` |
| **`cw_gate_tools`** | Temporarily disables unused MCP tool servers for the remainder of the session to save tokens and improve tool selection accuracy. | `{ "disable_servers": string[], "enable_servers"?: string[] }` |

---

## 4. IntelliJ Platform Plugin (`ide/cw-intellij/`)

Developers spend their time inside the IDE editor while the agent runs in a terminal or side panel. The **contextWatcher IntelliJ Plugin** bridges the code editor and the agent's context window in real time.

### 4.1 Architecture
* **Language & SDK:** Kotlin, targeting IntelliJ Platform `2024.2+` (IntelliJ IDEA, RustRover, WebStorm, PyCharm, GoLand, Android Studio).
* **Transport:** Connects to `ws://127.0.0.1:4400/api/v1/ws` and subscribes to `FileContextStatusChanged` and `SessionUpdated` events for the current project's `workspace_root`.

### 4.2 Core IDE Features

1. **Editor Tab & Project Tree Context Badges (`FileStatusProvider` / `EditorNotificationProvider`):**
   Every file in the project displays a subtle live indicator showing its status inside the active LLM harness session:
   * 🟢 **`In Context (Fresh — Turn 14, 1,820 tok)`** — The agent has the exact current version of this file in its context.
   * 🟠 **`Stale in Context (Read at Turn 5 — modified 3x since)`** — Warning banner at top of editor with a 1-click action: **`[Evict Stale Read]`** or **`[Queue Fresh Diff]`**.
   * 🔁 **`Duplicate in Context (Read 3x — 5,460 tok wasted)`** — 1-click **`[Deduplicate Reads]`**.
   * 📌 **`Pinned in Context`** — File or line range is pinned to the high-attention tail.

2. **Editor Gutter Line Highlights (`LineMarkerProvider`):**
   * Shows a vertical gutter bar on the exact line ranges (`StartLine..EndLine`) that the agent actually read via `read_file`—so you immediately see if the agent only read lines `1..100` and missed lines `101..250`!

3. **Editor Context Menu Actions (`AnAction`):**
   * Select a struct/function in the editor $\rightarrow$ Right-click $\rightarrow$ **`contextWatcher: Pin Selection to LLM Context`**.
   * Right-click file $\rightarrow$ **`contextWatcher: Evict from LLM Context`**.

4. **Embedded Tool Window (`ContextWatcherToolWindowFactory`):**
   * Docked panel in IntelliJ showing:
     * Live Context Meter & KV Cache Hit Bar.
     * Active Lint Warnings with 1-click fixes.
     * Context Tree / Turn list and embedded Svelte Web UI view (via `JBCefBrowser`).
