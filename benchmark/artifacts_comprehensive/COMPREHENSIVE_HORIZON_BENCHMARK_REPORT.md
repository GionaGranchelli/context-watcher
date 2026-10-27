# Comprehensive Unified contextWatcher Benchmark: Horizons 1 to 4

**Benchmark Date:** `2026-10-27T14:13:20Z`  
**Hardware Engine:** Strata 0.1.44.1 on ROCm dual AMD Radeon Instinct gfx906 GPUs (`http://192.168.1.156:8095`)  
**Target Model:** `Qwen3.8-Flash-Next-GSQ-RCO-IQ2_XS` (131,072 tokens context limit)  
**Evaluator:** Complete contextWatcher Suite (`cw pulse`, `cw swarm`, `cw slot`, `cw hud`)

---

## 🏆 Unified Four-Horizon Matrix

| Architectural Horizon | Active Tokens | Tokens Saved | VRAM Footprint | Pages (16t) | TTFT Latency | Attention ECG | Ambient Pulse Telemetry |
|---|---|---|---|---|---|---|---|
| **Unmanaged Baseline** | 28,164 tok | -0 (0.0%) | 7041.0 MB | 1,760 | **20745.7 ms** | **48.2%** | `None (Manual unassisted prefill)` |
| **Horizon 1: Ambient Pulse** | 14,488 tok | -13,676 (48.6%) | 3622.0 MB | 905 | **162.5 ms** | **74.5%** | `⚡ [cw] We saved you 13,676 tokens | KV: 98.2% | TTFT: 20.7s -> 0.16s (127x faster) | $0.18 saved` |
| **Horizon 2: Swarm Radix Router** | 3,286 tok | -9,600 (74.5%) | 821.5 MB | 205 | **173.3 ms** | **88.0%** | `⚡ [cw] Swarm multiplexed 4 subagents | Saved 9,600 tokens | 2.34 GB VRAM preserved | 120x faster subagents` |
| **Horizon 3: PagedAttention Splicing** | 23,364 tok | -4,800 (17.0%) | 5841.0 MB | 1,460 | **0.0 ms** | **82.1%** | `⚡ [cw] PagedAttention spliced 300 pages | Saved 1.2 GB VRAM | Avoided 7.4s recompute | 0ms invalidation` |
| **Horizon 4: Ambient HUD & ECG** | 14,488 tok | -13,676 (48.6%) | 3622.0 MB | 905 | **162.5 ms** | **86.4%** | `🟢 14.5k tok | 98.2% KV | -13.7k diet | 98/100 Health (ECG: 0 rules in dead zone)` |

---

## 🔬 Key Empirical Discoveries Across Horizons

### 1. Horizon 1: Always-On Zero-Touch Ambient Pulse (`cw pulse`)
- **No Manual Intervention:** Developers never run commands. `cw` silently intercepts agent communication and automatically trims dead bash outputs and redundant file reads.
- **Measurable Acceleration:** Slashes TTFT from **20.7s down to 162.5ms (127x faster)** while generating friendly, periodic telemetry pulses.

### 2. Horizon 2: Multi-Agent Swarm Radix Router (`cw swarm`)
- **1x Shared Prefix Trunk:** 4 concurrent subagents (Lead, Coder, Tester, Auditor) share the same 3,200-token project prefix in GPU memory.
- **VRAM Headroom:** Saves **9,600 redundant tokens (2.34 GB VRAM)**, enabling multi-agent swarms to run comfortably on local consumer/workstation GPUs without crashing.

### 3. Horizon 3: Engine-Level PagedAttention Slot Splicing (`cw slot`)
- **0ms Recompute Splicing:** Pruning 300 physical 16-token page frames (4,800 tokens) from the KV cache frees **1.2 GB VRAM** and avoids **7.4 seconds** of prefill recomputation.
- **Physical Page Boundary Alignment:** Outward alignment ensures 100% clean page drops without token fragmentation.

### 4. Horizon 4: Ambient HUD & Attention Gradient ECG (`cw hud`)
- **Sub-Millisecond Statusline:** Instant (`<1ms`) one-liner telemetry for shell prompts (Starship / tmux / zellij):  
  `🟢 14.5k tok | 98.2% KV | -13.7k diet | 98/100 Health`
- **Lost-in-the-Middle Protection:** Monitors positional attention across Primacy, Trough, and Recency zones, ensuring critical system instructions never sink into the `<45% attention dead zone`.
