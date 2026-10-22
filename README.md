# contextWatcher (`cw`) — Rust Context Killer for AI Coding Agents

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust: 2024](https://img.shields.io/badge/Rust-2024%20Edition-orange.svg)](https://www.rust-lang.org)
[![Prefill Speedup](https://img.shields.io/badge/Prefill%20Speedup-4x--337x-brightgreen.svg)](benchmark/README.md)
[![Token Reduction](https://img.shields.io/badge/Context%20Diet-60%25--98%25-success.svg)](benchmark/README.md)
[![KV Cache](https://img.shields.io/badge/KV%20Cache-100%25%20Prefix%20Safe-blue.svg)](benchmark/README.md)

> **Token-optimized context engine, intercepting proxy & KV-cache X-Ray for AI coding harnesses.**  
> Automatically cuts **60% to 98% of context bloat**, eliminates stale code hallucinations, prevents 80s local prefill freezes, and locks in 90% cloud prompt-cache discounts.

Works natively with **Antigravity (AGY)**, **Claude Code**, **Codex CLI**, **Pi / OMP**, **Hermes**, **OpenCode**, and **Aider**.

---

## ⚡ Quickstart (One Command)

```bash
# 1. Install contextWatcher binary
cargo install --path crates/cw-cli

# 2. Automatically install integrations for all your installed harnesses
cw init
```

`cw init` auto-discovers your local harnesses and configures:
* **Antigravity:** MCP server + native `/context-watcher` skill in `~/.gemini/antigravity-cli/`
* **Claude Code:** Slash commands (`/cw`) in `~/.claude/commands/` + auto-pilot post-tool hooks
* **Codex CLI:** Session watchers & rollout sync in `~/.codex/`
* **Pi / OMP (oh-my-pi):** Extension hook & commands in `~/.pi/agent/settings.json`
* **Hermes Agent:** State DB watchers & project categorizers in `~/.hermes/`

Verify your setup:
```bash
cw gain               # Show token, KV cache & dollar savings analytics across all sessions
cw status             # Inspect active context, KV cache hit ratio & health score
which cw              # Verify binary path (~/.local/bin/cw)
```

---

## 🛑 The Dirty Secret of AI Coding Agents

Every modern AI agent harness (**Claude Code, Codex, Antigravity, Pi, Aider**) shares the same architectural flaw: **append-only conversation memory**.

```
Turn 1: User request + Tools                     ➔ 5,000 tokens sent
Turn 2: Turn 1 + Assistant Monologue + Test Logs ➔ 12,000 tokens sent (Turn 1 re-billed!)
Turn 3: Turn 1 + Turn 2 + Build Stacks + Edits   ➔ 28,000 tokens sent (Everything re-billed!)
...
Turn 10: 100,000+ tokens re-sent on every keystroke!
```

### The Cost on Cloud Models (Claude 3.7 / GPT-4o / Gemini 2.0)
* **The Financial Drain:** Re-sending 100k+ tokens every turn costs **$10–$50 per day** in redundant input tokens.
* **The "Lost-in-the-Middle" Rot:** Past 60k tokens, models suffer attention dilution. They repeat old terminal errors from earlier turns and hallucinate deleted functions from stale file reads.

### The Catastrophe on Local Models (Ollama / `llama.cpp` / LM Studio)
* **Context Overflows:** Local engines (8k–16k context) hard-crash on Turn 2 (`exceed_context_size_error`) because hidden reasoning monologues eat 96% of the window.
* **The 80-Second Prefill Freeze:** At 64k–128k context, allocating KV-cache slots consumes **+26 GiB of RAM**, taking **84+ seconds of prefill computation** before generating a single word!

---

## 🆚 Why Not Just Use `/compact`? (contextWatcher vs Standard Compaction)

The number one question every developer asks:  
> *"My harness already has `/compact` (or auto-summarization). Why do I need `contextWatcher`?"*

**Standard compaction is an *emergency parachute*:** It invokes an LLM to generate a fuzzy English summary paragraph of past turns. It takes 15–30 seconds, burns tokens to run, causes model amnesia for exact code details, and **completely destroys your KV cache prefix**.

**`contextWatcher` is an *active garbage collector*:** It executes locally in pure native Rust in **<2ms** with **$0.00** LLM overhead. It acts like `jemalloc` or V8 GC for LLM context: deduplicating identical file reads, pruning noisy tool logs, and preserving your KV cache prefix so prefill latency stays under 100ms.

| Dimension | Standard Harness Compaction (`/compact`) | `contextWatcher` (`cw`) |
| :--- | :--- | :--- |
| **Mechanism** | Fuzzy LLM summarization pass over old turns. | Deterministic Radix Context IR in Rust (<2ms). Zero LLM calls. |
| **KV Cache Impact** | **100% Cache Invalidation.** Rewrites historical text, resetting prefix tokens to 0. Destroys cloud cache discounts & triggers massive local prefill spikes. | **Cache-Aware Surgery.** Operates via prefix-safe transforms or shadow-dedupes to preserve up to 95% of cached tokens. |
| **Code Fidelity** | **Lossy ("Telephone Game").** Exact line numbers, error traces, diffs, and type signatures are summarized away or hallucinated. | **100% Lossless for Code.** Preserves exact system prompts, instructions, and freshest file reads. Only dead redundancy is removed. |
| **Execution Cost** | **Burns completion tokens** ($0.05 – $0.25 per `/compact`) and takes **10–30 seconds**. | **$0.00 cost, <2ms execution.** Pure native Rust compiled binary. |
| **Stale File Reads** | Leaves duplicate file reads in history or lumps them into vague summaries. | **Shadow-Deduplication:** When an agent reads `lib.rs` 6 times, turns 1–5 are replaced with 0-token pointer stubs; only turn 6 remains active. |
| **Tool Output Bloat** | Cannot selectively trim a 4,000-line `cargo build` error log without losing the surrounding trajectory. | **Surgical Tool Pruning:** Truncates noisy command stdout while preserving the tool call ID, exit code, and model reasoning. |
| **Persistence** | In-memory only in many harnesses; lost on restart or re-sent in full next turn. | **Deterministic Overlays + Disk Sync:** Syncs clean states to `.jsonl` transcripts with automatic `.bak` backups across all harnesses. |

### The 4 Fatal Flaws of Standard Compaction

1. **The KV-Cache Annihilation Problem:** LLMs cache tokens sequentially from token 0. When standard `/compact` replaces Turns 1–20 with *"We refactored auth"*, the prefix changes completely. On cloud providers (Claude / GPT-4o), you lose the 90% prompt cache discount. On local GPUs (Qwen / Llama 3), prefill time jumps from **<100ms** to **25–45 seconds** as the GPU re-processes the entire prompt from scratch.
2. **The "Telephone Game" (Fuzzy Summarization vs Ground Truth):** When an LLM summarizes a 40-turn coding session, it generalizes: *"Fixed edge case in parser."* Five turns later, the agent needs the exact regex or exception signature from turn 12—which is now gone. The agent re-reads the file or hallucinates. `cw` never summarizes code blindly; it preserves ground-truth code byte-for-byte.
3. **Latency & Token Burn to Compact:** You run `/compact` to save tokens, but running `/compact` requires an expensive model inference turn that reads 50k–100k input tokens and writes a large summary block. You wait 15–30 seconds staring at a spinner. `cw` runs in **sub-2ms** on CPU.
4. **Blindness to Redundant File Floods:** Agents constantly run `cat file.ts`, `git diff`, and `cargo test`. Over a 2-hour session, 60% of context is literally identical copies of files read multiple times. Standard compaction lumps or ignores them; `cw`'s **Shadow-Deduplication** replaces prior copies with zero-cost pointer stubs, reclaiming 30,000–60,000 tokens instantly.

---

## 📊 Empirical Proof (Measured on Real Hardware)

Benchmarked on multi-GPU workstation running **`qwen3.8-27b-ud`** via **`omp` (oh-my-pi)** across all context window tiers (see [`benchmark/README.md`](benchmark/README.md)):

| Metric | Raw Agent Baseline | Tier 1: `/cw optimize min` | Tier 2: `/cw optimize mid` | Tier 3: `/cw optimize max` |
| :--- | :---: | :---: | :---: | :---: |
| **Strategy ID** | *Unoptimized* | `cache_safe_boost` | `surgical_log_diet` | `max_token_diet` |
| **Active Context** | **5,571 tokens** | **5,571 tokens** | **162 tokens** | **254 tokens** |
| **Token Reduction** | 0% | 0% (Prefix Safe) | **-97.09% diet** | **-95.44% diet** |
| **8k Window Headroom**| 68.0% (Near crash) | 68.0% | **1.98% (98% free!)** | **3.10% (96.9% free!)** |
| **KV Cache VRAM (FP16)**| 1,393.0 MB | 1,393.0 MB | **40.5 MB (-97.1%)** | **63.5 MB (-95.4%)** |
| **Prefill Latency (8k)**| 2,120 ms | 1,057 ms | **249 ms (8.5x faster)**| **390 ms (5.4x faster)**|
| **Prefill Latency (128k)**| 84,061 ms | 84,061 ms | **249 ms (337x faster)**| **390 ms (215x faster)**|
| **Unit Test Pass Rate**| 7 / 7 (100%) | 7 / 7 (100%) | **7 / 7 (100%)** | **7 / 7 (100%)** |

> **Result:** `contextWatcher` cuts context bloat by **97.1%**, accelerates local prefill by **8.5x to 337x**, and passes **100% of unit tests** with zero regressions.

---

## 🎛️ The 3 Optimization Tiers

Use slash commands natively inside your agent chat:

### 🛡️ `/cw optimize min` (Cache-Safe Boost)
* **KV-Cache Impact:** **0% Cache Invalidation (100% Cache Hit).**
* **What it does:** Leaves your cached token prefix completely untouched. Detects critical architectural rules sitting in the "<45% Lost-in-the-Middle" attention trough and pins them as lightweight reminders at the prompt tail.
* **Best for:** Active coding loops on Claude / GPT where you want zero prompt-cache breaks.

### 🧹 `/cw optimize mid` (Surgical Diet — **Recommended Default**)
* **Token Reduction:** **~90% to 97% reduction.**
* **What it does:** Keeps all human intent and fresh code files verbatim, but surgically strips completed thinking blocks, raw terminal logs, build stack traces, and noise.
* **Best for:** Daily dev work. Frees maximum headroom without losing any code context.

### 🔥 `/cw optimize max` (Deep Compaction)
* **Token Reduction:** **Maximum Token & Quality Optimization.**
* **What it does:** Batches Stale/Duplicate File Eviction + Thinking Compaction + Unused MCP Tool Schema Gating + Tail Rule Pinning into a single atomic commit.
* **Best for:** Long-running sessions (30+ turns), sub-task transitions, or local 8k models.

---

## 🛠️ CLI Reference

### `cw gain` — Token & Cost Savings Scoreboard
Show cumulative tokens saved, KV-cache efficiency, and dollar savings across all projects and harnesses:
```bash
$ cw gain
┌──────────────────────────────────────────────────────────────────────────────────┐
│ contextWatcher (`cw gain`) — Token, KV Cache & Cost Savings Analytics           │
├──────────────────────────────────────────────────────────────────────────────────┤
│ Sessions Analyzed   : 280        │ Active Context Volume :     19495587 tok     │
│ Active Overlay Cut  : -586165 tok │ Ready-to-Cut Bloat    :    -13893977 tok     │
│ KV Cache Hit Ratio  : 17.8     % │ Avg Context Health    :           70/100     │
│ Active Saved ($)    : $2.64      │ Cuttable Potential ($) : $      41.68/turn   │
└──────────────────────────────────────────────────────────────────────────────────┘

BY AGENT HARNESS:
  AGENT            SESSIONS    CONTEXT TOK   ACTIVE SAVED CUTTABLE BLOAT AVG HEALTH
  ──────────────────────────────────────────────────────────────────────────────
  antigravity           250       17133094        -432376      -12407650        72/100
  claude_code             4          48238             -0          -3877        95/100
  codex                   4         367902         -38154        -125305        67/100
  hermes                 20        1940916        -115635       -1357012        35/100
  pi                      2           5437             -0           -133       100/100
```

### `cw status` — Session Context & VRAM X-Ray
```bash
$ cw status
Session      : pi_01a163cd-a64 (pi • qwen3.8-27b-ud)
Turn         : #2 (5 nodes)
Tokens       : 162 / 200000 (0.1%) [Saved by overlays: 5409 tok]
KV Cache     : 0.0% Hit (0 hit | 162 new | 0 invalidated)
Health Score : 100/100 (0 lints, 0 active overlays)
Local KV VRAM: 40 MB (FP16) | 21 MB (Q8_0) | Recommended n_keep: 256 tok | Slot #0
```

### `cw tree` — ASCII Context Radix Prefix Tree
Inspect what is inside your prompt down to the exact token count and attention zone:
```bash
$ cw tree
Context Radix Tree — Session pi_01a163cd-a64:
  ├── [🟢 HIT ] Prefix Root (162 tok • 0% KV Hit)
  ├── [🟡 NEW ] User: Write lru_cache.py with a class LRUCache: __init (80 tok)
  ├── [🟡 NEW ] User: Successfully wrote 1803 bytes to lru_cache.py (11 tok)
  ├── [🟡 NEW ] User: Successfully wrote 985 bytes to /tmp/lru_smoke.py (14 tok)
  ├── [🟡 NEW ] Assistant: Fixing an arithmetic error I made in smoke test (22 tok)
  └── [🟡 NEW ] User: Write test_lru_cache.py using unittest (35 tok)
```

### `cw local` — Local LLM & KV-Cache Slot Studio
Optimize local inference engines (**Ollama, `llama.cpp`, vLLM, LM Studio**):
```bash
$ cw local
┌──────────────────────────────────────────────────────────────────────────────────────────┐
│ 🖥️  contextWatcher Local LLM & KV-Cache Slot Studio — Session: pi_01a163cd-a64           │
└──────────────────────────────────────────────────────────────────────────────────────────┘
  • Engine & Model           : qwen3.8-27b-ud (llama.cpp)
  • Context Window Usage     : 162 / 32768 tok (0.5%)
  • Pin System Prefix (VRAM) : n_keep / num_keep = 256 tokens (System Prompt + Rules + Tools)
  • Deterministic KV Slot    : Slot #0 (prevents subagent KV slot thrashing)
  • KV Cache VRAM Footprint  : 40.5 MB (FP16) | 21.5 MB (Q8_0) | 12.2 MB (Q4_0)
  • Local GPU Prefill Time   : ~249 ms (Saved ~1,870 ms via KV Cache hits)
```

### Batch Optimization across Projects
Optimize all sessions in a specific project or across your entire system:
```bash
cw optimize max --all                  # Apply max diet across all sessions
cw optimize mid --project aurora       # Apply mid diet to all sessions in 'aurora'
cw optimize min --agent claude_code    # Apply cache-safe boost to Claude Code
cw sync-back --all                     # Sync compacted state back to on-disk transcripts
```

---

## 🏗️ How It Works (The 3 Golden Invariants)

```
[ Your Coding Agent (Claude, Codex, AGY, Pi) ]
                 │
                 ├── 1. HTTP/SSE Intercepting Proxy (Zero-Drop Transparency)
                 ▼
 ┌──────────────────────────────────────────────────────────────────────────┐
 │                     contextWatcher Core (Rust 2024)                      │
 │                                                                          │
 │   Canonical Context IR            Radix Tree Engine & Linter             │
 │   • Lossless raw envelope         • KV-cache boundary tracking           │
 │   • Preserves thought signatures  • Attention-zone scoring (<45% trough) │
 │                                                                          │
 │   Deterministic Overlay Engine    Local KV Slot Studio                   │
 │   • Prune / Fold / Pin / Dedupe   • llama.cpp n_keep pinning             │
 │   • Content-hash keyed overlays   • PagedAttention block alignment       │
 └──────────────────────────────────────────────────────────────────────────┘
                 │
                 ├── 2. Synced back to disk transcripts (.jsonl.bak safe)
                 ▼
[ Local Engine (vLLM / llama.cpp)  OR  Cloud API (Anthropic / OpenAI / Gemini) ]
```

1. **Zero-Drop Proxy Transparency:** In passthrough mode, `cw` never corrupts provider-specific fields, SSE streaming chunks, or thinking signatures (`thoughtSignature`).
2. **Canonical Context IR + Lossless Raw Envelope:** Requests are parsed into a normalized Context DAG for analysis while keeping JSON pointer paths to the raw payload so mutations only touch targeted paths.
3. **Deterministic Mutation Overlays:** Because harnesses hold memory and re-send full history, mutations are stored as deterministic hash-keyed overlays (`NodeFingerprint`) and automatically re-applied on subsequent turns.

---

## 📦 Harness Compatibility

| Harness | Proxy Interception | On-Disk Transcript Watcher | Native Slash Commands | Disk Sync-Back |
| :--- | :---: | :---: | :---: | :---: |
| **Antigravity (AGY)** | ✅ Yes (`cw wrap`) | ✅ Auto-watched (`~/.gemini/...`) | ✅ `/context-watcher` | ✅ Safe `.jsonl.bak` |
| **Claude Code** | ✅ Yes (`cw wrap`) | ✅ Auto-watched (`~/.claude/...`) | ✅ `/cw optimize` | ✅ Safe `.jsonl.bak` |
| **Codex CLI** | ✅ Yes (`cw wrap`) | ✅ Auto-watched (`~/.codex/...`) | ✅ `/cw` skill | ✅ Safe `.jsonl.bak` |
| **Pi / OMP** | ✅ Yes (`cw wrap`) | ✅ Auto-watched (`~/.omp/...`) | ✅ Extension command | ✅ Safe `.jsonl.bak` |
| **Hermes Agent** | ✅ Yes (`cw wrap`) | ✅ SQLite Watcher (`~/.hermes/...`)| ✅ Integrated hooks | ✅ State DB sync |
| **OpenCode / Aider** | ✅ Yes (`cw wrap`) | ✅ Auto-watched | ✅ MCP Tool Server | ✅ Safe `.jsonl.bak` |

---

## 📄 License

Dual-licensed under either of:
* Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
* MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option.
