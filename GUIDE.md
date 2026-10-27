# User Guide: `contextWatcher` (`cw`)

> **The ultra-fast, local-first context killer and KV-cache optimizer for AI coding harnesses.**  
> Cut 60%–98% of context bloat, eliminate stale file hallucinations, prevent local GPU prefill freezes, and lock in 90% cloud cache discounts.

---

## ⚡ 1. Installation

### Option A: The One-Liner (Fastest — Linux & macOS)

Run the universal installer. It detects your OS and architecture, downloads the binary, sets up `$PATH`, and runs `cw init` automatically:

```bash
curl -fsSL https://constant-labs.com/cw-install.sh | sh
```

*(Direct GitHub URL fallback):*
```bash
curl -fsSL https://raw.githubusercontent.com/GionaGranchelli/context-watcher/main/install.sh | sh
```

### Option B: Build From Source via Cargo

```bash
git clone git@github.com:GionaGranchelli/context-watcher.git
cd context-watcher
cargo install --path crates/cw-cli
cw init
```

Verify the binary is available on your PATH:
```bash
cw --version
# Output: cw 0.2.0
```

`cw init` automatically discovers your installed AI coding harnesses and sets up integrations:
* **Antigravity (AGY):** Configures the native `/context-watcher` MCP server and skill in `~/.gemini/antigravity-cli/`.
* **Claude Code:** Registers `/cw` slash commands in `~/.claude/commands/` and sets up ambient hooks.
* **Codex CLI:** Hooks into session rollouts in `~/.codex/`.
* **Pi / OMP:** Installs extension hooks in `~/.pi/agent/settings.json`.
* **Hermes Agent:** Configures state database watchers in `~/.hermes/`.
* **Aider / OpenCode:** Provides standard MCP server configs.

---

## 🧠 3. How `contextWatcher` Works (Zero-Touch Ambient Mode)

The best tool is one you never have to think about.

Once installed, `cw` runs invisibly in the background. As you code with Claude Code, Antigravity, or local models:
1. It continuously monitors context window health.
2. It detects stale duplicate file reads and runaway terminal logs.
3. It autonomously applies prefix-safe garbage collection before your prompt hits the model.
4. It drops a friendly ambient savings pulse when significant memory or time is saved:

```text
⚡ [cw] We saved you 12,966 tokens | KV Cache: 98.2% preserved | TTFT: 171s -> 1.6s (105x faster) | $0.18 saved
```

---

## 📊 4. The Daily Driver Commands

### 1. Show Your Token & Cost Savings (`cw gain`)
View your lifetime and per-harness token diet and dollar savings:
```bash
cw gain
```
```text
┌──────────────────────────────────────────────────────────────────────────────────┐
│ contextWatcher (`cw gain`) — Token, KV Cache & Cost Savings Analytics           │
├──────────────────────────────────────────────────────────────────────────────────┤
│ Sessions Analyzed   : 280        │ Active Context Volume :     19495587 tok     │
│ Active Overlay Cut  : -586165 tok │ Ready-to-Cut Bloat    :    -13893977 tok     │
│ KV Cache Hit Ratio  : 98.2     % │ Avg Context Health    :           95/100     │
│ Active Saved ($)    : $2.64      │ Cuttable Potential ($) : $      41.68/turn   │
└──────────────────────────────────────────────────────────────────────────────────┘
```

### 2. View Recent Ambient Pulses (`cw pulse`)
Check the latest background notifications delivered by the autonomous healer:
```bash
cw pulse
# or for a specific session:
cw pulse --session <session-id>
```

### 3. Check Real-Time Context & KV Cache Health (`cw status`)
Inspect the current session's token consumption, KV-cache prefix status, and VRAM footprint:
```bash
cw status
```

### 4. Real-Time Statusline HUD (`cw hud`)
Add `cw hud` to your shell prompt (Starship, bash, zsh) or `tmux` statusline (<1ms latency):
```bash
# Standard status line:
cw hud
# Output: 🟢 28.2k tok | 98.2% KV | -12.9k CW diet | 95/100 Health

# Ultra-compact (for tmux / zellij):
cw hud --compact
# Output: 🟢 28k [98% KV] (-13k) H:95

# Positional Attention ECG (detects Lost-in-the-Middle dead zones):
cw hud --ecg
```

---

## 🎛️ 5. Manual Optimization Tiers (`min`, `mid`, `max`)

You can invoke optimization at any time from your agent chat prompt or the CLI:

### Tier 1: `/cw optimize min` (Cache-Safe Boost)
* **What it does:** Replaces duplicate/superseded reads of the same file with 1-line pointer tombstones. Pins architectural rules to the tail.
* **KV-Cache Impact:** 🟢 **0% Cache Invalidation (100% Cache Safe).**
* **When to use:** Fast, active editing loops where you want zero cache-rebuild delay on Claude or local models.

### Tier 2: `/cw optimize mid` (Surgical Log Diet — Default)
* **What it does:** Strips noisy past terminal outputs (`cargo test`, compiler logs, long directory listings) and removes completed `<thinking>` monologues from earlier turns.
* **Token Reduction:** 🟡 **50% to 85% cut.**
* **When to use:** Your everyday workhorse. Reclaims tens of thousands of tokens while preserving 100% of code and conversation.

### Tier 3: `/cw optimize max` (Max Token Diet)
* **What it does:** Everything in `min` and `mid` + temporarily disables unused MCP tool schemas + folds intermediate trial-and-error debugging trajectories.
* **Token Reduction:** 🔥 **70% to 98% cut.**
* **When to use:** Context emergencies (>100k tokens), long marathons, or running local models on strict 8k/16k context limits.

---

## 🐝 6. Advanced Features: Swarm Multiplexing & PagedAttention

### Multi-Agent Swarm Prefix Multiplexing (`cw swarm`)
When multiple agents work on the same repo (e.g. Lead, Coder, Tester, Auditor):
* `cw swarm` groups them by `(workspace, model)` and extracts their shared root prefix (system prompt, tools, rules).
* Local engines (`llama.cpp`, `vLLM`) keep only **1 physical copy** of the shared prefix in GPU memory, avoiding redundant prefill.
```bash
cw swarm
```

### Engine-Level PagedAttention & Slot Bridge (`cw slot`)
For local servers (`llama-server`, `vLLM`):
* Inspect active KV slots and VRAM usage.
* Simulate outward block-aligned physical page unmapping (16-token page granularity) to drop dead context blocks without recomputing prefill.
```bash
cw slot --engine http://127.0.0.1:8095
cw slot --engine http://127.0.0.1:8095 --splice-range 2000..6800
```

---

## ❓ FAQ

**Q: Does contextWatcher change my code?**  
A: Never. `cw` only optimizes the conversational context window sent to the LLM. It preserves the newest version of all files byte-for-byte.

**Q: Does it work with cloud providers (Claude, OpenAI, Gemini)?**  
A: Yes! By keeping the prompt prefix stable, `cw` locks in the 90% prompt-caching discount on Anthropic and OpenAI, saving significant API billing.

**Q: How is this different from harness `/compact`?**  
A: Harness `/compact` calls an LLM to generate a fuzzy English summary, taking 20s and completely destroying your KV-cache prefix. `cw` runs in pure Rust in <2ms with $0.00 cost, keeping exact code and preserving your KV cache.

---

## 📄 License

Dual-licensed under [Apache-2.0](LICENSE-APACHE) and [MIT](LICENSE-MIT).
