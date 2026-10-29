```text
               _            _ __          __   _       _               
  ___ ___  _ _| |_ _____ __| |\ \        / /__| |_ ___| |_  ___ _ _ 
 / _/ _ \| ' \  _/ -_) \ /  _| \ \  /\  / / _ \  _/ _ \ ' \/ -_) '_|
 \__\___/|_||_\__\___/_\_\___|  \_\/  \/ \___/\__\___/_||_\___|_|  
```

# contextWatcher (`cw`) — The Context Garbage Collector for AI Coding Agents

[![License: MIT / Apache-2.0](https://img.shields.io/badge/License-MIT%20%2F%20Apache--2.0-blue.svg)](LICENSE)
[![Rust: 2024](https://img.shields.io/badge/Rust-2024%20Edition-orange.svg)](https://www.rust-lang.org)
[![Agent Setup](https://img.shields.io/badge/AI%20Agent%20Runbook-AGENT__SETUP.md-brightgreen.svg)](docs/AGENT_SETUP.md)
[![How It Works](https://img.shields.io/badge/Architecture-HOW__IT__WORKS.md-blue.svg)](docs/HOW_IT_WORKS.md)
[![Compatibility](https://img.shields.io/badge/Compatibility-Contract-informational.svg)](docs/COMPATIBILITY.md)

> **Think of `cw` as `jemalloc` or V8 Garbage Collection for your LLM context window.**  
> While your coding agent works, `contextWatcher` observes the context graph, tracks KV-cache prefix boundaries, and deterministically garbage-collects **disposable transcript bloat** (duplicate file reads, old test logs, completed `<thinking>` blocks) while keeping **durable state** (fresh code, user prompts, signed reasoning, and cached prefixes) intact.

Works with **Antigravity (AGY)**, **Claude Code**, **Codex CLI**, **Pi / OMP**, **Hermes**, **OpenCode**, and **Aider**.

---

## 🤖 Let Your AI Agent Install & Configure It

Copy and paste this prompt into your coding agent (**Claude Code**, **Codex**, **Antigravity**, **OpenCode**, **Cursor**, **Aider**):

> Read `docs/AGENT_SETUP.md` (or `https://raw.githubusercontent.com/GionaGranchelli/context-watcher/main/docs/AGENT_SETUP.md`) and follow its runbook to install `contextWatcher` (`cw`), run `cw init`, start and verify the local daemon on port 4400, and check `cw status`.

---

## ⚡ Quickstart

### 1. Install & Initialize Harnesses

```bash
# Prebuilt binary installer:
curl -fsSL https://raw.githubusercontent.com/GionaGranchelli/context-watcher/main/install.sh | sh

# Or build from source:
cargo install --path crates/cw-cli
cw init
```

### 2. Start the Local Daemon & Web UI

```bash
cw serve --port 4400 --proxy-port 4401
cw auth-token
```

Open `http://127.0.0.1:4400` and paste the token from `cw auth-token` to inspect live sessions, KV-cache prefix trees, and token flamegraphs.

### 3. Run Context Garbage Collection

From your terminal—or via `/cw` skills and MCP tools inside your agent chat:

```bash
cw status                           # Check token fullness, KV-cache hit %, and health score
cw optimize --dry-run               # Preview garbage-collection candidates before applying
cw optimize -s min                  # 0-KV-break GC (uncached duplicate reads & rule reminders)
cw optimize -s clean_code_state     # Deduplicate all covered duplicate/stale file reads
cw optimize -s mid                  # Compact old completed tool outputs & unsigned thinking
cw optimize -s max --sync-back      # Full conservative diet + export compacted .jsonl copy
cw gain                             # View token and estimated cost ($) savings scoreboard
cw update                           # Self-update binary (SHA-256 verified) & refresh plugins
```

---

## 🗑️ What Gets Garbage-Collected vs. What Stays

Unlike `/compact` (which calls an LLM to rewrite your history into a lossy English summary and invalidates your KV-cache prefix), `cw` performs deterministic AST surgery in `<2ms` on CPU:

| Context Element | Disposition in `cw` |
| :--- | :--- |
| **User Prompts & Final Assistant Answers** | 🛡️ **100% Preserved** |
| **Newest / Unique File Reads** | 🛡️ **100% Preserved** |
| **Current / Unread Tail Tool Outputs & Errors** | 🛡️ **100% Preserved** (never blinds active debugging) |
| **Signed Thinking (`signature` / `thoughtSignature`)** | 🛡️ **100% Preserved** |
| **Earlier Covered Reads of the Same File** | 🗑️ **Garbage-Collected** (`[cw:shadow_deduped ...]` pointer) |
| **Verbose Old Tool Logs (`cargo test`, `git diff`)** | 🗑️ **Garbage-Collected** (`[cw:pruned ...]` 1-line stub) |
| **Completed Unsigned `<thinking>` Blocks** | 🗑️ **Garbage-Collected** (`[cw:pruned_thinking ...]`) |
| **Low-Salience Rules (<42% Attention Zone)** | 📌 **Pinned to Tail** (`<cw_pinned_context>`, 0% KV break) |

### Strategy Tiers

| Tier / ID | What It Collects | KV-Cache Impact |
| :--- | :--- | :--- |
| **`min`** (`cache_safe_boost`) | Only verified zero-break candidates (`kv_break_tokens == 0`; preserves 100% of history when cache telemetry is unverified) | **0 cached prefix tokens broken** |
| **`clean`** (`clean_code_state`) | All earlier file reads covered by a later identical read | May modify prefix |
| **`mid`** (`surgical_log_diet`) | Completed past-turn tool logs (`>100` tok) and unsigned thinking | May modify prefix |
| **`max`** (`max_token_diet`) | Combines `clean` + `mid` + low-salience rule reminders | May modify prefix |
| **`fold`** (`cw_fold_turns`) | Manual turn-range folding (`cw fold <start> <end> --summary "..."`) | Modifies prefix |

---

## 📚 Documentation

We split the documentation into focused guides so both humans and AI agents can find exactly what they need:

- 🤖 **[AI Agent Setup & Operations Runbook (`docs/AGENT_SETUP.md`)](docs/AGENT_SETUP.md)** — Step-by-step instructions written for AI agents to install, configure, start, and use `cw` via MCP and CLI.
- ⚙️ **[How It Works & Architecture (`docs/HOW_IT_WORKS.md`)](docs/HOW_IT_WORKS.md)** — Deep dive into Context Garbage Collection vs. `/compact`, the 5-stage Rust pipeline, safety guards, SQLite blob GC, and security/retention (`64 MB` migration backup cap).
- 🛠️ **[Local User Guide (`GUIDE.md`)](GUIDE.md)** — Proxy routing (`/openai`, `/anthropic`, `/gemini`), session headers, transcript import/export (`--sync-back`), and `cw purge`.
- 📋 **[Compatibility & Security Contract (`docs/COMPATIBILITY.md`)](docs/COMPATIBILITY.md)** — Per-harness and per-provider support matrix, wrapper isolation boundaries, and live provider verification tests.
- 📊 **[Reproducible Benchmarks (`benchmark/README.md`)](benchmark/README.md)** — Recorded multi-turn `.jsonl` fixtures, continuation tests, and hardware prefill benchmark scripts.
- 📝 **[Changelog (`CHANGELOG.md`)](CHANGELOG.md)** — Release history and upgrade notes.

---

## 📄 License

Dual-licensed under [Apache-2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT) at your option.
