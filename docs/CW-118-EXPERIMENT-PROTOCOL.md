# CW-118 — Pre-Registered Experiment Protocol: Product Value & Economic Qualification

**Milestone:** CW-118 (Product Value & Economic Qualification)  
**Status:** Pre-Registered & Frozen Protocol  
**Date:** November 1, 2026  
**Repository:** `GionaGranchelli/context-watcher`  
**Target Matrix:** 5 Task Families $\times$ 3 Repetitions $\times$ 3 Execution Modes = 45 Sessions (15 Matched Triads)  

---

## 1. Primary Objective

Determine whether ContextWatcher provides measurable, repeatable economic and performance benefits to autonomous AI coding agents while preserving functional correctness, session reliability, and provider KV cache efficiency.

The objective is to establish empirical truth rather than market claims:
$$\text{Relative Cost Change} = \frac{\text{Cost}(M2) - \text{Cost}(M1)}{\text{Cost}(M1)}$$
Where $M1$ (ContextWatcher Passthrough) serves as the primary experimental control, and $M2$ (Cache-Safe Optimization) represents the treatment arm.

---

## 2. The Three Execution Modes

### M0 — Native Baseline
- **Configuration:** Harness executes directly against model provider without ContextWatcher proxy.
- **Role:** Reference for unproxied agent execution, raw provider latency, and baseline token usage.

### M1 — ContextWatcher Passthrough (Control Arm)
- **Configuration:** Harness routes through ContextWatcher proxy (`http://127.0.0.1:4401`) with zero active mutations (`MutationRule` empty or bypassed).
- **Role:** Primary control arm. Quantifies proxy streaming overhead (< 2 ms) and confirms payload transparency and transcript preservation (AC1-AC7).

### M2 — ContextWatcher Cache-Safe Optimized (Treatment Arm)
- **Configuration:** Harness routes through ContextWatcher proxy with strictly cache-safe optimizations:
  1. `estimated_kv_break_tokens == 0` strictly enforced.
  2. Zero mutation of cryptographic signatures, user prompts, system prompts, or active tool call pairings.
  3. Tail recency directives (`PinContext`) permitted only at clean turn boundaries.
  4. If no safe candidate exists, records an explicit **abstention** and forwards unmodified.
- **Classification:** Tail injections that influence model planning are explicitly classified as **behavioral steering**, not mechanical context reduction.

---

## 3. Workload Task Families (F1 - F5)

| Family | Code | Domain | Baseline Files | Independent Verification |
|---|:---:|---|---|---|
| Bug Fixing | **F1** | Arithmetic & fallbacks | `math_ops.py`, `test_math_ops.py` | `python3 -m unittest test_math_ops.py` |
| Refactoring | **F2** | String formatting duplication | `string_utils.py`, `test_string_utils.py` | `python3 -m unittest test_string_utils.py` |
| Feature Impl | **F3** | Cache backing store | `store.py`, `cache.py`, `test_cache_system.py` | `python3 -m unittest test_cache_system.py` |
| TDD | **F4** | Registration validator | `validator.py`, `test_validator.py` | `python3 -m unittest test_validator.py` |
| Investigation | **F5** | Broken pipeline recovery | `pipeline.py`, `test_pipeline.py` | `python3 -m unittest test_pipeline.py` |

---

## 4. Balanced Mode-Order Rotation (Latin Square)

To prevent systematic time-of-day provider drift or diurnal cache clearing from biasing results, each `(task_family, repetition)` block rotates mode execution order using Latin square counterbalancing:

| Block Repetition | Execution Order | Position 0 | Position 1 | Position 2 |
|:---:|:---:|:---:|:---:|:---:|
| **Repetition 0** | `[M0, M1, M2]` | M0 | M1 | M2 |
| **Repetition 1** | `[M1, M2, M0]` | M1 | M2 | M0 |
| **Repetition 2** | `[M2, M0, M1]` | M2 | M0 | M1 |

Across 3 repetitions, each mode occupies every position (0, 1, 2) exactly once per task family.

---

## 5. Economic & Statistical Methodology

1. **Unit of Analysis:** Total provider cost per task attempt.
2. **Failure Accounting:** Never drop or censor failed runs from primary comparison. Report both:
   - *All-attempt costs:* Incurred spend regardless of completion outcome.
   - *Success-conditional costs:* Spend for runs satisfying independent acceptance tests.
3. **Paired Relative Cost Change:**
   $$\Delta_{\text{rel}} = \frac{C_{M2} - C_{M1}}{C_{M1}}$$
   Where $C_{M1} > 0$. If $C_{M1} = 0$, record `valid_division = false` without dividing by zero.
4. **Bootstrap Confidence Intervals:** 1,000 resamples computing 95% non-parametric confidence intervals.
5. **Causal Decomposition:**
   - *Mechanical Effect:* Direct prompt reduction on identical request structures.
   - *Cache Effect:* Prompt cache reuse differences ($\text{Hit Ratio}$).
   - *Behavioral Effect:* Reductions in request/tool iterations resulting from steering prompts.
   - *Proxy Effect:* Processing latency and overhead introduced by proxy.

---

## 6. Pre-Registered Decision Criteria (Frozen Thresholds)

To qualify ContextWatcher cache-safe optimization for user pilot (`SUPPORTED_FOR_PILOT`) during authorized live qualification (Gate B):
1. **Zero Correctness Regression:** $\text{PassRate}(M2) \ge \text{PassRate}(M1)$.
2. **Positive Economic Direction:** $\text{Mean}(\Delta_{\text{rel}}) < 0.0$.
3. **Substantial Median Savings:** $\text{Median}(\Delta_{\text{rel}}) \le -10.0\%$.
4. **Task Generalizability:** Demonstrated savings across $\ge 2$ distinct task families.
5. **Prefix Preservation:** 0 unexpected provider cache invalidations ($\text{kv\_break} = 0$).
6. **Reliability Preservation:** 0 proxy-induced aborts or session crashes.
7. **Documented Intervention:** 100% of mutations trace to safe candidates in the overlay engine.

If any criterion fails or evidence is insufficient, outcome defaults to `INCONCLUSIVE` or `REGRESSION`.
During Gate A (offline engineering qualification), criteria 1, 5, 6, 7 are verified on deterministic infrastructure; criteria 2, 3, 4 are marked `NOT_EVALUATED` because economic savings on LLMs must be measured on live autonomous agents, not assumed or pre-programmed.
Until Gate B, the product hypothesis is reported as `NOT_EVALUATED`, never `SUPPORTED_FOR_PILOT`.

---

## 7. Gate A vs. Gate B Separation

- **Gate A (Engineering Qualification):** 100% local execution using deterministic fixtures, agent tool execution, and mock providers. Verifies that the experimental pipeline, policy gating, and telemetry reconciliation function without defect. Zero paid provider spend.
- **Gate B (Authorized Live Qualification):** Strictly opt-in. Requires explicit command-line authorization (`--authorize-spend <ENVELOPE>`). Without explicit authorization, live execution halts immediately with status `BLOCKED_SPEND_AUTHORIZATION`. Fails closed if live provider environment is unconfigured (`BLOCKED_LIVE_EXECUTION_UNAVAILABLE`).
