# CW-115 — Four-Mode Optimization Isolation Experiment Report

**Milestone:** CW-115 (Empirical Performance & Savings Qualification)  
**Date:** October 31, 2026  
**Status:** Completed & Documented  
**Branch:** `feat/cw-115-optimization-isolation`  
**Harness:** Codex CLI (`0.166.0`)  
**Workload:** `F1BugFixing` (`math_ops.py` / `test_math_ops.py`)  
**Provider / Model:** OpenAI (`gpt-4o-mini`, Responses API)  
**Git Commit SHA:** `72bec0d78c0224ce24ca47e3aca48ed72d622c67` (v0.2.4)  
**Machine-Readable Dataset:** [`docs/cw115_four_mode_benchmark_data.json`](cw115_four_mode_benchmark_data.json)  
**Total Campaign Spend:** $0.0922 USD (SpendGuard Session Admission Reservations: $0.10 / session; $1.00 envelope)  

---

## 1. Objective & Experimental Design

Following the exploratory pilot (PR #21), which uncovered prompt-cache sensitivity and cost inversions caused by blind historical pruning, this experiment isolates ContextWatcher optimization mechanisms into four distinct modes to observe their independent effects on **token consumption**, **prompt-cache reuse**, **execution cost**, and **latency**:

1. **Mode 0 (Passthrough Baseline):** ContextWatcher proxy active, zero mutations applied. Serves as the experimental control representing unmutated harness cache behavior through the proxy.
2. **Mode 1 (Cache-Safe Tail Operations):** Only tail operations applied (`PinContext` / high-attention recency steering). Historical turns are left unmutated.
3. **Mode 2 (Historical Pruning):** Only historical compaction applied (`PruneOrStub` on past tool results). Zero tail pin directives. Isolates the effect of prefix mutation and measures pure token cuts against cache retention.
4. **Mode 3 (Combined Optimization):** Both historical compaction (`PruneOrStub`) and tail steering (`PinContext`) applied simultaneously.

---

## 2. Empirical Benchmark Telemetry

All metrics below originate directly from raw OpenAI response telemetry (`usage.prompt_tokens_details.cached_tokens`, `input_tokens`, `output_tokens`) and network timers recorded in [`docs/cw115_four_mode_benchmark_data.json`](cw115_four_mode_benchmark_data.json):

| Metric | Mode 0 (Passthrough) | Mode 1 (Tail Pin Only) | Mode 2 (Hist Prune Only) | Mode 3 (Combined) |
|:---|:---:|:---:|:---:|:---:|
| **Session ID** | `cw115_m0_passthrough_f1_rep1` | `cw115_m1_tail_f1_rep1` | `cw115_m2_prune_f1_rep1` | `cw115_m3_combined_f1_rep1` |
| **Model Requests** | 23 | 17 (-6) | 18 (-5) | 13 (-10) |
| **Total Input Tokens** | 349,980 | 230,763 | 265,475 | 174,920 |
| **Input Token Delta vs M0** | *Baseline* | **-34.06%** (-119,217) | **-24.15%** (-84,505) | **-50.02%** (-175,060) |
| **Cached Input Tokens** | 313,472 | 212,864 | 250,880 | 156,160 |
| **Prompt Cache Hit Ratio** | 89.57% | 92.24% (+2.68%) | 94.50% (+4.93%) | 89.28% (-0.29%) |
| **Uncached Input Tokens** | 36,508 | 17,899 | 14,595 | 18,760 |
| **Total Output Tokens** | 4,975 | 2,110 | 4,696 | 3,334 |
| **Session Wall Time** | 60,385 ms | 36,393 ms (-39.7%) | 52,153 ms (-13.6%) | 41,768 ms (-30.8%) |
| **Provider Cost (USD)** | **$0.03197** | **$0.01992** | **$0.02382** | **$0.01653** |
| **Cost Savings vs M0** | *Baseline* | **+37.71%** (-$0.01206) | **+25.49%** (-$0.00815) | **+48.31%** (-$0.01545) |
| **Cache Ratio Retained** | *Baseline* | **Yes (92.24%)** | **Yes (94.50%)** | **Yes (89.28%)** |
| **Acceptance Test Status** | **PASS** | **PASS** | **PASS** | **PASS** |

> [!NOTE]
> **Telemetry Reconciliation:** Output tokens, input tokens, cached tokens, and costs in the table above match the raw dataset in [`docs/cw115_four_mode_benchmark_data.json`](cw115_four_mode_benchmark_data.json) exactly.
> **Prefix Preservation Distinction:** While Mode 1 retained a 92.24% prompt cache hit ratio in this live run, byte-level prefix equality is proven via deterministic wire fixture testing in CI (`test_cw115_four_mode_isolation_and_prefix_preservation`), not inferred from provider cache ratios alone.

---

## 3. Key Isolation Findings

### Finding 1: Mode 1 Behavioral Steering vs. Context Compaction
* **Turn-Boundary Directive Application:** In Mode 1, `PinContext` applied the directive (`OPTIMIZATION_ACTIVE: keep helper signatures concise and clean.`) at the start of Turn 2 (request 6 of 17), where the payload tail was a supported `message` item.
* **Fail-Closed Sub-Request Behavior:** On subsequent sub-requests within Turn 2 (requests 7..17), the harness was executing tool iterations where the payload tail was a `function_call_output` item. Under the CW-R5 safety contract, `PinContext` does not append text to raw tool execution records; it was therefore reported unapplied on requests 7..17.
* **Behavioral Guidance Impact:** Despite being injected once at the Turn 2 boundary, this prompt directive guided the model's plan, resulting in **17 model requests** instead of 23. This demonstrates that Mode 1's observed savings arose primarily from **behavioral prompt steering** that reduced tool iterations, rather than purely mechanical token compaction.

### Finding 2: Mode 2 (Historical Pruning) Cuts Tokens but Demands Boundary Stability
* **Direct Token Savings:** Pruning resolved test execution logs from Turn 1 cut 84,505 input tokens (-24.15% vs Mode 0) and saved 25.49% in provider cost ($0.02382 vs $0.03197).
* **Cache Alignment in Clean Runs:** In this run, the pruned Turn 1 stub remained consistent across Turn 2 requests, allowing the provider to cache the compacted prefix (94.50% cache hit ratio).
* **Comparison to Pilot Risk:** In contrast to Repetition 1 of the pilot (where mid-stream pruning broke a warm prefix during ongoing tool calls and led to a +108.2% cost inversion), historical pruning succeeded here because Turn 1 was already settled when compaction occurred.

### Finding 3: Mode 3 (Combined Optimization) Achieves Compound Maximum Savings
* **Compound Impact:** Combining historical tool pruning with tail steering yielded the lowest token count and cost:
  * Input tokens reduced by **-50.02%** (174,920 vs 349,980).
  * Model requests reduced by **-43.5%** (13 requests vs 23 requests).
  * Total provider cost reduced by **-48.31%** ($0.01653 vs $0.03197).
  * Wall time reduced by **-30.8%** (41.8s vs 60.4s).
* **Functional Integrity:** All unit tests (`test_math_ops.py`) passed cleanly in every mode.

---

## 4. Limitations & Socratic Perspective

1. **Exploratory Scope & Order Effects:**
   The four modes were executed in a single sequential order (Mode 0 $\to$ Mode 1 $\to$ Mode 2 $\to$ Mode 3). While each mode ran in an isolated fresh workspace with private mock configurations and temporary home directories, an $N=1$ run cannot account for OpenAI cluster routing variations or stochastic model generation variance. These findings represent exploratory evidence of potential benefits and mechanisms rather than established repeatable causality.

2. **Prompt Engineering vs. Context Pruning:**
   Mode 1's directive asked the model to keep helper signatures concise. A significant portion of its token and cost savings came from reducing tool iterations (17 vs 23 requests). Teams should distinguish between prompt-engineering interventions that alter agent behavior and mechanical optimizations that remove context bloat without instruction changes.

3. **Admission Control vs. Wire Ceiling:**
   `SpendGuard` enforces a pre-request reservation ($0.10 / session) that acts as session-level admission control (blocking starting a session if funds are exhausted), not an in-flight byte-level wire ceiling. The 30-request cap is a post-execution iteration guard verified on store turns.

---

## 5. Deterministic Verification & CI Evidence

To ensure these guarantees are strictly enforced in regression testing without external API dependencies:

1. **`test_cw115_four_mode_isolation_and_prefix_preservation`:**
   - **Mode 0:** Confirms zero mutations applied; wire payload is 100% identical to incoming request.
   - **Mode 1:** Confirms that items `input[0..3]` (user, assistant, tool result, assistant response) are identical to Mode 0, with only `input[4]` (tail) modified. Verifies `estimate_mutation_impact` produces strictly `kv_break_tokens == 0`.
   - **Mode 2:** Confirms that historical item `input[2]` is pruned to stub text, mutating the prefix. Verifies `estimate_mutation_impact` produces `kv_break_tokens > 0`.
   - **Mode 3:** Confirms both historical item `input[2]` is pruned and tail item `input[4]` has the pinned directive.
   - **Disclosure Assertions:** Confirms that `compute_four_mode_comparisons` surfaces any cache hit ratio drops or cost inversions as explicit warnings.
2. **`test_cw115_cost_model_and_budget_guards`:**
   - Confirms pricing formulas, pre-request reservation mechanics, and single-commit reconciliation.
