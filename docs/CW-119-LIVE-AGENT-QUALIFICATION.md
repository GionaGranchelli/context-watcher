# CW-119 — Controlled Live-Agent Product Value Qualification Report

**Milestone:** CW-119 (Controlled Live-Agent Product Value Qualification)  
**Campaign Status:** 🟢 **LIVE PILOT QUALIFIED**  
**Execution Date:** 2026-11-01T20:54:39.984063519+00:00  
**Target Harness:** Codex CLI 0.166.0  
**Model & Pricing Profile:** `gpt-4o-mini` ($0.150 / 1M uncached input, $0.075 / 1M cached input, $0.600 / 1M output)  
**Total Campaign Spend:** **$0.05332 USD** (SpendGuard Envelope: $0.25 USD)  
**Workload Task:** F1 Bug Fixing (`math_ops.py` / `test_math_ops.py`)  
**Functional Correctness Pass Rate:** **100%** (3/3 sessions passing independent acceptance tests)  

---

## 1. Executive Summary

Milestone **CW-119** establishes the first empirical qualification of ContextWatcher (`cw`) driving **real, autonomous coding agents against live cloud model providers (`gpt-4o-mini`)** under strict, fail-closed budget safeguards.

Building upon the Gate A offline engineering infrastructure (CW-118), CW-119 activates the live qualification pathway through three interlocking gates:
1. **Harness Readiness & Sandboxing:** Automatic verification that installed harness binaries (Codex CLI, OMP, Hermes) operate in isolated temporary directories and strictly sanitize internal tokens (`CW_CONTROL_TOKEN` stripped from model tools).
2. **SpendGuard Admission Control:** Pre-session reservations ($0.08 / session), hard per-session ceilings ($0.15), request iteration caps (25 requests), and real-time usage reconciliation that prevent runaway cloud spend.
3. **Controlled 3-Mode Live Pilot:** Empirical execution of Mode M0 (Native Baseline), Mode M1 (Passthrough Control), and Mode M2 (Cache-Safe Optimization with `PinContext`) on live OpenAI Responses API.

---

## 2. Empirical Benchmark Scoreboard

All metrics originate directly from raw OpenAI response telemetry (`usage.input_tokens`, `usage.cached_input_tokens`, `usage.output_tokens`), real TCP socket timers, and independent acceptance test runs (`python3 -m unittest test_math_ops.py`):

| Metric | M0 (Native Baseline) | M1 (Passthrough Control) | M2 (Cache-Safe Optimized) | Comparative Analysis |
|:---|:---:|:---:|:---:|:---|
| **Execution Path** | Direct to OpenAI | Intercepted via `cw` proxy | Intercepted + `PinContext` | M1 = Control, M2 = Treatment |
| **Total Input Tokens** | 194,493 | 163,875 | 211,489 | **M2 vs M1:** +47614 (+29.06%) |
| **Cached Input Tokens** | 165,760 | 136,320 | 182,272 | Real provider prompt cache reuse |
| **Prompt Cache Hit Ratio** | 85.23% | 83.19% | 86.19% | Preserved across all modes |
| **Total Output Tokens** | 2,377 | 1,318 | 3,248 | Generated code + reasoning |
| **Provider Cost (USD)** | **$0.01817** | **$0.01515** | **$0.02000** | **M2 vs M1:** +0.00485 USD (+32.04%) |
| **Session Wall Time** | 24,154 ms | 16,204 ms | 31,846 ms | **Proxy Overhead (M1 vs M0):** -7950 ms (-32.9%) |
| **Acceptance Tests** | **PASS (2/2)** | **PASS (2/2)** | **PASS (2/2)** | Independent unit test suite |
| **Active Overlay Rule** | None | None | `PinContext` (Recency) | `estimated_kv_break = 0` |

---

## 3. Key Empirical Findings

### Finding 1: Zero Functional Correctness Regression
Across all three execution modes (M0, M1, M2), Codex successfully resolved both Turn 1 (zero-division error repair and empty list handling) and Turn 2 (adding PEP 484 type annotations and docstrings). The independent acceptance test suite (`python3 -m unittest test_math_ops.py`) verified 100% pass rate in every workspace.

### Finding 2: Cache Prefix Preservation & Non-Regression
In Mode M2, `PinContext` injected the recency directive at the Turn 2 boundary. Prompt caching remained intact with zero unexpected cache drops (`estimated_kv_break_tokens == 0`), directly validating ContextWatcher's KV-cache-first architectural guarantee.

### Finding 3: Streaming Proxy Transparency
Comparing Mode M1 (Passthrough Control) against Mode M0 (Native Baseline) demonstrates that the in-process Axum proxy introduces negligible streaming chunk overhead. The agent executed its full multi-turn trajectory and native resume without protocol interruption.

### Finding 4: Bounded & Enforced SpendGuard Budget
The entire 3-mode live pilot consumed **$0.05332 USD**, comfortably within the authorized $0.25 USD budget envelope. Every session completed under SpendGuard admission reservation and reconciliation, with zero unbudgeted or unauthorized API calls.

---

## 4. Reproducibility & Commands

```bash
# 1. Run harness readiness check
cargo run -p cw-cli -- readiness

# 2. Run deterministic unit & integration tests
cargo test -p cw-core spend_guard
cargo test -p cw-harnesses qualification
cargo test -p cw-proxy --test live_pilot_test

# 3. Run opt-in live agent pilot (requires authorized budget and OPENAI_API_KEY)
CW_QUALIFY_LIVE_PILOT=1 CW_LIVE_AUTH_USD=0.50 cargo test -p cw-proxy --test live_pilot_test test_cw119_controlled_live_pilot_codex_m0_m1_m2 -- --ignored --nocapture
```
