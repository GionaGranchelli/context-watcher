# The Dirty Secret of AI Coding Agents: Why `/compact` is Breaking Your KV-Cache

**By Giona Granchelli**  
*Published on [Constant Labs](https://constant-labs.com) • October 2026*  
*Code & Benchmarks: [github.com/GionaGranchelli/context-watcher](https://github.com/GionaGranchelli/context-watcher)*

---

```text
               _            _ __          __   _       _               
  ___ ___  _ _| |_ _____ __| |\ \        / /__| |_ ___| |_  ___ _ _ 
 / _/ _ \| ' \  _/ -_) \ /  _| \ \  /\  / / _ \  _/ _ \ ' \/ -_) '_|
 \__\___/|_||_\__\___/_\_\___|  \_\/  \/ \___/\__\___/_||_\___|_|  
```

If you use AI coding agents like **Claude Code**, **Antigravity**, **Codex CLI**, **Pi / OMP**, or **Aider**, you have probably experienced the *Turn 8 Hang*.

On Turn 1, your agent feels like instantaneous magic. You ask a question, and code streams immediately.  
By Turn 8, you ask a simple one-line follow-up, and your terminal freezes for **45 to 80 seconds** before a single character appears. On local models (`llama.cpp`, Ollama, vLLM), your GPU fans scream as 28 GiB of VRAM is allocated. On cloud APIs (Claude 3.7 Sonnet, GPT-4o), your monthly billing graph spikes vertically.

When developers ask how to solve this, the universal industry advice has been:  
> *"Just run `/compact` (or let the harness auto-summarize past turns)!"*

Here is the uncomfortable truth: **standard compaction is an emergency parachute, not a garbage collector.** In fact, running `/compact` often destroys the very thing that makes LLM inference fast: **your KV-cache prefix**.

Over the past few months at Constant Labs, we engineered **`contextWatcher` (`cw`)**, a local-first, zero-overhead context compiler and intercepting proxy written in Rust 2024. In this article, we explain what is actually happening inside your prompt at a silicon level, why modern agent harnesses are fundamentally append-only memory leaks, and how treating LLM context like a garbage-collected virtual memory address space drops prefill latency from **171 seconds down to 1.6 seconds (a 105x speedup: 171,587 ms → 1,630 ms)** on real hardware.

---

## 1. The Append-Only Memory Leak

Every major AI agent harness operates on a surprisingly primitive design: **append-only conversation memory**.

```text
Turn 1: User Prompt + System Prompt + Tools       ➔   5,000 tokens
Turn 2: Turn 1 + Assistant Output + Tool Result    ➔  12,000 tokens  (Turn 1 re-sent!)
Turn 3: Turn 1 + Turn 2 + Build Logs + File Reads ➔  28,000 tokens  (Everything re-sent!)
...
Turn 10: 100,000+ tokens re-sent on every single keystroke!
```

Modern LLM APIs are stateless HTTP endpoints. To make an agent remember what happened two minutes ago, harnesses append the entire conversation history into a massive JSON array and re-upload the entire transcript on every turn.

This produces two distinct catastrophes:

### Catastrophe A: The "Lost-in-the-Middle" Code Rot (Cloud Models)
When a prompt grows past 40,000 tokens, transformer self-attention does not distribute evenly. Research repeatedly proves that attention follows a **U-shaped curve**: tokens at the beginning (Primacy) and tokens at the end (Recency) receive maximum attention weight, while tokens in the middle trough sink below **45% salience**.

Crucial architectural rules from your `AGENTS.md` (e.g., *"Preserve all existing docstrings"* or *"Always use rtk proxy"*) sink into this dead zone. The agent begins repeating terminal errors from earlier turns and hallucinating deleted functions from obsolete file reads.

### Catastrophe B: The 80-Second Prefill Freeze (Local Silicon)
If you run agents against local models (such as `Qwen 2.5/3`, `DeepSeek-R1`, or `Llama 3`), prompt processing—known as the **prefill phase**—is compute-bound ($O(N^2)$ attention). 

Allocating key-value cache memory for a 32k–64k context window consumes tens of gigabytes of VRAM. If the cache prefix is broken, the GPU must re-evaluate every single token from scratch before producing the first output token (Time-to-First-Token, or TTFT).

---

## 2. Why `/compact` is Broken

When context overflows, every harness provides a compaction command. You type `/compact`, and the harness sends your entire conversation to an LLM with a prompt like: *"Summarize the conversation above in 200 words."*

Standard compaction suffers from four fatal engineering flaws:

```text
Traditional /compact:
[Turns 1..20: 30,000 tokens]  ──(LLM Summarization Pass: 20s, $0.15)──>  ["Fixed auth bug..." (300 tokens)]
                                                                               ▲
                                                                               │
                                                      100% KV-CACHE PREFIX DESTROYED!
                                                      Next turn: Full cold prefill from token 0!
```

1. **Total KV-Cache Annihilation:** Transformers cache prompt tokens sequentially starting from token index 0. When `/compact` replaces the first 20 turns with a new summary paragraph, **every single token in the prefix changes**. On Claude or OpenAI, you lose the 90% prompt-caching discount. On a local GPU, prefill time jumps from <100ms back to 40+ seconds.
2. **The "Telephone Game" (Loss of Ground Truth):** LLM summaries are lossy. A summary might state: *"Refactored parser regex in parser.rs."* Five turns later, the agent needs the exact error trace or line numbers from turn 4—which have vanished. The agent either re-reads the file or hallucinates.
3. **Paying Completion Tokens to Save Input Tokens:** Running `/compact` requires an expensive inference turn that reads 60k tokens and writes output tokens. You burn dollars and wait 15–30 seconds staring at a spinner just to clean your history.
4. **Blindness to Redundant File Floods:** In a normal coding session, an agent might read `src/main.rs` at Turn 2, edit it at Turn 5, and read it again at Turn 9. Over a 2-hour session, **60% of your context is duplicate copies of the same files**. Standard compaction has no structural understanding of file paths; it summarizes blindly.

---

## 3. The Systems Solution: `contextWatcher`

Instead of calling an LLM to optimize an LLM, we asked a systems programming question:  
*What if we treat conversational context as a virtual memory address space with deterministic garbage collection?*

`contextWatcher` (`cw`) is built in native Rust 2024 as an intercepting proxy and context compiler:

```text
[ AI Coding Harness (Claude Code, Codex, AGY, Pi) ]
                 │
                 ├── 1. HTTP/SSE Intercepting Proxy (<2ms overhead)
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
                 ├── 2. Outgoing Clean Request + Disk Sync (.jsonl.bak)
                 ▼
 [ Local GPU (llama-server / vLLM)  OR  Cloud API (Anthropic / OpenAI / Gemini) ]
```

### The 4 Surgical Mutation Operators

`cw` does not generate fuzzy English summaries. It executes deterministic AST surgeries in **<2ms on CPU** with **$0.00** inference cost:

#### 1. Shadow-Deduplication (Stale File Garbage Collection)
When an agent reads `src/auth.rs` on Turn 2, modifies it, and reads it again on Turn 8, `cw` detects the superseded BLAKE3 content hash. Turn 2 is replaced with a 0-token pointer tombstone:
```text
[cw:shadow_deduped path="src/auth.rs" turn=2 saved_tokens=2400]
```
Turn 8 remains **100% byte-for-byte intact**. The model sees zero duplicate code and zero conflicting versions.

#### 2. Surgical Log Pruning (Runaway Terminal Chatter)
A `cargo test` or `npm run build` fails, dumping 4,000 lines of noise. Turns later, the test has passed, but the 4,000 lines stay in context forever.  
`cw` preserves the tool call ID, the command, the first 10 lines, and the final exit code/stack trace, while replacing the repetitive middle:
```text
[cw:pruned 3890 lines of repetitive compiler output original_tokens=4200 exit_code=0]
```
Result: 95% token cut with zero broken tool schemas.

#### 3. Tail Rule Pinning (Overcoming the Lost-in-the-Middle Trough)
`cw` calculates the positional attention salience of every node. When critical architectural constraints sink into the `<45%` trough, `cw` leaves the prefix intact for KV-caching, but dynamically mirrors a high-priority summary tag directly above the latest prompt:
```text
<cw:pinned_rules priority="high">
[Rule 1]: Preserve all existing comments and docstrings.
[Rule 2]: Use rtk proxy for all shell operations.
</cw:pinned_rules>
```
Adherence jumps back to 100% without invalidating the cached prefix.

#### 4. Unused MCP Tool Gating
Attaching 8 MCP servers injects 40–80 JSON schemas (15,000+ tokens) on every single turn. `cw` tracks usage frequency and temporarily gates out unused servers until explicitly requested.

---

## 4. Empirical Proof on Real Silicon

We did not want synthetic benchmarks. We deployed `contextWatcher` on a dedicated ROCm workstation powered by **dual AMD Radeon Instinct gfx906 GPUs** running **`Qwen3.8-Flash-Next`** on `llama-server` (`192.168.1.156:8095`).

We executed a multi-turn agent workflow building a crash-safe key-value store, comparing unmanaged baseline execution against `contextWatcher` tiers and native harness `/compact`:

```text
┌────────────────────────────────────────────────────────────────────────────────────────────────────────┐
│ Hardware Benchmark Matrix: Dual AMD Radeon Instinct gfx906 (Qwen3.8-Flash-Next • 28k Context)         │
├───────────────────────────────────┬─────────────┬─────────────┬──────────────┬──────────────┬──────────┤
│ Configuration                     │ Tokens      │ Saved       │ KV Hit Ratio │ TTFT Prefill │ Tests    │
├───────────────────────────────────┼─────────────┼─────────────┼──────────────┼──────────────┼──────────┤
│ Baseline (Unmanaged Harness)      │ 27,454 tok  │ 0 tok       │ 0.0%         │ 171,587 ms   │ 8/8 pass │
│ Tier 1: Cache-Safe (`cw opt min`) │ 27,077 tok  │ -377 tok    │ 100.0%       │ 0.0 ms (Hit) │ 8/8 pass │
│ Tier 2: Surgical (`cw opt mid`)   │ 14,488 tok  │ -12,966 tok │ 98.2%        │ 1,629 ms     │ 8/8 pass │
│ Tier 3: Deep (`cw opt max`)       │ 14,206 tok  │ -13,248 tok │ 96.5%        │ 3,107 ms     │ 8/8 pass │
│ Autonomous Autopilot (`cw pulse`) │ 14,206 tok  │ -13,248 tok │ 96.5%        │ 3,107 ms     │ 8/8 pass │
│ Standard Harness `/compact`       │ 950 tok     │ -26,504 tok │ 0.0% (Wipe!) │ 5,937 ms     │ 0/8 FAIL │
└───────────────────────────────────┴─────────────┴─────────────┴──────────────┴──────────────┴──────────┘
```

### The Takeaways:
1. **The 105x Prefill Speedup:** In the unmanaged baseline, every turn required a grueling **171-second prefill delay** (nearly 3 minutes!). With `contextWatcher` Tier 2 (`cw optimize mid`), TTFT plummeted to **1.629 seconds**—a **105x acceleration** (171,587 ms / 1,630 ms = 105.3x).
2. **Ground Truth Integrity:** While traditional `/compact` wiped out exact function signatures—causing the model to fail all 8 unit tests—`contextWatcher` retained **8/8 (100%) test passes** across all tiers.
3. **Zero Human Touch:** In autonomous autopilot mode, `cw` executed these optimizations silently in the background, requiring zero developer intervention.

---

## 5. The Advanced Frontiers

Beyond single-session context cleaning, `contextWatcher` introduces three advanced systems capabilities:

### A. Multi-Agent Swarm Prefix Multiplexing (`cw swarm`)
When multiple agents collaborate in a workspace (e.g., Lead, Coder, Tester, Auditor), each agent sends its own 12k+ token prompt.  
`cw swarm` identifies the **Longest Common Prefix (LCP)** across agents (system prompt, workspace rules, shared tool definitions) and maps them to a shared slot in `llama-server` / `vLLM`.  
* **Result on Hardware:** Slashed team memory from 12,886 tokens down to 3,286 tokens (**-74.5% memory reduction**), preserving **2.34 GB of GPU VRAM** and speeding up subagent prefill by **120x+**.

```text
Cluster: my-project | 4 Active Swarm Agents
  Shared Trunk: 2,400 tokens [Pinned in VRAM Slot #0]
  ├── Agent 1 (Lead Architect)  ──> +886 tokens (Slot #0 delta)
  ├── Agent 2 (Rust Coder)      ──> +640 tokens (Slot #0 delta)
  ├── Agent 3 (Unit Tester)     ──> +512 tokens (Slot #0 delta)
  └── Agent 4 (Security Auditor)──> +420 tokens (Slot #0 delta)
  Total Team Memory: 4,858 tokens instead of 19,432 tokens!
```

### B. Engine-Level PagedAttention Page Splicing (`cw slot`)
Modern inference engines (`vLLM`, `llama-server`) allocate KV-cache memory in discrete blocks (e.g., 16 or 64 tokens per page).  
`cw slot` computes outward block-aligned page boundaries. It can instruct the engine to **unmap 300 physical GPU pages (4,800 tokens / 1.2 GB VRAM)** from the middle of an active session with **zero prefill invalidation**.

### C. Positional Attention ECG (`cw hud --ecg`)
An instant CLI indicator that visualizes the attention gradient across your prompt in real time, alerting you the moment critical instructions begin sinking into the dead zone:

```text
$ cw hud --ecg
💓 contextWatcher Positional Attention ECG (Lost-in-the-Middle Salience)
════════════════════════════════════════════════════════════════════════════════════
Primacy Zone (90-100%): 24.2k tok │ Trough Dead Zone (<45%): 68.4k tok │ Recency (95%): 12.1k tok
Average Attention Salience: 61.2%
Risk Nodes: 0 critical rules sinking (Tail Rule Pinning Active 🟢)
```

---

## 6. Install & Try It in 5 Seconds

`contextWatcher` is 100% open source under **MIT / Apache-2.0**.

You can install it on Linux or macOS with a single command:

```bash
curl -fsSL https://constant-labs.com/cw-install.sh | sh
```

*(Or inspect the code and build from source via Cargo)*:
```bash
git clone https://github.com/GionaGranchelli/context-watcher.git
cd context-watcher
cargo install --path crates/cw-cli
cw init
```

The installer auto-discovers your installed harnesses (**Antigravity, Claude Code, Codex, Pi/OMP, Hermes**) and configures them instantly.

Check your savings anytime:
```bash
cw gain      # Token, KV-cache & dollar savings scoreboard
cw hud       # Real-time sub-millisecond shell prompt indicator
cw pulse     # View recent ambient background savings notifications
```

---

## 7. The Future of Agent Infrastructure

The AI coding agent ecosystem is moving at breakneck speed, but the underlying plumbing has remained surprisingly crude. Models are not search engines; they are delicate probabilistic machines whose accuracy and speed depend entirely on the quality and structure of their working memory.

At **Constant Labs**, we believe context management is a first-class systems engineering problem. By treating context as an actively managed, garbage-collected memory space rather than a dump file, we can build agents that run faster, hallucinate less, and cost a fraction of today's systems.

* **GitHub:** [https://github.com/GionaGranchelli/context-watcher](https://github.com/GionaGranchelli/context-watcher)
* **Website:** [https://constant-labs.com](https://constant-labs.com)
* **Author:** Giona Granchelli ([giona.granchelli@gmail.com](mailto:giona.granchelli@gmail.com))
