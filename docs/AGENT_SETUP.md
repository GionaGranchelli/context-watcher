# AI Agent Setup & Operations Runbook (`contextWatcher` / `cw`)

> **Audience:** This document is written specifically for **AI coding agents** (Claude Code, Codex CLI, Antigravity/AGY, OpenCode, Pi/OMP, Hermes, Aider, Cursor, Windsurf).  
> If a user asked you to install, configure, start, or operate `contextWatcher` (`cw`), follow the steps below in order without asking unnecessary questions.

---

## 1. What `contextWatcher` (`cw`) Is

`contextWatcher` (`cw`) is a **local-first Context Garbage Collector and KV-Cache Optimizer** for LLM coding harnesses.
Instead of using lossy LLM summarization (`/compact`), `cw` deterministically collects disposable context garbage in `<2ms`:
- **Duplicate/Stale File Reads:** Replaces older reads of the same file with a 1-line pointer when a newer read covers the same contents.
- **Completed Tool Output & Reasoning Noise:** Compacts verbose old command stdout (`cargo test`, `npm test`, `git diff`) and unsigned `<thinking>` blocks from completed past turns while keeping current/unread tool outputs intact.
- **Lost-in-the-Middle Rule Recovery:** Appends concise `<cw_pinned_context>` reminders at the prompt tail without breaking the cached prefix.

---

## 2. Step-by-Step Installation & Setup

### Step 1: Check if `cw` is Already Installed

Run:
```bash
~/.local/bin/cw --version || cw --version
```

- **If `cw` is already installed** and you only need to update it to the latest release:
  ```bash
  ~/.local/bin/cw update
  ```
- **If `cw` is NOT installed**, install it using **one** of these methods:
  1. **Release Installer Script (Fastest, prebuilt binary):**
     ```bash
     curl -fsSL https://raw.githubusercontent.com/GionaGranchelli/context-watcher/main/install.sh | sh
     ```
  2. **Build from Source (if inside a checkout of `contextWatcher` or Rust toolchain is preferred):**
     ```bash
     cargo install --path crates/cw-cli
     ```

Ensure `~/.local/bin` is on `PATH` (or invoke `~/.local/bin/cw` directly).

---

### Step 2: Register Harness Integrations (`cw init`)

Run:
```bash
~/.local/bin/cw init
```

`cw init` automatically detects installed agent harnesses on the machine and configures:
- **Antigravity (AGY):** Registers the `contextwatcher` MCP server and `/cw` & `/context-watcher` skills in `~/.gemini/antigravity-cli/`.
- **Claude Code:** Registers the user-scope `contextwatcher` MCP server in `~/.claude.json` and `/cw` slash commands in `~/.claude/commands/`.
- **Codex CLI:** Adds `[mcp_servers.contextwatcher]` to `~/.codex/config.toml` and installs skills in `~/.codex/skills/`.
- **OpenCode:** Registers the `contextwatcher` MCP server in `~/.config/opencode/config.json`.
- **Pi / OMP:** Links the `contextwatcher` extension and registers `/cw` commands.
- **Hermes:** Installs and enables the `context-watcher` plugin in `~/.hermes/`.
- **Systemd (Linux):** Writes `~/.config/systemd/user/contextwatcher.service`.

---

### Step 3: Start & Verify the Local Daemon

Check whether the `cw` daemon is already running on `http://127.0.0.1:4400`:

```bash
curl -fsS http://127.0.0.1:4400/api/v1/health
```

If it is **not** running, start it:

- **Option A — Systemd User Service (Linux, persistent):**
  ```bash
  systemctl --user daemon-reload
  systemctl --user enable --now contextwatcher.service
  ```
- **Option B — Background Daemon Process (macOS / Linux / Docker containers without systemd):**
  ```bash
  nohup ~/.local/bin/cw serve --port 4400 --proxy-port 4401 >/tmp/cw-daemon.log 2>&1 &
  ```

Verify health again:
```bash
curl -fsS http://127.0.0.1:4400/api/v1/health
```
Expected response includes `"status":"ok"`.

---

## 3. How to Connect Your Harness

### A. Via MCP Server (Recommended for Self-Inspection & Compaction)

If your harness supports MCP, point it at:
- **Command:** `~/.local/bin/cw`
- **Args:** `["mcp"]`

Example `mcp.json` / `config.json` snippet:
```json
{
  "mcpServers": {
    "contextwatcher": {
      "command": "/home/<user>/.local/bin/cw",
      "args": ["mcp"]
    }
  }
}
```

### B. Via Loopback Proxy (`cw wrap` or Base URL Env Vars)

To route LLM API traffic through `cw`'s loopback proxy (`127.0.0.1:4401`):
- **Codex CLI (authenticated overlay session):**
  ```bash
  cw wrap --session-id my_session codex
  ```
- **Hermes (DeepSeek route with authenticated overlay plugin):**
  ```bash
  cw wrap hermes chat
  ```
- **Observation Proxy Endpoints (Port `4401`):**
  - OpenAI Chat / Responses: `http://127.0.0.1:4401/openai/v1`
  - Anthropic Messages: `http://127.0.0.1:4401/anthropic`
  - Gemini: `http://127.0.0.1:4401/gemini`

*(See [`docs/COMPATIBILITY.md`](COMPATIBILITY.md) for per-harness authenticated session header and wrapper isolation details.)*

---

## 4. How Agents Should Use `cw` During a Coding Session

### Available MCP Tools (`cw mcp`)

All MCP tools accept an optional `session_id`. If omitted, `cw mcp` automatically resolves the session from `CW_SESSION_ID` or an unambiguous workspace match for the current working directory (and returns candidate IDs if multiple sessions match).

1. **`cw_context_status`**
   - Call this to inspect current token usage, KV-cache hit ratio, health score (`0..100`), and active lint findings.
2. **`cw_optimize_strategy`**
   - Arguments:
     - `strategy_id`: `"min"` | `"clean_code_state"` | `"mid"` | `"max"`
     - `export_compacted`: `true` (optional; exports a separate `.cw-compacted.jsonl` copy without modifying the live transcript in-place)
   - Strategy guide:
     - `"min"` (`cache_safe_boost`): Zero-KV-break garbage collection (`kv_break_tokens == 0`: uncached duplicate reads, uncached tail bloat, and low-salience rule reminders).
     - `"clean_code_state"` (`clean`): Shadow-deduplicates all covered duplicate/stale file reads across the session.
     - `"mid"` (`surgical_log_diet`): Compacts old completed tool outputs (`>100` tokens) and unsigned `<thinking>` blocks.
     - `"max"` (`max_token_diet`): Combines `clean_code_state` + `surgical_log_diet` + rule reminders.
3. **`cw_evict_stale_reads`**
   - Call after editing files you read earlier to replace superseded file-read blocks with 1-line pointers.
4. **`cw_prune_tool_output`**
   - Stubs a specific verbose tool output from an earlier turn by `call_id` or `target_fingerprint`.
5. **`cw_fold_turns`**
   - Collapses a completed multi-turn debugging loop (`from_turn`..`to_turn`) into a concise, verified `summary` string that you supply.
6. **`cw_pin_context` / `cw_unpin_context`**
   - Pins or removes high-priority constraints (`<cw_pinned_context>`) at the tail of the prompt.

### CLI Fallback Commands (If MCP Tools Are Not Loaded Yet)

If `cw init` was just run and your current agent session has not reloaded its MCP tool list yet, invoke the CLI directly via shell:

```bash
~/.local/bin/cw status                           # Check context health & token usage
~/.local/bin/cw optimize -s min --sync-back      # 0-KV-break garbage collection
~/.local/bin/cw optimize -s mid --sync-back      # Compact old tool logs & thinking blocks
~/.local/bin/cw optimize -s max --sync-back      # Full conservative garbage collection
~/.local/bin/cw gain                             # Show token & cost ($) savings scoreboard
```
