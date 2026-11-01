# CW-117C — Prospective Shadow Decision & Outcome Correlation

**Milestone:** CW-117C (Prospective Shadow Decision & Outcome Correlation)  
**Date:** November 1, 2026  
**Status:** Completed & Deterministically Qualified  
**Branch:** `feat/cw-117c-prospective-correlation`  
**Crates Affected:** `cw-core`, `cw-store`, `cw-proxy`, `cw-cli`  
**Qualification Test Suite:** `crates/cw-proxy/tests/prospective_correlation_test.rs` (16/16 passed), `crates/cw-store/tests/calibration_evidence_test.rs` (5/5 passed)  
**Evidence Artifact:** [`docs/cw117c_synthetic_calibration_report.json`](cw117c_synthetic_calibration_report.json)  
**Operating Mode:** Strictly Observation-Only / Strict Shadow Mode (Zero mutations committed or applied to outbound requests)

---

## 1. Mission & Architectural Overview

In **CW-117A** and **CW-117B**, ContextWatcher established a cache-aware optimization policy engine and an empirical calibration pipeline capable of replaying historical traces. However, retrospective evaluation inherently lacks pre-dispatch decision identity and must rely on historical observations.

**CW-117C** builds the end-to-end prospective measurement pipeline connecting **pre-dispatch shadow decisions** evaluated before an outbound model request to **observed provider responses** and **continuation requests over a multi-turn horizon $H$**.

Crucially, CW-117C operates under a strict, provable boundary between:
1. **Measured Telemetry:** Empirically verified provider metrics (actual tokens, cached reads, cache writes, output tokens, TTFB, request latency, calculated dollar spend).
2. **Modeled Policy Estimations:** Pre-dispatch projections estimated by ContextWatcher (modeled baseline spend, modeled optimized spend, projected gross savings).
3. **Unverifiable Counterfactuals:** Explicit accounting for deferred or rejected mutations (`ActiveToolLifecycleProtection`, `SignedThinkingContent`, `SafetyViolation`). Because these mutations were deferred or rejected, they were never executed upstream; their alternative trajectories cannot be measured directly from passive telemetry.

---

## 2. Pipeline Architecture

```
Client Harness Request (Unmutated Source Payload)
  │
  ├── 1. Pre-Dispatch Policy Evaluation (Shadow Mode)
  │      ├─ Compute deterministic BLAKE3 payload fingerprint (compute_payload_fingerprint)
  │      ├─ Classify cache state (WarmVerified, ColdLikely, Unknown, Rebuilding)
  │      ├─ Evaluate candidate & plan decisions (Apply, Defer, Reject, NoOp)
  │      └─ Persist unique ShadowDecisionEvidence to SQLite (sde_{session}_{req}_{scope})
  │
  ├── 2. Zero-Drop Passthrough Forwarding
  │      ├─ Outbound bytes forwarded 100% bit-for-bit unchanged to provider
  │      └─ SSE chunks or JSON streamed with <2ms overhead
  │
  ├── 3. Asynchronous Recorder Completion (Fail-Open Tap)
  │      ├─ Finalize ContextTurn with observed provider usage (cached_tokens, output_tokens, TTFB)
  │      ├─ Construct ObservedProviderOutcome with model pricing profile
  │      ├─ Correlate pre-dispatch decision to immediate outcome (correlate_decision_with_outcome)
  │      │     ├─ MatchedImmediate on exact (session_id, request_index) match
  │      │     ├─ Cost delta = predicted baseline cost - actual observed cost
  │      │     └─ Cache consistency = predicted state vs actual cache hit ratio
  │      └─ Update continuation horizon observations for prior decisions (evaluate_prospective_horizon)
  │            ├─ Horizon window: base_req < req <= base_req + H
  │            ├─ Transition classification: WarmPreserved, WarmDrop, ColdWrite, ColdContinued
  │            └─ Truncation tracking: session_ended_before_horizon
  │
  └── 4. Calibration & Evidence Aggregation (CLI & REST)
         ├─ Build ProspectiveCalibrationReport separating Measured, Modeled, and Unverifiable
         └─ Render via CLI (`cw calibrate --synthetic` / `cw calibrate --session <id>`)
```

---

## 3. Data Structures & Entities

### 3.1. Durable Decision Identity (`ShadowDecisionEvidence`)
- `record_id`: Structured unique key `sde_{session_id}_{request_index}_{scope_key}` preventing SQLite overwrite collisions between concurrent candidates and plans.
- `payload_fingerprint`: BLAKE3 hash `pfp_{hash}` of the canonical outbound request JSON (zero prompt text or secrets stored).
- `forecast_horizon_turns`: Declared continuation horizon $H$ (default: 3 turns).
- `pricing_version` & `policy_version`: Auditable capability and algorithm versions.

### 3.2. Observed Provider Outcome (`ObservedProviderOutcome`)
- Derived directly from finalized `ContextTurn` and provider usage headers via `extract_normalized_provider_usage`:
  - `actual_input_tokens`, `actual_cached_tokens`, `actual_uncached_tokens`, `actual_cache_write_tokens`, `actual_output_tokens`
  - Normalized across provider wire formats: Anthropic disjoint buckets (cache write tokens are NOT added to uncached tokens, preventing double billing), OpenAI, and Gemini.
  - `calculated_cost_usd`: Recalculated from verified `ModelPricingProfile`. When essential usage telemetry is missing or incomplete, cost calculation is strictly prevented (`None`).
  - `observed_cache_hit_ratio`: $\frac{\text{cached\_tokens}}{\text{total\_input\_tokens}}$ (`None` when telemetry is missing).
  - `ttfb_ms` and `request_latency_ms`
  - `execution_outcome`: `"completed"`, `"stream_error"`, `"cancelled"`, etc.
  - `missing_data_reasons`: Explicit tracking for missing usage headers or unmeasured TTFB.

### 3.3. Decision-to-Outcome Correlation (`CorrelatedDecisionOutcome`)
- `correlation_status`:
  - `MatchedImmediate`: Pre-dispatch decision paired with completed, telemetry-valid immediate outbound request.
  - `PairedIncompleteExecution`: Pre-dispatch decision paired with an execution that failed, errored, or was cancelled before completion. Single-turn cost delta and cache consistency remain `None`.
  - `PairedMissingTelemetry`: Pre-dispatch decision paired with an execution whose provider usage telemetry was missing or incomplete. Cost delta and cache consistency remain `None`.
  - `UncorrelatedMissingOutcome`: Request failed or timed out before completion.
  - `UncorrelatedAmbiguous`: Session or request sequence mismatch.
- `single_turn_cost_delta_usd`:
  - Computed when `forecast_horizon_turns == 1` matching the 1-turn baseline.
  - When $H > 1$, single-turn cost delta is `None` with an explanatory note to avoid apples-to-oranges comparisons (multi-turn comparisons are tracked in `ProspectiveHorizonObservation`).
- `cache_state_consistent`:
  - `WarmVerified` $\rightarrow$ Consistent if observed cache hit ratio $> 5\%$.
  - `ColdLikely` $\rightarrow$ Consistent if observed cache hit ratio $< 5\%$.
  - Discrepancy details logged to `notes` if inconsistent.

### 3.4. Continuation Horizon Observation (`ProspectiveHorizonObservation`)
- `horizon_id`: Unique identifier `pho_{decision_record_id}`.
- `horizon_declared` vs `horizon_observed`: Declared turns $H$ vs actual turns executed before session termination.
- `tracking_status`:
  - `FullyObserved`: Full declared horizon $H$ observed with verified provider continuation requests.
  - `PendingContinuation`: Session is active; waiting for subsequent continuation turns to reach horizon $H$ (no premature truncation labeling).
  - `TruncatedEarly`: Session explicitly terminated before reaching declared horizon $H$.
- `horizon_complete`: `true` if $\text{observed} \ge \text{declared}$, `false` otherwise.
- `truncation_reason`: Populated with `"session_ended_before_horizon"` only when a session explicitly completes before $H$ turns.
- `transitions`: Detailed list of transitions per turn (`WarmPreserved`, `WarmDrop`, `ColdWrite`, `ColdContinued`) with `is_verified` flag. Initial base cache hit ratio is drawn directly from `base_outcome.observed_cache_hit_ratio` without hardcoded assumptions.
- `observed_cache_retention_ratio`: Fraction of warm cache transitions that retained hits $> 50\%$.
- `horizon_cost_delta_usd`: Compared when `FullyObserved` against cumulative verified continuation costs.

---

## 4. Calibration Report Decomposition

The prospective calibration report strictly isolates measured evidence from modeled projections:

```
┌──────────────────────────────────────────────────────────────────────────────┐
│ contextWatcher — CW-117C Prospective Shadow Calibration                      │
└──────────────────────────────────────────────────────────────────────────────┘

  Report Title: ContextWatcher CW-117C Prospective Shadow Calibration Report
  Git Commit  : git_commit_cw117c
  Generated At: 2026-11-01T08:01:00+00:00
  ────────────────────────────────────────────────────────────────────────────
  [1] Measured Provider Telemetry (Observed Requests: 6, Valid Telemetry: 6):
    - Total Measured Cost        : $0.048075
    - Total Measured Tokens      : 28500 cached / 0 uncached (34800 total input)
    - Overall Measured Hit Ratio : 81.90%
    - Mean TTFB                  : 228.3 ms
    - Mean Request Latency       : 781.7 ms

  [2] Modeled Policy Projections (Total Evaluations: 5):
    - Decisions                  : 2 Apply | 1 Defer | 1 Reject | 1 Ignore
    - Modeled Baseline Cost      : $0.044640
    - Modeled Optimized Cost     : $0.043215
    - Projected Gross Savings    : $0.001425
      * ActiveToolLifecycleProtection        : 1
      * ModeledEconomicBenefitPositive       : 2
      * SafetyViolation                      : 1
      * TailAppendPreservesPrefixNoDirectSaving : 1

  [3] Unverifiable Counterfactuals:
    - Deferred Candidates        : 1
    - Rejected Candidates        : 1
    - Rationale                  : Deferred and rejected mutations were never executed upstream; counterfactual alternative trajectory cannot be measured or proven directly from passive telemetry.
      * ActiveToolLifecycle deferrals preserve tool invocation integrity without generating modified requests.
      * SignedThinkingContent rejections preserve cryptographically signed model outputs without modifying thinking blocks.

  [4] Decision-to-Outcome Correlation:
    - Total Pre-Dispatch Decisions : 5
    - Matched Immediate Outbound   : 5
    - Paired Incomplete Execution  : 0
    - Paired Missing Telemetry     : 0
    - Cache State Consistent       : 5 requests
    - Cache State Inconsistent     : 0 requests
    - Mean Single-Turn Cost Delta  : $0.000000

  [5] Multi-Turn Horizon Tracking (Horizons: 5):
    - Fully Observed Horizons      : 3
    - Pending Continuation         : 0
    - Truncated (Session End)      : 2
    - Mean Observed Turns          : 2.4
    - Observed Cache Retention     : 100.00%
    - Warm Cache Drops Detected    : 0

  [6] Integrity & Safety Invariants:
    • Measured metrics reflect passive provider telemetry captured via proxy tap.
    • Modeled metrics reflect pre-dispatch policy estimations calculated in shadow mode.
    • Strict shadow mode ensured 100% byte fidelity to provider endpoints with zero mutations applied.
```

---

## 5. Qualification Test Suite (16 Deterministic Scenarios)

All 16 qualification scenarios are implemented in `crates/cw-proxy/tests/prospective_correlation_test.rs` and executed with mock providers (0 paid API calls, 100% offline):

| Scenario | Qualification Test | Verified Invariant | Status |
|:---|:---|:---|:---:|
| **SC-1** | `test_prospective_correlation_sc1_multiple_decisions_single_turn` | Pre-dispatch decision generation with multiple candidates & plan on same turn persist without collisions. | **PASS** |
| **SC-2** | `test_prospective_correlation_sc2_successful_response_immediate_match` | Standard successful response with usage matches pre-dispatch decision, cost delta & cache consistency computed. | **PASS** |
| **SC-3** | `test_prospective_correlation_sc3_sse_streaming_chunked_response` | Chunked SSE streaming parses usage on `message_stop`, extracts cached tokens, sets terminal outcome `completed`. | **PASS** |
| **SC-4** | `test_prospective_correlation_sc4_missing_or_zero_usage_tokens` | Provider response lacking usage headers handled fail-open; recorded with `observed_usage_missing`. | **PASS** |
| **SC-5** | `test_prospective_correlation_sc5_client_cancellation_recorded` | Early client cancellation before response completion records TTFB and marks outcome `cancelled`. | **PASS** |
| **SC-6** | `test_prospective_correlation_sc6_provider_error_recovery` | Upstream HTTP 500/503/429 error does not corrupt prior decision history; subsequent retry succeeds. | **PASS** |
| **SC-7** | `test_prospective_correlation_sc7_repeated_responses_on_sequence` | Repeated attempts or retries on same sequence cleanly update SQLite records without constraint errors. | **PASS** |
| **SC-8** | `test_prospective_correlation_sc8_concurrent_multisession_isolation` | Concurrent requests from distinct sessions remain strictly isolated in store and correlation pipeline. | **PASS** |
| **SC-9** | `test_prospective_correlation_sc9_unexpected_cache_miss_inconsistency` | Predicted `WarmVerified` but provider returned 0 cache hits detects `cache_state_consistent = false` with explanatory note. | **PASS** |
| **SC-10** | `test_prospective_correlation_sc10_horizon_truncation_on_session_end` | Session terminating before horizon $H$ expires records `horizon_complete = false` and `session_ended_before_horizon`. | **PASS** |
| **SC-11** | `test_prospective_correlation_sc11_strict_shadow_mode_wire_preservation` | Outbound request payload sent to provider matches original client payload byte-for-byte with 0 mutations. | **PASS** |
| **SC-12** | `test_prospective_correlation_sc12_provider_model_mismatch_rejected` | Decisions paired with mismatched provider or model are rejected as `UncorrelatedAmbiguous` with diagnostic notes. | **PASS** |
| **SC-13** | `test_prospective_correlation_sc13_explicit_output_tokens_required` | Wire usage lacking explicit output tokens is rejected across Anthropic, OpenAI, and Gemini to prevent cost undercounting. | **PASS** |
| **SC-14** | `test_prospective_correlation_sc14_horizon_consecutive_verification_and_unknown_baseline` | Horizon tracking requires $H$ consecutive verified turns for `FullyObserved`; unknown base telemetry yields unverified baseline. | **PASS** |
| **SC-15** | `test_prospective_correlation_sc15_session_end_signals` | Explicit session end via `x-cw-session-end` header or `POST /api/v1/sessions/:id/end` persists termination and prevents reopening; lifecycle monotonicity preserves `TruncatedEarly`. | **PASS** |
| **SC-16** | `test_prospective_correlation_sc16_per_turn_model_identity_preserved` | Reconstructed historical turns retain per-request model and pricing rather than adopting session's latest model. | **PASS** |

---

## 6. How to Reproduce

```bash
# 1. Run prospective correlation proxy qualification suite
cargo test -p cw-proxy --test prospective_correlation_test

# 2. Run SQLite persistence qualification suite
cargo test -p cw-store --test calibration_evidence_test

# 3. Generate terminal formatted prospective report
cargo run -p cw-cli -- calibrate --synthetic

# 4. Generate machine-readable JSON prospective report
cargo run -p cw-cli -- calibrate --synthetic --json > docs/cw117c_synthetic_calibration_report.json
```
