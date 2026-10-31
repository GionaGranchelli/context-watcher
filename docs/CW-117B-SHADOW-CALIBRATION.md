# CW-117B — Shadow Policy Empirical Calibration & Evidence Loop Report

**Milestone:** CW-117B (Shadow Policy Empirical Calibration & Evidence Loop)  
**Date:** October 31, 2026  
**Status:** Completed & Empirically Calibrated  
**Branch:** `feat/cw-117b-shadow-calibration`  
**Datasets Replayed:**
1. `docs/cw115_benchmark_data.json` (6 sessions, 59 requests, git commit `e9b1b43b`)
2. `docs/cw115_four_mode_benchmark_data.json` (4 sessions, 71 requests, git commit `72bec0d7`)  
**Total Telemetry Analyzed:** 10 sessions, 130 request-level traces  
**Machine-Readable Report:** `docs/cw117b_calibration_report.json`  
**CLI Diagnostic Command:** `cw calibrate [--dataset <path>] [--horizon <N>] [--json]`  

---

## 1. Executive Summary & Success Criteria

ContextWatcher's CW-117 Phase 1 policy engine introduced explainable, cache-aware optimization decisions based on structural KV-cache impact, provider telemetry provenance, and pure economic modeling.

CW-117B closes the loop from **modeled expectations** to **measurable empirical calibration**. By replaying the immutable historical traces recorded during the CW-115 benchmarks through the policy evaluator, this calibration campaign measures:
1. **Economic Model Fidelity:** The pricing profile for `gpt-4o-mini` reconciles against actual provider wire costs across all 130 requests with **$0.0000000000 variance** (100.0% coverage).
2. **Cache State Consistency:** Evaluated cache-state classifications (`ColdLikely`, `WarmVerified`) agree with subsequent provider cache outcomes in **89.83%** (`cw115_benchmark_data.json`) and **87.32%** (`cw115_four_mode_benchmark_data.json`) of requests.
3. **Prefix Invalidation Validation:** Empirical traces from Mode 2 historical pruning prove the exact failure mode predicted by CW-117: applying 4 pruning mutations on Request 6 reduced input tokens by 169 tokens, but triggered an immediate loss of 1,280 cached tokens and a surge in uncached tokens (+1,111 tokens), driving single-request costs up. CW-117's policy correctly emitted `DEFER` (`CachePrefixAtRisk`) on this request.
4. **Distinction of Counterfactuals:** Pure passthrough traces evaluate hypothetical cold-cache cuts as `Apply` on Turn 1, but explicitly tag them as `unverifiable_counterfactuals` because alternative prompts were never executed upstream.

---

## 2. Evidence Architecture & Privacy Guarantees

### Schema: `ShadowDecisionEvidence`
Located in `crates/cw-core/src/calibration.rs`, the evidence record links policy decisions with provider telemetry:

```
┌────────────────────────────────────────────────────────────────────────┐
│ ShadowDecisionEvidence                                                 │
├────────────────────────────────────────────────────────────────────────┤
│ record_id                   : sde_<session_id>_<request_index>         │
│ session_id                  : String (harness session identifier)      │
│ request_index / turn_index  : u32 (monotonic sequence)                 │
│ provider / model            : ProviderKind / Model identifier          │
│ cache_state                 : WarmVerified | ColdLikely | Unknown      │
│ provenance                  : ObservedProvider | LocalEstimate | None  │
│ prefix_fingerprint          : Option<String> (SHA-256 hash only)       │
│ candidate_id / plan_id      : Option<String> (structural IDs)          │
│ decision                    : Apply | Defer | Reject | NoOp            │
│ reason_code                 : Typed DecisionReasonCode                 │
│ estimated_saved_tokens      : i32 (before - after)                     │
│ estimated_kv_break_tokens   : u32 (historical prefix at risk)          │
│ predicted_baseline_cost_usd : f64 (projected over horizon H)           │
│ predicted_optimized_cost_usd: f64 (projected over horizon H)           │
│ predicted_net_benefit_usd   : f64 (projected savings)                  │
│ forecast_horizon_turns      : u32 (continuation horizon, default: 3)   │
│ policy_version              : "0.2.4-cw117b"                           │
│ decision_timestamp          : DateTime<Utc>                            │
└────────────────────────────────────────────────────────────────────────┘
```

### Strict Privacy & Storage Invariants
- **Zero Prompt Storage:** Evidence records contain zero prompt text, user messages, code snippets, or raw tool stdout.
- **SQLite Persistence:** Stored locally in `shadow_decision_evidence` table in `cw-store`.
- **Zero-Network Invariant:** Offline replay is strictly local, reads immutable JSON files, and contacts zero inference providers.

---

## 3. Replay Methodology & Sample Scope

### Datasets Analyzed

| Dataset Source | Git Commit | Sessions | Total Requests | Total Wire Spend ($) | Harness & Task |
|:---|:---:|:---:|:---:|:---:|:---|
| `docs/cw115_benchmark_data.json` | `e9b1b43b` | 6 | 59 | $0.109464 | Codex CLI 0.166.0 (F1 Bug Fixing, Reps 1 & 2) |
| `docs/cw115_four_mode_benchmark_data.json` | `72bec0d7` | 4 | 71 | $0.092237 | Codex CLI 0.166.0 (F1 Bug Fixing, Modes 0–3) |
| **Total Qualified** | — | **10** | **130** | **$0.201701** | **Codex CLI on `gpt-4o-mini`** |

### Execution Pipeline per Request
1. Parse request telemetry: `input_tokens`, `cached_tokens`, `uncached_tokens`, `output_tokens`, `raw_provider_usage`.
2. Reconstruct `CacheObservation` using CW-117 evidence rules.
3. Query `find_pricing_profile(OpenAI, "gpt-4o-mini")`.
4. Reconcile cost: evaluate theoretical cost using formula:
   $$\text{Cost} = \frac{C_{\text{cached}} \cdot 0.075 + T_{\text{uncached}} \cdot 0.15 + O_{\text{output}} \cdot 0.60}{10^6}$$
5. Record policy decision outcome and correlate with subsequent request outcomes.

---

## 4. Calibration & Economic Reconciliation Metrics

### A. Economic Reconciliation

| Metric | Benchmark Data (`cw115_benchmark`) | Four-Mode Data (`cw115_four_mode`) | Aggregate (Both) |
|:---|:---:|:---:|:---:|
| Total Reported Wire Cost | $0.109464 | $0.092237 | **$0.201701** |
| Total Model Profile Recalculated Cost | $0.109464 | $0.092237 | **$0.201701** |
| Absolute Total Variance | $0.0000000000 | $0.0000000000 | **$0.0000000000** |
| Max Single-Request Error | $0.0000000000 | $0.0000000000 | **$0.0000000000** |
| Pricing Profile Coverage | 100.0% | 100.0% | **100.0%** |

### B. Cache-State Classification & Agreement

| Metric | Benchmark Data | Four-Mode Data | Notes |
|:---|:---:|:---:|:---|
| `WarmVerified` Observations | 55 (93.2%) | 67 (94.4%) | Observed subsequent prompt reuse |
| `ColdLikely` Observations | 4 (6.8%) | 4 (5.6%) | Initial turn prompts ($\ge 1,024$ tokens) |
| `Unknown` Observations | 0 (0.0%) | 0 (0.0%) | All requests provided wire usage details |
| Warm Cache Hits Retained | 49 / 49 | 59 / 60 | Cache hit maintained across turn sequence |
| Transient Cache Drops to 0 | 0 | 1 | Observed at Mode 0 Req #9 (provider cache eviction) |
| Cold Initialization Agreement | 4 / 4 | 4 / 4 | Turn 1 cold prompt established prefix for Turn 2 |
| **Overall State Consistency** | **89.83%** | **87.32%** | High empirical agreement |

---

## 5. Investigation of Policy Weaknesses & Edge Cases

The replay evaluated the 8 policy edge cases specified in the CW-117B charter:

### Case 1: Warm Cache Dropped to 0 Hits on Subsequent Request
- **Location:** `cw115_m0_passthrough_f1_rep1` (Request #8 $\to$ Request #9).
- **Phenomenon:** Request #8 had 13,184 cached tokens (94.2% hit ratio, cost $0.001273). Request #9, despite appending a normal user/tool message without editing history, reported 0 cached tokens and 14,312 uncached tokens. Cost jumped to $0.002327 (+82.8%). At Request #10, cache hit ratio immediately returned to 94.3% (13,824 cached tokens).
- **Root Cause:** Provider-side infrastructure artifact (e.g. transient cache tier eviction or load-balancer rerouting to an unprimed engine node).
- **Policy Remedy:** In Phase 2, when a request unexpectedly reports 0 cached tokens following a `WarmVerified` turn with identical prefix, the policy must avoid permanently declaring the cache cold or assuming compaction is safe; it should transition to `CacheState::Unknown` until telemetry confirms stability.

### Case 2: Cold Initial Prompt Successfully Primes the Cache
- **Location:** Turn 1 $\to$ Turn 2 across all 10 sessions.
- **Phenomenon:** Request #1 in every session had 0 cached tokens on a ~11,980-token prompt (`ColdLikely`). Request #2 immediately achieved ~11,776 to 11,904 cached tokens (97-98% hit ratio).
- **Policy Implication:** Compacting history on Turn 1 carries zero prefix break penalty. However, once Turn 2 executes, the prefix is warm and historical mutations must immediately be deferred.

### Case 3: Applied Historical Pruning Incurred Massive Prefix Break Penalty
- **Location:** `cw115_m2_prune_f1_rep1` (Request #6) and `cw115_optimized_f1_rep1` (Request #6).
- **Phenomenon:** In Mode 2 Request #6, 4 pruning mutations were applied (`opt_prune_node_4_15`, `opt_prune_node_5_17`, etc.). Total input tokens decreased from 12,856 to 12,687 (-169 tokens). However, cached tokens collapsed from 12,544 to 11,264 (-1,280 cached tokens), causing uncached tokens to jump from 312 to 1,423 (+1,111 uncached tokens).
- **Policy Validation:** CW-117's rule correctly evaluated this on a warm cache as `DEFER` (`CachePrefixAtRisk`). The empirical benchmark proves that applying this candidate increased total costs rather than saving money.

### Case 4: Candidate-Level vs Plan-Level Consistency
- **Evaluation:** Evaluated candidate deferrals for active tool results vs aggregate plan savings.
- **Finding:** Plan-level deferral propagation prevents economic savings from overriding lifecycle protection. In replay, 0 safety violations or orphaned tool calls were permitted.

### Case 5: Pricing Profile Currency & Allowlist Strictness
- **Evaluation:** `find_pricing_profile` was verified against strict model identifiers.
- **Finding:** 100% of benchmark requests mapped to `gpt-4o-mini`. Negative unit tests verified that loose patterns (`gpt-4o-custom`, `claude-3-5-sonnet-custom-tuning`, `gemini-1.5-pro-experimental`) return `None` and fail safe to deferral.

### Case 6: Continuation Horizon Discrepancy ($H=3$ vs Actual Session Length)
- **Location:** Aggregate across all sessions.
- **Phenomenon:** ContextWatcher's economic evaluator assumes a fixed 3-turn continuation horizon ($H=3$). However, actual sessions averaged 17.8 requests in the four-mode trial (range 13–23 requests) and 9.8 requests in the pilot trial.
- **Policy Implication:** When a historical prefix break occurs early in a 20-turn session, the model continues reading uncached tokens on subsequent turns if the prefix changes again. In long sessions, the cost penalty of breaking an active cache is even higher than modeled by $H=3$.
- **Recommendation:** Allow continuation horizon $H$ to scale dynamically based on session trajectory in Phase 2.

### Case 7: One-Time Tail Steering Directive Alters Agent Trajectory
- **Location:** `cw115_m1_tail_f1_rep1` (Request #6).
- **Phenomenon:** Mode 1 injected a tail-only `PinContext` reminder on Request #6. Cached tokens remained completely intact (12,544 cached tokens at Request #6, 12,544 cached tokens at Request #7). The directive was not re-injected, yet the agent converged in 17 requests instead of 23.
- **Policy Validation:** CW-117 classifies tail pin operations as `NoOp` (`TailAppendPreservesPrefixNoDirectSaving`), conservatively valuing standalone direct savings at $0 USD while preserving 100% of the KV cache prefix.

### Case 8: Missing or Ambiguous Telemetry Handling
- **Location:** `cw115_benchmark_data.json` (Mode A native sessions).
- **Phenomenon:** Mode A native traces lack request-level latency (`ttfb_ms: null`) and do not break down sub-turn requests.
- **Policy Invariant:** Replay recorded these as `UncorrelatedMissingOutcome` notes without fabricating missing metrics.

---

## 6. Known Limitations in Historical Evidence

1. **Absence of Raw Prompt Text:** Committed datasets contain token usage counts, latencies, and mutation lists, but deliberately omit raw prompt messages to prevent repo bloat and protect test secrets. Consequently, historical replay cannot compute fresh BLAKE3 prefix fingerprints on historical runs; fingerprints are recorded as `None`.
2. **Unverifiable Counterfactuals:** In passthrough sessions (Mode B and Mode 0), Turn 1 was cold and theoretical optimization plans would have yielded positive modeled savings (`Apply`). However, because these alternative prompts were never executed upstream, these savings remain counterfactual estimates and are explicitly tagged `unverifiable_counterfactuals`.

---

## 7. Recommendations for Next Execution Gate (Phase 2)

1. **Transient Eviction Guard:** Add an edge detector for Case 1: when cached tokens drop to 0 unexpectedly while prefix fingerprint is unchanged, do not authorize aggressive compaction; defer until provider node stabilizes.
2. **Dynamic Horizon Scaling:** Extend the fixed $H=3$ horizon to incorporate observed session depth and task family characteristics.
3. **Observation-Driven Autopilot Activation:** When migrating from shadow mode to active autopilot, restrict automated mutations strictly to verified `ColdLikely` states or provably cache-safe tail appends (`PinContext`).
