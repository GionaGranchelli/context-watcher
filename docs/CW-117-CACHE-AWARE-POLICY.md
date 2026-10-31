# CW-117 — Cache-Aware Optimization Policy Engine (Phase 1) Report

**Milestone:** CW-117 (Cache-Aware Optimization Policy Engine — Phase 1)  
**Date:** October 31, 2026  
**Status:** Completed & Deterministically Qualified  
**Branch:** `feat/cw-117-cache-aware-policy`  
**Crates Affected:** `cw-core` (`policy.rs`, `autopilot.rs`, `ir.rs`, `lib.rs`)  
**Mode:** Strict Shadow Mode Only (Decisions recorded; 0 mutations committed or applied)  
**Deterministic Test Suite:** `crates/cw-core/tests/policy_test.rs` (18/18 passed)  

---

## 1. Mission & Decision Architecture

ContextWatcher's CW-115 empirical qualification campaign demonstrated that context optimization cannot be treated as a pure token-counting problem. While append-only tail steering (`PinContext`) preserves the cached prefix bit-for-bit (yielding -34.06% tokens and +37.71% cost savings), blind historical pruning (`PruneOrStub`) can break an active prompt cache, causing cost inversions (up to +46.2% across multiple sessions) due to uncached re-reading and cache-write penalties.

CW-117 introduces ContextWatcher's **Provider-Aware, Cache-Sensitive Optimization Policy Engine**. Its mission is to determine whether an optimization candidate or plan should be applied, deferred, rejected, or left untouched—based on structural KV-cache impact, observed provider telemetry provenance, estimated economic benefit, task lifecycle safety, and evidence certainty.

### Architectural Decision Pipeline

```
ContextTurn (Nodes, KV Report, Lint Findings, Usage)
  │
  ├── 1. Cache-State Classification (classify_cache_state)
  │      ├─ WarmVerified  (recent provider cache hits for session/prefix)
  │      ├─ ColdLikely    (provider confirmed 0 hits on eligible prompt)
  │      ├─ Unknown       (telemetry absent, stale, below threshold, or ambiguous)
  │      └─ Rebuilding    (intentional compacted prefix observing initial reuse)
  │
  ├── 2. Structural & Safety Gating
  │      ├─ Analysis completeness check (!turn.analysis_complete -> REJECT)
  │      ├─ Node kind safety (UserPrompt/SystemPrompt/ThinkingSignature -> REJECT)
  │      ├─ Tool lifecycle pairing (is_protected_tool_result -> DEFER)
  │      └─ Plan-level deferral propagation (candidate Defer/Reject blocks plan Apply before economics)
  │
  ├── 3. Pure Economic Evaluation (evaluate_economic_impact)
  │      ├─ ModelPricingProfile lookup (Anthropic, OpenAI, Gemini)
  │      ├─ Prefix invalidation cost calculation (Turn 1 uncached re-read + cache-write fee)
  │      ├─ Horizon projection (subsequent turns at cached read discount; Medium confidence)
  │      └─ Net dollar delta & break-even calculation
  │
  └── 4. Explainable Policy Decision (OptimizationDecision)
         ├─ Apply   (ColdLikely + positive modeled economic benefit + lifecycle safe)
         ├─ Defer   (WarmVerified prefix at risk, or telemetry unknown, or rebuilding, or candidate deferred)
         ├─ Reject  (Incomplete analysis or safety invariant violation)
         └─ NoOp    (Tail pin preserving prefix with 0 direct token cut, or 0 benefit)
```

---

## 2. Cache-State Classification Rules

Cache state classification is driven strictly by evidence provenance, never by arbitrary time-based heuristics:

| Cache State | Definition | Provenance | Criteria |
|:---|:---|:---|:---|
| **`WarmVerified`** | Active, verified cache hits | `ObservedProvider` | Provider reported `cached_tokens > 0` on compatible session and model; prefix fingerprint aligned. |
| **`ColdLikely`** | Cache reuse confirmed unavailable | `ObservedProvider` | Provider telemetry explicitly reported `cached_tokens == 0` on a request meeting or exceeding model threshold. |
| **`Unknown`** | Absent, stale, or ambiguous | `None` / `LocalEstimate` | Wire telemetry unavailable (`telemetry_available == false`), input tokens below threshold (< min_threshold, including zero-length prompt), or incompatible prior observation (session, model, or prefix diverged). |
| **`Rebuilding`** | Active compaction stabilization | `ObservedProvider` / `LocalEstimate` | Intentional compaction rule applied on prior turn; subsequent turns observing new prefix reuse. |

### Strict Invariants
1. **No Time-Based Eviction Assumption:** An idle interval or agent resume does not classify a cache as `ColdLikely`. Telemetry remains `Unknown` until provider response headers/usage confirm actual status.
2. **Eligibility Threshold Awareness:** Input prompts below provider caching thresholds (e.g., <1,024 tokens for Anthropic/OpenAI, <32,768 tokens for Gemini 1.5) reporting 0 cached tokens are classified as `Unknown` eligibility, not cold engines.
3. **Provenance Segregation:** Local Radix tree prefix matches are tagged `TelemetryProvenance::LocalEstimate` and are never equated with `TelemetryProvenance::ObservedProvider`.
4. **Fail-Closed on Incompatibility or Zero-Length Prompt:** If prior cache observation contains a mismatched session ID, altered model/provider, or diverged prefix fingerprint, or if current request tokens are zero/insufficient to establish eligibility, classification strictly fails closed to `Unknown` (provenance `None`).

---

## 3. Pure Economic Model & Provider Profiles

The economic evaluator separates structural token reduction from billed financial impact over a bounded horizon $H$ (default: 3 turns):

### Baseline Cost Model
$$\text{Cost}_{\text{baseline}} = H \times \left( \frac{C_{\text{cached}}}{10^6} \cdot P_{\text{cached\_read}} + \frac{T - C_{\text{cached}}}{10^6} \cdot P_{\text{uncached\_input}} \right)$$

### Optimized Cost Model (with KV Prefix Break)
On Turn 1:
- If `WarmVerified`:
  - Preserved prefix: $C_{\text{preserved}} = \min(C_{\text{cached}}, K_{\text{break\_start}})$.
  - Invalidated tail: $T'_{\text{invalidated}} = T' - C_{\text{preserved}}$.
  - Pricing charges: For providers with separate cache-write fees (Anthropic), tokens written to cache are billed at the single cache-write rate $P_{\text{cache\_write}}$ ($3.75/M total write fee = 1.25\times$ base rate, not uncached rate plus write fee). For providers with no write fee (OpenAI, Gemini), uncached tokens are billed at $P_{\text{uncached\_input}}$.
  - Turn 1 cost includes preserved cached reads, uncached re-reading, and any applicable write fees.
- If `ColdLikely`:
  - 0 cached tokens existed.
  - All $T'$ tokens are read at uncached rate, directly saving $(T - T') \times P_{\text{uncached\_input}}$ with zero KV penalty.
- Subsequent Turns ($2 .. H$):
  - Compacted prefix $T'$ is modeled as cached: $T' \times P_{\text{cached\_read}}$ (labeled with `Medium` confidence as a modeled projection).

### Supported Pricing Profiles
* **OpenAI (`gpt-4o-mini`):** Uncached: $0.15 / Mtok | Cached read: $0.075 / Mtok (-50%) | Cache write: $0.00 / Mtok | Min threshold: 1,024 tok.
* **OpenAI (`gpt-4o`):** Uncached: $2.50 / Mtok | Cached read: $1.25 / Mtok (-50%) | Cache write: $0.00 / Mtok | Min threshold: 1,024 tok.
* **Anthropic (`claude-3-5-sonnet`):** Uncached: $3.00 / Mtok | Cached read: $0.30 / Mtok (-90%) | Cache write: $3.75 / Mtok (single write fee = 1.25x base) | Min threshold: 1,024 tok.
* **Anthropic (`claude-3-haiku`):** Uncached: $0.25 / Mtok | Cached read: $0.025 / Mtok (-90%) | Cache write: $0.30 / Mtok (single write fee = 1.25x base) | Min threshold: 1,024 tok.
* **Gemini (`gemini-1.5-flash`):** Uncached: $0.075 / Mtok | Cached read: $0.01875 / Mtok (-75%) | Cache write: $0.00 / Mtok | Min threshold: 32,768 tok.
* **Strict Identifier Matching:** Pricing resolution requires explicit, verified model identifiers (exact match or standard timestamped prefix like `gpt-4o-2024-08-06`, `claude-3-5-sonnet-20241022`). Loose substrings (e.g., `gpt-4o-custom-finetune`) are rejected.
* **Unsupported / Local Models:** Return `is_supported: false` and `EvaluationConfidence::Unverified`, triggering conservative `DEFER`.

---

## 4. Decision Policy & Explanation Catalog

Every evaluation produces an explicit `OptimizationDecision` with a typed `DecisionReasonCode`:

### Decision Outcomes

| Decision | Primary Reason Code | Typical Trigger Scenario |
|:---|:---|:---|
| **`APPLY`** | `ModeledEconomicBenefitPositive` | Cache is credibly `ColdLikely`, candidate has positive token savings, pricing verified, net USD benefit positive. |
| **`DEFER`** | `CachePrefixAtRisk` | Cache is `WarmVerified`, but candidate mutates historical prefix, invalidating cached tokens. |
| **`DEFER`** | `CacheTelemetryUnknown` | Cache telemetry is absent or unverified; conservative fail-safe deferral. |
| **`DEFER`** | `ActiveToolLifecycleProtection` | Candidate targets an active or unread tool result awaiting assistant response. |
| **`DEFER`** | `MissingPricingProfile` | Model pricing is unverified or unsupported; no automatic economic approval. |
| **`DEFER`** | `CacheRebuilding` | Prefix is currently stabilizing following an intentional compaction. |
| **`NO_OP`** | `TailAppendPreservesPrefixNoDirectSaving` | Append-only tail operation (`PinContext`) preserves prefix bit-for-bit (0 KV break) but has 0 direct token reduction. |
| **`NO_OP`** | `NoDemonstrableBenefit` | Candidate yields $\le 0$ net token reduction or non-positive dollar benefit. |
| **`REJECT`** | `IncompleteContextAnalysis` | Turn payload analysis was partial, malformed, or unsupported for mutation. |
| **`REJECT`** | `SafetyViolation` | Candidate targets protected kinds (`UserMessage`, `SystemPrompt`, signed thinking blocks). |

> [!NOTE]
> Multi-turn continuation savings ($H > 1$) are modeled projections based on expected prefix reuse, not verified facts. Evaluated multi-turn projections carry `EvaluationConfidence::Medium` to distinguish them from direct observation.
>
> Furthermore, plan-level evaluation strictly enforces safety precedence: if any candidate in an optimization plan is deferred (e.g. for active tool lifecycle protection) or rejected, the plan-level decision cannot be `Apply` regardless of projected positive economic savings on a cold cache.

---

## 5. Shadow-Mode Operation

In Phase 1, the engine runs strictly in **Shadow Mode** (`ShadowPolicyConfig { enabled: true, strict_shadow_only: true, .. }`):

1. **Passive Observation:** Observes incoming turns and generated optimization candidates.
2. **Evaluation & Attribution:** Evaluates cache state, KV-break boundaries, and economic impact.
3. **Diagnostic Logging:** Records explainable decisions (`report.shadow_decisions`).
4. **Zero Mutation Guarantee:** Emits **0** mutation rules into `report.generated_mutations`. Payload bytes forwarded to the upstream provider are **100% bit-for-bit identical** to the unmutated original.
5. **Strict Shadow Isolation:** When `strict_shadow_only` is true, the engine completely suppresses mutation generation and payload modification even if legacy `autopilot.config.enabled` is true.
6. **Privacy Preserving:** Diagnostics contain only node IDs, token spans, reason codes, and numeric projections; zero prompt text is stored.

### Example Shadow Diagnostic Output
```
Candidate: prune_exec_1
Cache state: WarmVerified
Estimated KV break: 1240 tokens
Decision: Defer
Reason: CachePrefixAtRisk
Cost confidence: High
Actual mutation applied by shadow policy: false
```

---

## 6. Deterministic Qualification Evidence

Qualification suite located in [`crates/cw-core/tests/policy_test.rs`](file:///home/gionag/Development/contextWatcher/crates/cw-core/tests/policy_test.rs). All 18 tests pass deterministically in default CI:

| # | Test Name | Scenario Tested | Result |
|:---:|:---|:---|:---:|
| 1 | `test_policy_1_warm_cache_early_historical_pruning_defers` | `WarmVerified` cache + early historical pruning | `DEFER` (`CachePrefixAtRisk`) |
| 2 | `test_policy_2_warm_cache_append_only_tail_pin_noops` | `WarmVerified` cache + append-only tail pin | `NO_OP` (`TailAppendPreservesPrefixNoDirectSaving`, 0 KV break) |
| 3 | `test_policy_3_unknown_cache_telemetry_defers` | `Unknown` cache telemetry + historical candidate | `DEFER` (`CacheTelemetryUnknown`) |
| 4 | `test_policy_4_stale_or_incompatible_cache_observation` | Provider/model/session divergence across turns | Fails closed to `Unknown` (`TelemetryProvenance::None`) |
| 5 | `test_policy_5_cold_cache_beneficial_candidate_applies` | `ColdLikely` confirmed + beneficial prune | `APPLY` (`ModeledEconomicBenefitPositive`, 0 KV penalty) |
| 6 | `test_policy_6_missing_or_unsupported_pricing_defers` | Unsupported model pattern ("custom-local-model") | `DEFER` (`MissingPricingProfile`, Unverified) |
| 7 | `test_policy_7_multiple_mutations_overlapping_kv_break` | 2 candidates with overlapping break regions | Earliest break token used; no break summation |
| 8 | `test_policy_8_unsafe_tool_lifecycle_protection_defers` | Candidate targeting unread tail tool output | `DEFER` (`ActiveToolLifecycleProtection`) |
| 9 | `test_policy_9_zero_or_negative_token_savings_noops` | Candidate with $\le 0$ saved tokens on cold cache | `NO_OP` (`NoDemonstrableBenefit`) |
| 10 | `test_policy_10_incomplete_turn_analysis_rejects` | `turn.analysis_complete == false` | `REJECT` (`IncompleteContextAnalysis`) |
| 11 | `test_policy_11_feature_flag_disabled_leaves_behavior_unchanged` | `shadow_policy.enabled == false` | 0 shadow decisions; payload unmutated |
| 12 | `test_policy_12_shadow_mode_enabled_records_decisions_without_mutations` | `shadow_policy.enabled == true` | Decisions recorded; 0 mutations committed |
| 13 | `test_policy_pricing_profiles_anthropic_write_surcharge_calculation` | Anthropic single cache-write rate ($3.75/M total) | Negative net benefit on Turn 1 verified |
| 14 | `test_policy_pricing_profiles_gemini_threshold_handling` | Gemini prompt < 32,768 tokens reporting 0 hits | Classified as `Unknown` eligibility |
| 15 | `test_policy_p0_plan_defers_when_candidate_deferred_for_lifecycle` | Plan evaluation where candidate has lifecycle deferral | Plan `DEFER` (`ActiveToolLifecycleProtection`); never `APPLY` |
| 16 | `test_policy_classify_cache_state_fails_closed_on_zero_length_input` | Zero-length input prompt (`total_tokens == 0`) | Fails closed to `Unknown` |
| 17 | `test_policy_loose_or_unverified_model_names_rejected` | Loose model patterns (`gpt-4o-custom`, `custom-sonnet`) | Rejected (`None` profile); defers in evaluation |
| 18 | `test_policy_shadow_mode_suppresses_mutations_even_if_autopilot_enabled` | `autopilot.enabled: true` + `strict_shadow_only: true` | Suppresses mutation application; payload unmutated |

---

## 7. Limitations & Future Empirical Qualifications

1. **Model Horizon Uncertainty:** The current economic evaluator models a fixed $H=3$ turn continuation horizon. Dynamic horizon estimation based on task complexity (e.g. from linter loop findings) is deferred to Phase 2.
2. **Behavioral Steering Valuation:** Mode 1 prompt steering has demonstrated significant empirical request reductions in CW-115, but is conservatively valued at $0 direct financial savings by the economic model to avoid speculative claims.
3. **Local Engine KV Models:** Local engine memory footprint (PagedAttention / slot allocation) uses structural token accounting; hardware VRAM residency will be benchmarked under CW-104.
