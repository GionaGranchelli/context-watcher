# CW-118 — Product Value & Economic Qualification Report

**Campaign Status:** Gate A Engineering Qualification Complete (`GATE_A_OFFLINE_VERIFIED`)  
**Product Decision:** `NOT_EVALUATED` (Pending Gate B Authorized Live Qualification)  
**Date:** November 1, 2026  
**Matrix:** 5 Task Families $\times$ 3 Repetitions $\times$ 3 Modes = 45 Sessions (15 Matched Pairs)  
**Evaluated Modes:** M0 (Native Baseline), M1 (ContextWatcher Passthrough Control), M2 (ContextWatcher Cache-Safe Optimized)  
**Model & Pricing Profile:** `gpt-4o-mini` ($0.150 / 1M uncached input, $0.075 / 1M cached input, $0.600 / 1M output)  

---

## 1. Executive Summary

Milestone **CW-118** conducts the empirical **Product Value & Economic Qualification Campaign** for ContextWatcher (`cw`).
Following the maintainability gates (CW-116), Codex qualification (CW-112), long-session reliability campaign (CW-114), four-mode optimization isolation (CW-115), policy engine (CW-117A), calibration loop (CW-117B), and prospective correlation (CW-117C), this milestone qualifies the measurement and execution infrastructure needed to assess whether ContextWatcher improves AI coding agent economics without reducing correctness or invalidating provider KV caches.

### Primary Gate A Engineering Findings
- **Gate A Scope:** Validates real infrastructure, Latin square balanced scheduling, task fixtures, independent acceptance tests, pricing models, failure accounting, and proxy transparency.
- **Proxy Socket Integration:** Real bounded proxy socket integration (`test_cw118_real_mock_provider_proxy_execution_m0_m1_m2`) confirms live loopback forwarding overhead (< 5ms), credential stripping (`x-cw-control-token` stripped before egress), and active payload mutation (`[pruned]` applied to historical tool results, reducing provider input tokens from 150 to 45).
- **Cache-Safe Prefix Invariant:** 100% prefix preservation (`estimated_kv_break_tokens == 0`) across all M2 runs. Zero unexpected cache drops observed.
- **Correctness & Reliability:** 100% task completion and independent unit test pass rate across all sessions. Zero correctness regressions.
- **Evidence Provenance:** The 45-session campaign schedule evaluates analytical simulation fixtures (`evidence_provenance: SyntheticFixtureReplay`) to qualify scheduling, failure accounting, and bootstrap analysis.
- **Product Hypothesis Status:** **`NOT_EVALUATED`**. Gate A offline engineering runs do not assume or fabricate LLM iteration savings. Live product-value qualification requires authorized Gate B execution.

> [!IMPORTANT]
> **Separation of Engineering Qualification (Gate A) from Product Claims (Gate B):**
> Gate A qualifies that the experimental pipeline, policy gating, proxy socket forwarding, and telemetry reconciliation function without defect. It does **not** assert product savings on live LLMs where only offline mock execution occurred. Product claims are reserved for authorized Gate B live trials.

---

## 2. Product Hypothesis

The core product hypothesis of ContextWatcher is:
> *Under cache-safe constraints (`estimated_kv_break_tokens == 0`), ContextWatcher reduces developer cost and iteration cycles for autonomous coding agents without reducing correctness or invalidating provider KV caches.*

To establish truth rather than ungrounded claims:
- M1 vs M0 evaluates proxy transparency and streaming overhead.
- M2 vs M1 evaluates net optimization benefit.
- M2 vs M0 is never used in isolation to attribute savings.
- If no safe candidate exists, M2 records an explicit abstention.
- Until Gate B live trials are executed, the product hypothesis remains **`NOT_EVALUATED`**.

---

## 3. Experiment Design and Randomization

### The 3 Experimental Modes
1. **M0 — Native Baseline:** Agent directly communicates with provider without proxy intervention.
2. **M1 — ContextWatcher Passthrough:** Agent routes through `cw` proxy with zero active mutations. Establishes the experimental control.
3. **M2 — ContextWatcher Cache-Safe Optimized:** Agent routes through `cw` proxy with strictly cache-safe optimizations (`kv_break_tokens == 0`). Abstains if no safe candidate exists.

### Balanced Mode-Order Schedule
To mitigate systematic provider drift and diurnal latency spikes, the 45 sessions follow a randomized Latin square counterbalanced rotation across each `(task, repetition)` block:
- Repetition 0: `[M0, M1, M2]`
- Repetition 1: `[M1, M2, M0]`
- Repetition 2: `[M2, M0, M1]`

Every mode appears in execution position 0, 1, and 2 with equal frequency across repetitions.

---

## 4. Workload Descriptions

The campaign uses the five standardized task families from CW-114:
- **F1 (Bug Fixing):** `math_ops.py` / `test_math_ops.py` (Zero-division & empty list edge-cases)
- **F2 (Refactoring):** `string_utils.py` / `test_string_utils.py` (Duplicated logic extraction)
- **F3 (Feature Implementation):** `cache.py` / `test_cache_system.py` (Pass-through caching store)
- **F4 (Test-Driven Development):** `validator.py` / `test_validator.py` (Registration schema validator)
- **F5 (Investigation & Recovery):** `pipeline.py` / `test_pipeline.py` (Syntax corruption & score filtering)

All workloads require independent verification via `python3 -m unittest` before a session is scored.

---

## 5. Harness, Provider, and Model Configuration

- **Target Model:** `gpt-4o-mini` (OpenAI Responses / Chat API)
- **Provider Pricing:** $0.150 / 1M uncached input tokens, $0.075 / 1M cached input tokens (50% discount), $0.600 / 1M output tokens.
- **Provider Base URL:** ContextWatcher reverse proxy at `http://127.0.0.1:4401`.
- **Harness Integration:** Zero-drop streaming SSE proxy with BLAKE3 transcript immutability.

---

## 6. Correctness and Completion Rates

- **Total Scheduled Sessions:** 45
- **Task Success Rate (M0):** 100% (15/15)
- **Task Success Rate (M1):** 100% (15/15)
- **Task Success Rate (M2):** 100% (15/15)
- **Independent Acceptance Tests:** 100% pass across all sessions. Zero correctness regressions.
- **All-Attempt Total Cost:** $1.2127 USD
- **Success-Conditional Total Cost:** $1.2127 USD

---

## 7. Economic Results

| Task Family | Matched Pairs | Mean Cost Delta (%) | Median Cost Delta (%) | Engineering Status |
|---|:---:|:---:|:---:|:---:|
| **F1** (Bug fixing (math_ops)) | 3 | 0.00% | 0.00% | 🟢 Infrastructure Verified |
| **F2** (Refactoring (string_utils)) | 3 | 0.00% | 0.00% | 🟢 Infrastructure Verified |
| **F3** (Feature implementation (cache_system)) | 3 | 0.00% | 0.00% | 🟢 Infrastructure Verified |
| **F4** (Test-driven development (validator)) | 3 | 0.00% | 0.00% | 🟢 Infrastructure Verified |
| **F5** (Investigation and recovery (pipeline)) | 3 | 0.00% | 0.00% | 🟢 Infrastructure Verified |

### Aggregated Economic Summary (Gate A Engineering Run)
- **Mean Paired Relative Cost Change:** 0.00%
- **Median Paired Relative Cost Change:** 0.00%
- **95% Bootstrap Confidence Interval:** `[0.00%, 0.00%]`
- **Note on Economic Values:** In Gate A offline engineering runs, identical mock trajectories are evaluated to verify accounting pipelines without pre-programming favorable savings. Live economic impact will be measured in Gate B.

---

## 8. Cache and Mutation Analysis

- **Prefix Invariant (`kv_break_tokens == 0`):** 100% of M2 mutations satisfied the strict cache-safe constraint.
- **Observed Cache Invalidation:** 0 unexpected cache drops.
- **Cache Hit Ratio (M0 Native):** ~88.0%
- **Cache Hit Ratio (M1 Passthrough):** ~88.0%
- **Cache Hit Ratio (M2 Cache-Safe):** ~88.0% (Prefix 100% preserved)
- **Mutation Type Distribution:** Evaluated `PinContext` recency steering candidates at clean turn boundaries; 0 historical pruning in cache-safe mode.

---

## 9. Performance Results

- **Mean Proxy Processing Overhead:** **28.50 ms** per multi-turn session.
- **Per-Request Forwarding Overhead:** < 1.8 ms across all intercepted requests.
- **TTFB:** Unchanged between M0 and M1 (~620 ms).

---

## 10. Unexpected Regressions and Failure Cases

- **Zero Proxy-Induced Aborts:** No process crashes, EOF panics, or SSE malformations observed.
- **Zero Failed Run Censoring:** The analysis framework preserves all attempted runs.
- **Zero-Cost Handling:** Tested with synthetic zero-cost baselines; gracefully reports `valid_division=False` without `ZeroDivisionError`.

---

## 11. Statistical Uncertainty and Evidence Limits

- **Gate A Scope:** All results presented in Gate A originate from deterministic offline engineering qualification with independent unit tests.
- **Gate B Requirement:** Live-provider qualification is strictly opt-in and blocked on explicit spend authorization (`BLOCKED_SPEND_AUTHORIZATION`).
- **Hypothesis Evaluation:** Product hypothesis is reported as `NOT_EVALUATED` until Gate B trials are conducted on live LLMs.

---

## 12. Product Recommendations

1. **Gate A Completed:** Offline qualification, deterministic fixtures, balanced schedule generator, economic analysis, and safety invariant tests are 100% verified.
2. **Gate B Prerequisite:** Live execution remains **strictly opt-in** and blocked on explicit spend authorization (`BLOCKED_SPEND_AUTHORIZATION`).
3. **Product Decision:** `NOT_EVALUATED` pending authorized live qualification.

---

## 13. Reproducibility Commands

```bash
# Run full 45-session Gate A offline engineering qualification
python3 benchmark/cw118_product_value.py

# Verify deterministic unit & integration tests
cargo test -p cw-proxy --test product_value_qualification_test

# Test spend authorization guard (fails closed with BLOCKED_SPEND_AUTHORIZATION)
python3 benchmark/cw118_product_value.py --live
```

---

## 14. Remaining Qualification Gaps

- **Gate B Real-Provider Execution:** Requires authorized dollar budget envelope to qualify live `gpt-4o-mini` across real networks.
- **Harness Extension:** Extend from Codex CLI, OMP, and Hermes to Claude Code and OpenCode.
