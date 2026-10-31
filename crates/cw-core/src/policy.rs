use crate::ir::{ContextNode, ContextTurn, KvCacheReport, MutationOp, NodeKind, ProviderKind};
use crate::mutation::is_protected_tool_result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// High-level cache state classifications based on empirical evidence provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheState {
    /// Recent provider cache-hit evidence for a compatible session and prefix.
    WarmVerified,
    /// Credible evidence suggests cache reuse is unavailable (e.g. verified 0 hits across turns or cold session start).
    ColdLikely,
    /// Telemetry absent, stale, unsupported or ambiguous.
    Unknown,
    /// An intentional compacted prefix has been established and subsequent reuse is being observed.
    Rebuilding,
}

/// Provenance and credibility source for cache telemetry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetryProvenance {
    /// Direct from provider usage/headers (e.g., prompt_tokens_details.cached_tokens, cache_read_input_tokens).
    ObservedProvider,
    /// Locally computed estimate (e.g., Radix tree LCP token count).
    LocalEstimate,
    /// Absent or unavailable.
    None,
}

/// Structured cache observation tracking provenance and evidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheObservation {
    pub state: CacheState,
    pub provenance: TelemetryProvenance,
    pub observed_cached_tokens: Option<u32>,
    pub reported_input_tokens: Option<u32>,
    pub provider: ProviderKind,
    pub model: String,
    pub session_id: String,
    pub prefix_fingerprint: Option<String>,
    pub observed_at: DateTime<Utc>,
    pub turn_seq: u32,
    pub notes: Vec<String>,
}

/// Computes a deterministic BLAKE3 fingerprint of the prompt prefix.
pub fn compute_prefix_fingerprint(nodes: &[ContextNode]) -> String {
    let mut hasher = blake3::Hasher::new();
    for node in nodes {
        hasher.update(node.fingerprint.0.as_bytes());
        hasher.update(b"|");
    }
    hasher.finalize().to_hex()[..24].to_string()
}

/// Classifies the active cache state using evidence provenance and strict invariants.
///
/// Invariants:
/// - Do not classify a cache as cold solely because time elapsed or an agent resumed.
/// - When evidence is unavailable or ambiguous, classify it as `Unknown`.
/// - Distinguish observed provider cache usage from local estimates.
#[allow(clippy::too_many_arguments)]
pub fn classify_cache_state(
    session_id: &str,
    provider: ProviderKind,
    model: &str,
    turn_seq: u32,
    current_prefix_fp: Option<&str>,
    prior_observation: Option<&CacheObservation>,
    kv_report: Option<&KvCacheReport>,
    intentional_rebuild_in_progress: bool,
) -> CacheObservation {
    let mut notes = Vec::new();

    // 1. Rebuilding state: an intentional compaction occurred and subsequent reuse is being established.
    if intentional_rebuild_in_progress {
        notes.push("Intentional compacted prefix established; observing reuse.".to_string());
        let provenance = if kv_report.map(|r| r.telemetry_available).unwrap_or(false) {
            TelemetryProvenance::ObservedProvider
        } else {
            TelemetryProvenance::LocalEstimate
        };
        return CacheObservation {
            state: CacheState::Rebuilding,
            provenance,
            observed_cached_tokens: kv_report.map(|r| r.hit_tokens),
            reported_input_tokens: None,
            provider,
            model: model.to_string(),
            session_id: session_id.to_string(),
            prefix_fingerprint: current_prefix_fp.map(ToString::to_string),
            observed_at: Utc::now(),
            turn_seq,
            notes,
        };
    }

    // 2. Telemetry absent or report missing -> Unknown
    let Some(report) = kv_report else {
        notes.push("No KV cache report provided.".to_string());
        return CacheObservation {
            state: CacheState::Unknown,
            provenance: TelemetryProvenance::None,
            observed_cached_tokens: None,
            reported_input_tokens: None,
            provider,
            model: model.to_string(),
            session_id: session_id.to_string(),
            prefix_fingerprint: current_prefix_fp.map(ToString::to_string),
            observed_at: Utc::now(),
            turn_seq,
            notes,
        };
    };

    if !report.telemetry_available {
        notes.push("Provider wire telemetry absent or unverified.".to_string());
        let provenance = if report.hit_tokens > 0 {
            TelemetryProvenance::LocalEstimate
        } else {
            TelemetryProvenance::None
        };
        return CacheObservation {
            state: CacheState::Unknown,
            provenance,
            observed_cached_tokens: None,
            reported_input_tokens: None,
            provider,
            model: model.to_string(),
            session_id: session_id.to_string(),
            prefix_fingerprint: current_prefix_fp.map(ToString::to_string),
            observed_at: Utc::now(),
            turn_seq,
            notes,
        };
    }

    // 3. Telemetry is available from provider
    let hit_tokens = report.hit_tokens;

    // Check compatibility with prior observation if present
    if let Some(prior) = prior_observation {
        if prior.session_id != session_id {
            notes.push("Session ID mismatch against prior observation.".to_string());
        }
        if prior.provider != provider || prior.model != model {
            notes.push("Provider/model changed between observations.".to_string());
        }
        if let (Some(prior_fp), Some(curr_fp)) = (&prior.prefix_fingerprint, current_prefix_fp)
            && prior_fp != curr_fp
        {
            notes.push("Prefix fingerprint diverged from prior observation.".to_string());
        }
    }

    if hit_tokens > 0 {
        notes.push(format!(
            "Observed {hit_tokens} cached tokens from provider."
        ));
        CacheObservation {
            state: CacheState::WarmVerified,
            provenance: TelemetryProvenance::ObservedProvider,
            observed_cached_tokens: Some(hit_tokens),
            reported_input_tokens: None,
            provider,
            model: model.to_string(),
            session_id: session_id.to_string(),
            prefix_fingerprint: current_prefix_fp.map(ToString::to_string),
            observed_at: Utc::now(),
            turn_seq,
            notes,
        }
    } else {
        // Hit tokens == 0 with observed telemetry
        // Determine whether this is credibly cold or below threshold/ambiguous
        let total_tokens = report.hit_tokens + report.newly_cached_tokens;
        let min_threshold = min_cache_threshold_for_model(provider, model);

        if total_tokens > 0 && total_tokens < min_threshold {
            notes.push(format!(
                "Input tokens ({total_tokens}) below provider caching threshold ({min_threshold}); cache eligibility unavailable."
            ));
            CacheObservation {
                state: CacheState::Unknown,
                provenance: TelemetryProvenance::ObservedProvider,
                observed_cached_tokens: Some(0),
                reported_input_tokens: Some(total_tokens),
                provider,
                model: model.to_string(),
                session_id: session_id.to_string(),
                prefix_fingerprint: current_prefix_fp.map(ToString::to_string),
                observed_at: Utc::now(),
                turn_seq,
                notes,
            }
        } else {
            notes.push("Provider confirmed 0 cached tokens on cache-eligible prompt.".to_string());
            CacheObservation {
                state: CacheState::ColdLikely,
                provenance: TelemetryProvenance::ObservedProvider,
                observed_cached_tokens: Some(0),
                reported_input_tokens: Some(total_tokens),
                provider,
                model: model.to_string(),
                session_id: session_id.to_string(),
                prefix_fingerprint: current_prefix_fp.map(ToString::to_string),
                observed_at: Utc::now(),
                turn_seq,
                notes,
            }
        }
    }
}

/// Helper returning the minimum caching threshold for a provider/model.
pub fn min_cache_threshold_for_model(provider: ProviderKind, model: &str) -> u32 {
    match provider {
        ProviderKind::Anthropic => 1024,
        ProviderKind::OpenAI => 1024,
        ProviderKind::Gemini => {
            if model.contains("flash") || model.contains("pro") {
                32768
            } else {
                1024
            }
        }
        ProviderKind::OpenAICompatible => 1024,
    }
}

/// Provider-aware pricing profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPricingProfile {
    pub provider: ProviderKind,
    pub model_pattern: String,
    /// USD per 1M uncached input tokens
    pub uncached_input_per_mtok: f64,
    /// USD per 1M cached input read tokens
    pub cached_input_read_per_mtok: f64,
    /// USD per 1M tokens written to cache (surcharge over base, e.g. Anthropic +25% = 3.75 total; 0 if free)
    pub cache_write_per_mtok: f64,
    /// USD per 1M output tokens
    pub output_per_mtok: f64,
    /// Minimum token threshold required for provider prompt caching
    pub min_cache_threshold_tokens: u32,
    pub is_supported: bool,
}

/// Look up the verified pricing profile for a given provider and model name.
pub fn find_pricing_profile(provider: ProviderKind, model: &str) -> Option<ModelPricingProfile> {
    let lower = model.to_ascii_lowercase();
    match provider {
        ProviderKind::OpenAI => {
            if lower.contains("gpt-4o-mini") {
                Some(ModelPricingProfile {
                    provider,
                    model_pattern: "gpt-4o-mini".to_string(),
                    uncached_input_per_mtok: 0.15,
                    cached_input_read_per_mtok: 0.075, // 50% discount
                    cache_write_per_mtok: 0.0,         // Free caching
                    output_per_mtok: 0.60,
                    min_cache_threshold_tokens: 1024,
                    is_supported: true,
                })
            } else if lower.contains("gpt-4o") {
                Some(ModelPricingProfile {
                    provider,
                    model_pattern: "gpt-4o".to_string(),
                    uncached_input_per_mtok: 2.50,
                    cached_input_read_per_mtok: 1.25, // 50% discount
                    cache_write_per_mtok: 0.0,
                    output_per_mtok: 10.00,
                    min_cache_threshold_tokens: 1024,
                    is_supported: true,
                })
            } else {
                None
            }
        }
        ProviderKind::Anthropic => {
            if lower.contains("claude-3-5-sonnet") || lower.contains("sonnet") {
                Some(ModelPricingProfile {
                    provider,
                    model_pattern: "claude-3-5-sonnet".to_string(),
                    uncached_input_per_mtok: 3.00,
                    cached_input_read_per_mtok: 0.30, // 90% discount
                    cache_write_per_mtok: 3.75,       // +25% write surcharge
                    output_per_mtok: 15.00,
                    min_cache_threshold_tokens: 1024,
                    is_supported: true,
                })
            } else if lower.contains("claude-3-haiku") || lower.contains("haiku") {
                Some(ModelPricingProfile {
                    provider,
                    model_pattern: "claude-3-haiku".to_string(),
                    uncached_input_per_mtok: 0.25,
                    cached_input_read_per_mtok: 0.025, // 90% discount
                    cache_write_per_mtok: 0.30,
                    output_per_mtok: 1.25,
                    min_cache_threshold_tokens: 1024,
                    is_supported: true,
                })
            } else {
                None
            }
        }
        ProviderKind::Gemini => {
            if lower.contains("flash") {
                Some(ModelPricingProfile {
                    provider,
                    model_pattern: "gemini-1.5-flash".to_string(),
                    uncached_input_per_mtok: 0.075,
                    cached_input_read_per_mtok: 0.01875, // 75% discount
                    cache_write_per_mtok: 0.0,
                    output_per_mtok: 0.30,
                    min_cache_threshold_tokens: 32768,
                    is_supported: true,
                })
            } else if lower.contains("pro") {
                Some(ModelPricingProfile {
                    provider,
                    model_pattern: "gemini-1.5-pro".to_string(),
                    uncached_input_per_mtok: 3.50,
                    cached_input_read_per_mtok: 0.875, // 75% discount
                    cache_write_per_mtok: 0.0,
                    output_per_mtok: 10.50,
                    min_cache_threshold_tokens: 32768,
                    is_supported: true,
                })
            } else {
                None
            }
        }
        ProviderKind::OpenAICompatible => None,
    }
}

/// Evaluation confidence level reflecting data completeness and certainty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvaluationConfidence {
    /// Pricing and provider cache telemetry verified.
    High,
    /// Estimated structural impact with verified pricing.
    Medium,
    /// Speculative or incomplete telemetry/pricing.
    Low,
    /// Missing critical data (e.g. unknown pricing or unmeasured cache).
    Unverified,
}

/// Result of a pure economic evaluation over a bounded horizon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EconomicEvaluation {
    pub baseline_cost_usd: f64,
    pub optimized_cost_usd: f64,
    pub net_benefit_usd: f64,
    pub benefit_pct: f32,
    pub break_even_turns: Option<u32>,
    pub horizon_turns: u32,
    pub confidence: EvaluationConfidence,
    pub assumptions: Vec<String>,
    pub reasons: Vec<String>,
}

/// Pure economic evaluator: computes expected cost impact over horizon turns.
///
/// Invariants:
/// - Do not assume token savings equal billed savings.
/// - Do not treat KV-break estimates as confirmed cache misses without checking cache state.
/// - Do not assume historical pruning always destroys the entire cached prefix.
/// - Do not monetize changes in agent behavior using invented request-count reductions.
/// - Do not sum overlapping candidates without accounting for combined impact.
/// - Unknown pricing or unsupported provider behavior produces unverified confidence and zero recommendation.
pub fn evaluate_economic_impact(
    total_tokens: u32,
    saved_tokens: i32,
    earliest_kv_break_start: Option<u32>,
    is_append_only: bool,
    cache_obs: &CacheObservation,
    pricing: Option<&ModelPricingProfile>,
    horizon_turns: u32,
) -> EconomicEvaluation {
    let mut assumptions = Vec::new();
    let mut reasons = Vec::new();
    let horizon = horizon_turns.max(1);

    let Some(profile) = pricing.filter(|p| p.is_supported) else {
        assumptions.push("Pricing profile unavailable or unsupported.".to_string());
        reasons.push("Cannot compute economic impact without verified pricing.".to_string());
        return EconomicEvaluation {
            baseline_cost_usd: 0.0,
            optimized_cost_usd: 0.0,
            net_benefit_usd: 0.0,
            benefit_pct: 0.0,
            break_even_turns: None,
            horizon_turns: horizon,
            confidence: EvaluationConfidence::Unverified,
            assumptions,
            reasons,
        };
    };

    let cached_tokens = cache_obs
        .observed_cached_tokens
        .unwrap_or(0)
        .min(total_tokens);
    let uncached_tokens = total_tokens.saturating_sub(cached_tokens);

    // Baseline cost per turn:
    let turn_baseline_cached_cost =
        (cached_tokens as f64 / 1_000_000.0) * profile.cached_input_read_per_mtok;
    let turn_baseline_uncached_cost =
        (uncached_tokens as f64 / 1_000_000.0) * profile.uncached_input_per_mtok;
    let turn_baseline_cost = turn_baseline_cached_cost + turn_baseline_uncached_cost;
    let baseline_cost_usd = turn_baseline_cost * (horizon as f64);

    assumptions.push(format!(
        "Horizon: {horizon} turns; baseline assumes ~{total_tokens} tokens ({cached_tokens} cached, {uncached_tokens} uncached) per turn."
    ));

    // Optimized payload size
    let opt_total_tokens = if saved_tokens >= 0 {
        total_tokens.saturating_sub(saved_tokens as u32)
    } else {
        total_tokens.saturating_add((-saved_tokens) as u32)
    };

    // Calculate Turn 1 optimized cost based on cache state and KV break
    let (turn1_cached, turn1_uncached, turn1_write) = if is_append_only {
        // Pure append-only operation (e.g. PinContext):
        // Historical prefix is 100% preserved. Turn 1 retains all previously cached tokens!
        let retained_cached = cached_tokens;
        let uncached_tail = opt_total_tokens.saturating_sub(retained_cached);
        (retained_cached, uncached_tail, 0u32)
    } else {
        match cache_obs.state {
            CacheState::WarmVerified => {
                // Prefix up to earliest_kv_break_start remains cached; remainder must be re-read uncached!
                let break_start = earliest_kv_break_start.unwrap_or(0);
                let preserved_cached = cached_tokens.min(break_start);
                let invalidated_uncached = opt_total_tokens.saturating_sub(preserved_cached);
                let write_tokens = if profile.cache_write_per_mtok > 0.0 {
                    invalidated_uncached
                } else {
                    0
                };
                reasons.push(format!(
                    "Warm cache prefix broken at token {break_start}; {invalidated_uncached} tokens must be re-read as uncached input."
                ));
                (preserved_cached, invalidated_uncached, write_tokens)
            }
            CacheState::ColdLikely => {
                // Cache is cold anyway: zero cached tokens, all tokens read at uncached rate.
                // No KV break penalty since there was no cache hit to lose!
                reasons.push("Cold cache: token reduction directly saves uncached input cost with zero KV penalty.".to_string());
                (0u32, opt_total_tokens, 0u32)
            }
            CacheState::Unknown | CacheState::Rebuilding => {
                // Conservative estimation
                let break_start = earliest_kv_break_start.unwrap_or(0);
                let preserved_cached = cached_tokens.min(break_start);
                let uncached_tail = opt_total_tokens.saturating_sub(preserved_cached);
                (preserved_cached, uncached_tail, 0u32)
            }
        }
    };

    let turn1_opt_cost = (turn1_cached as f64 / 1_000_000.0) * profile.cached_input_read_per_mtok
        + (turn1_uncached as f64 / 1_000_000.0) * profile.uncached_input_per_mtok
        + (turn1_write as f64 / 1_000_000.0) * profile.cache_write_per_mtok;

    // Subsequent turns (turns 2..H): assuming compacted prefix of opt_total_tokens is cached
    let sub_turn_cost = if horizon > 1 {
        let sub_cached = opt_total_tokens;
        (sub_cached as f64 / 1_000_000.0) * profile.cached_input_read_per_mtok
    } else {
        0.0
    };

    let optimized_cost_usd = turn1_opt_cost + (sub_turn_cost * ((horizon - 1) as f64));
    let net_benefit_usd = baseline_cost_usd - optimized_cost_usd;
    let benefit_pct = if baseline_cost_usd > 0.0 {
        ((net_benefit_usd / baseline_cost_usd) * 100.0) as f32
    } else {
        0.0
    };

    // Calculate break-even turn count
    let mut break_even_turns = None;
    let mut cum_baseline = 0.0;
    let mut cum_optimized = 0.0;
    for t in 1..=horizon.max(10) {
        let t_base = turn_baseline_cost;
        let t_opt = if t == 1 {
            turn1_opt_cost
        } else {
            (opt_total_tokens as f64 / 1_000_000.0) * profile.cached_input_read_per_mtok
        };
        cum_baseline += t_base;
        cum_optimized += t_opt;
        if cum_optimized <= cum_baseline {
            break_even_turns = Some(t);
            break;
        }
    }

    let confidence = match (cache_obs.provenance, cache_obs.state) {
        (
            TelemetryProvenance::ObservedProvider,
            CacheState::WarmVerified | CacheState::ColdLikely,
        ) => EvaluationConfidence::High,
        (TelemetryProvenance::ObservedProvider, _) | (TelemetryProvenance::LocalEstimate, _) => {
            EvaluationConfidence::Medium
        }
        _ => EvaluationConfidence::Low,
    };

    EconomicEvaluation {
        baseline_cost_usd,
        optimized_cost_usd,
        net_benefit_usd,
        benefit_pct,
        break_even_turns,
        horizon_turns: horizon,
        confidence,
        assumptions,
        reasons,
    }
}

/// Explicit decision outcome for an optimization candidate or plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OptimizationDecision {
    Apply,
    Defer,
    Reject,
    NoOp,
}

/// Categorized reason code justifying the optimization decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionReasonCode {
    /// Economic net benefit verified with cache safety.
    EconomicBenefitVerified,
    /// Cache prefix at risk: warm cache would suffer costly invalidation.
    CachePrefixAtRisk,
    /// Telemetry absent, unverified, or ambiguous.
    CacheTelemetryUnknown,
    /// Candidate is append-only tail operation preserving prefix, but has no standalone token reduction.
    TailAppendPreservesPrefixNoDirectSaving,
    /// Candidate has no net positive token savings or economic benefit.
    NoDemonstrableBenefit,
    /// Unresolved or active tool call/result lifecycle pairing.
    ActiveToolLifecycleProtection,
    /// Incomplete context analysis or mutation safety check failure.
    IncompleteContextAnalysis,
    /// Unsigned content or safety invariant violation.
    SafetyViolation,
    /// Pricing model missing or unsupported for provider/model.
    MissingPricingProfile,
    /// Cache currently rebuilding after recent intentional compaction.
    CacheRebuilding,
    /// Multiple overlapping candidates create prohibitive invalidation.
    OverlappingBreakExceedsBenefit,
}

/// Evidence provenance supporting the policy decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionEvidence {
    pub session_id: String,
    pub turn_seq: u32,
    pub provider: ProviderKind,
    pub model: String,
    pub telemetry_provenance: TelemetryProvenance,
    pub reported_cached_tokens: Option<u32>,
    pub is_append_only: bool,
    pub lifecycle_safe: bool,
    pub analysis_complete: bool,
}

/// Explainable decision record produced by the policy engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyDecision {
    pub decision: OptimizationDecision,
    pub reason_code: DecisionReasonCode,
    pub explanation: String,
    pub candidate_id: Option<String>,
    pub plan_id: Option<String>,
    pub cache_state: CacheState,
    pub estimated_kv_break_tokens: u32,
    pub net_token_delta: i32,
    pub economic_evaluation: Option<EconomicEvaluation>,
    pub evidence: DecisionEvidence,
    pub timestamp: DateTime<Utc>,
}

impl PolicyDecision {
    /// Formats a human-readable diagnostic summary block.
    pub fn compact_diagnostic_summary(&self) -> String {
        format!(
            "Candidate: {}\nCache state: {:?}\nEstimated KV break: {} tokens\nDecision: {:?}\nReason: {:?}\nCost confidence: {:?}\nActual mutation applied by shadow policy: false",
            self.candidate_id.as_deref().unwrap_or("Plan"),
            self.cache_state,
            self.estimated_kv_break_tokens,
            self.decision,
            self.reason_code,
            self.economic_evaluation
                .as_ref()
                .map(|e| e.confidence)
                .unwrap_or(EvaluationConfidence::Unverified)
        )
    }
}

/// Evaluates an individual candidate or complete plan against cache state and invariants.
pub fn evaluate_decision(
    candidate_id: Option<&str>,
    plan_id: Option<&str>,
    op: &MutationOp,
    turn: &ContextTurn,
    cache_obs: &CacheObservation,
    pricing: Option<&ModelPricingProfile>,
    horizon_turns: u32,
) -> PolicyDecision {
    let session_id = turn.session_id.clone();
    let turn_seq = turn.sequence_number;
    let provider = cache_obs.provider;
    let model = cache_obs.model.clone();
    let is_append = op.is_append_only();

    // 1. Incomplete context analysis -> Reject
    if !turn.analysis_complete {
        return PolicyDecision {
            decision: OptimizationDecision::Reject,
            reason_code: DecisionReasonCode::IncompleteContextAnalysis,
            explanation: "ContextTurn analysis is incomplete or unsupported for payload."
                .to_string(),
            candidate_id: candidate_id.map(ToString::to_string),
            plan_id: plan_id.map(ToString::to_string),
            cache_state: cache_obs.state,
            estimated_kv_break_tokens: 0,
            net_token_delta: 0,
            economic_evaluation: None,
            evidence: DecisionEvidence {
                session_id,
                turn_seq,
                provider,
                model,
                telemetry_provenance: cache_obs.provenance,
                reported_cached_tokens: cache_obs.observed_cached_tokens,
                is_append_only: is_append,
                lifecycle_safe: false,
                analysis_complete: false,
            },
            timestamp: Utc::now(),
        };
    }

    // 2. Lifecycle & Safety Check: verify candidate does not target protected tool results or forbidden node kinds
    let last_assistant_idx = turn
        .nodes
        .iter()
        .rposition(|n| matches!(n.kind, NodeKind::AssistantText { .. }))
        .unwrap_or(0);

    let mut targets_protected_tool = false;
    let mut targets_forbidden_kind = false;
    let mut target_node_start: Option<u32> = None;

    match op {
        MutationOp::PruneOrStub {
            target_fingerprint,
            target_call_id,
            target_json_pointer,
            ..
        } => {
            for (idx, node) in turn.nodes.iter().enumerate() {
                let matches = target_fingerprint
                    .as_ref()
                    .map(|f| f == &node.fingerprint.0)
                    .unwrap_or(false)
                    || target_json_pointer
                        .as_ref()
                        .map(|p| p == &node.raw_json_pointer)
                        .unwrap_or(false)
                    || match (&node.kind, target_call_id.as_ref()) {
                        (NodeKind::ToolResult { call_id, .. }, Some(tc)) => call_id == tc,
                        _ => false,
                    };
                if matches {
                    target_node_start = Some(node.token_span.start);
                    if matches!(
                        node.kind,
                        NodeKind::UserMessage { .. }
                            | NodeKind::SystemPrompt { .. }
                            | NodeKind::AssistantText { .. }
                            | NodeKind::ToolCall { .. }
                    ) {
                        targets_forbidden_kind = true;
                    }
                    if is_protected_tool_result(node, idx, turn.nodes.len(), last_assistant_idx) {
                        targets_protected_tool = true;
                    }
                }
            }
        }
        MutationOp::FoldSubtree {
            start_turn,
            end_turn,
            ..
        } => {
            for (idx, node) in turn.nodes.iter().enumerate() {
                if node.turn_seq >= *start_turn && node.turn_seq <= *end_turn {
                    if target_node_start.is_none() {
                        target_node_start = Some(node.token_span.start);
                    }
                    if is_protected_tool_result(node, idx, turn.nodes.len(), last_assistant_idx) {
                        targets_protected_tool = true;
                    }
                }
            }
        }
        _ => {}
    }

    if targets_forbidden_kind {
        return PolicyDecision {
            decision: OptimizationDecision::Reject,
            reason_code: DecisionReasonCode::SafetyViolation,
            explanation: "Candidate targets protected node kind (UserPrompt, SystemPrompt, or AssistantText).".to_string(),
            candidate_id: candidate_id.map(ToString::to_string),
            plan_id: plan_id.map(ToString::to_string),
            cache_state: cache_obs.state,
            estimated_kv_break_tokens: 0,
            net_token_delta: 0,
            economic_evaluation: None,
            evidence: DecisionEvidence {
                session_id,
                turn_seq,
                provider,
                model,
                telemetry_provenance: cache_obs.provenance,
                reported_cached_tokens: cache_obs.observed_cached_tokens,
                is_append_only: is_append,
                lifecycle_safe: false,
                analysis_complete: true,
            },
            timestamp: Utc::now(),
        };
    }

    if targets_protected_tool {
        return PolicyDecision {
            decision: OptimizationDecision::Defer,
            reason_code: DecisionReasonCode::ActiveToolLifecycleProtection,
            explanation:
                "Candidate targets unread or active tool result awaiting assistant response."
                    .to_string(),
            candidate_id: candidate_id.map(ToString::to_string),
            plan_id: plan_id.map(ToString::to_string),
            cache_state: cache_obs.state,
            estimated_kv_break_tokens: 0,
            net_token_delta: 0,
            economic_evaluation: None,
            evidence: DecisionEvidence {
                session_id,
                turn_seq,
                provider,
                model,
                telemetry_provenance: cache_obs.provenance,
                reported_cached_tokens: cache_obs.observed_cached_tokens,
                is_append_only: is_append,
                lifecycle_safe: false,
                analysis_complete: true,
            },
            timestamp: Utc::now(),
        };
    }

    // 3. Structural Impact: estimate saved tokens and KV break
    let (saved_tokens, kv_break_tokens) =
        crate::mutation::estimate_mutation_impact(op, &turn.nodes, Some(&turn.kv_cache_report));
    let total_tokens = turn.token_usage.input_tokens;

    // 4. Append-Only Tail Operations (PinContext):
    // Preserves prefix bit-for-bit (0 KV break), but has no direct standalone token reduction.
    if is_append {
        let econ = evaluate_economic_impact(
            total_tokens,
            saved_tokens,
            None,
            true,
            cache_obs,
            pricing,
            horizon_turns,
        );
        return PolicyDecision {
            decision: OptimizationDecision::NoOp,
            reason_code: DecisionReasonCode::TailAppendPreservesPrefixNoDirectSaving,
            explanation: "Tail append operation preserves prefix bit-for-bit (0 KV break) but yields no standalone token reduction without behavioral steering.".to_string(),
            candidate_id: candidate_id.map(ToString::to_string),
            plan_id: plan_id.map(ToString::to_string),
            cache_state: cache_obs.state,
            estimated_kv_break_tokens: 0,
            net_token_delta: saved_tokens,
            economic_evaluation: Some(econ),
            evidence: DecisionEvidence {
                session_id,
                turn_seq,
                provider,
                model,
                telemetry_provenance: cache_obs.provenance,
                reported_cached_tokens: cache_obs.observed_cached_tokens,
                is_append_only: true,
                lifecycle_safe: true,
                analysis_complete: true,
            },
            timestamp: Utc::now(),
        };
    }

    // 5. Historical Prefix Compaction (Prune/Fold/Dedupe):
    // Evaluate based on cache state:
    match cache_obs.state {
        CacheState::WarmVerified => {
            let econ = evaluate_economic_impact(
                total_tokens,
                saved_tokens,
                target_node_start,
                false,
                cache_obs,
                pricing,
                horizon_turns,
            );
            // In warm verified cache, historical prefix break invalidates cached tokens.
            PolicyDecision {
                decision: OptimizationDecision::Defer,
                reason_code: DecisionReasonCode::CachePrefixAtRisk,
                explanation: format!(
                    "Active warm cache prefix would be invalidated by historical mutation ({kv_break_tokens} tokens at risk). Deferring to preserve cache hits."
                ),
                candidate_id: candidate_id.map(ToString::to_string),
                plan_id: plan_id.map(ToString::to_string),
                cache_state: CacheState::WarmVerified,
                estimated_kv_break_tokens: kv_break_tokens,
                net_token_delta: saved_tokens,
                economic_evaluation: Some(econ),
                evidence: DecisionEvidence {
                    session_id,
                    turn_seq,
                    provider,
                    model,
                    telemetry_provenance: cache_obs.provenance,
                    reported_cached_tokens: cache_obs.observed_cached_tokens,
                    is_append_only: false,
                    lifecycle_safe: true,
                    analysis_complete: true,
                },
                timestamp: Utc::now(),
            }
        }
        CacheState::Unknown => {
            let econ = evaluate_economic_impact(
                total_tokens,
                saved_tokens,
                target_node_start,
                false,
                cache_obs,
                pricing,
                horizon_turns,
            );
            PolicyDecision {
                decision: OptimizationDecision::Defer,
                reason_code: DecisionReasonCode::CacheTelemetryUnknown,
                explanation: "Cache telemetry is absent, unverified, or ambiguous; deferring historical mutation conservatively.".to_string(),
                candidate_id: candidate_id.map(ToString::to_string),
                plan_id: plan_id.map(ToString::to_string),
                cache_state: CacheState::Unknown,
                estimated_kv_break_tokens: kv_break_tokens,
                net_token_delta: saved_tokens,
                economic_evaluation: Some(econ),
                evidence: DecisionEvidence {
                    session_id,
                    turn_seq,
                    provider,
                    model,
                    telemetry_provenance: cache_obs.provenance,
                    reported_cached_tokens: cache_obs.observed_cached_tokens,
                    is_append_only: false,
                    lifecycle_safe: true,
                    analysis_complete: true,
                },
                timestamp: Utc::now(),
            }
        }
        CacheState::Rebuilding => {
            let econ = evaluate_economic_impact(
                total_tokens,
                saved_tokens,
                target_node_start,
                false,
                cache_obs,
                pricing,
                horizon_turns,
            );
            PolicyDecision {
                decision: OptimizationDecision::Defer,
                reason_code: DecisionReasonCode::CacheRebuilding,
                explanation: "Cache prefix is actively rebuilding from prior compaction; deferring additional historical edits.".to_string(),
                candidate_id: candidate_id.map(ToString::to_string),
                plan_id: plan_id.map(ToString::to_string),
                cache_state: CacheState::Rebuilding,
                estimated_kv_break_tokens: kv_break_tokens,
                net_token_delta: saved_tokens,
                economic_evaluation: Some(econ),
                evidence: DecisionEvidence {
                    session_id,
                    turn_seq,
                    provider,
                    model,
                    telemetry_provenance: cache_obs.provenance,
                    reported_cached_tokens: cache_obs.observed_cached_tokens,
                    is_append_only: false,
                    lifecycle_safe: true,
                    analysis_complete: true,
                },
                timestamp: Utc::now(),
            }
        }
        CacheState::ColdLikely => {
            let econ = evaluate_economic_impact(
                total_tokens,
                saved_tokens,
                target_node_start,
                false,
                cache_obs,
                pricing,
                horizon_turns,
            );
            if pricing.is_none() || !pricing.map(|p| p.is_supported).unwrap_or(false) {
                return PolicyDecision {
                    decision: OptimizationDecision::Defer,
                    reason_code: DecisionReasonCode::MissingPricingProfile,
                    explanation: "Pricing profile missing or unsupported; cannot authorize cold cache optimization.".to_string(),
                    candidate_id: candidate_id.map(ToString::to_string),
                    plan_id: plan_id.map(ToString::to_string),
                    cache_state: CacheState::ColdLikely,
                    estimated_kv_break_tokens: kv_break_tokens,
                    net_token_delta: saved_tokens,
                    economic_evaluation: Some(econ),
                    evidence: DecisionEvidence {
                        session_id,
                        turn_seq,
                        provider,
                        model,
                        telemetry_provenance: cache_obs.provenance,
                        reported_cached_tokens: cache_obs.observed_cached_tokens,
                        is_append_only: false,
                        lifecycle_safe: true,
                        analysis_complete: true,
                    },
                    timestamp: Utc::now(),
                };
            }

            if saved_tokens > 0 && econ.net_benefit_usd > 0.0 {
                PolicyDecision {
                    decision: OptimizationDecision::Apply,
                    reason_code: DecisionReasonCode::EconomicBenefitVerified,
                    explanation: format!(
                        "Cold cache verified: token reduction (saving {saved_tokens} tokens, ${:.5} USD) carries zero cache penalty.",
                        econ.net_benefit_usd
                    ),
                    candidate_id: candidate_id.map(ToString::to_string),
                    plan_id: plan_id.map(ToString::to_string),
                    cache_state: CacheState::ColdLikely,
                    estimated_kv_break_tokens: kv_break_tokens,
                    net_token_delta: saved_tokens,
                    economic_evaluation: Some(econ),
                    evidence: DecisionEvidence {
                        session_id,
                        turn_seq,
                        provider,
                        model,
                        telemetry_provenance: cache_obs.provenance,
                        reported_cached_tokens: cache_obs.observed_cached_tokens,
                        is_append_only: false,
                        lifecycle_safe: true,
                        analysis_complete: true,
                    },
                    timestamp: Utc::now(),
                }
            } else {
                PolicyDecision {
                    decision: OptimizationDecision::NoOp,
                    reason_code: DecisionReasonCode::NoDemonstrableBenefit,
                    explanation: "Candidate yields non-positive token savings or non-positive economic benefit.".to_string(),
                    candidate_id: candidate_id.map(ToString::to_string),
                    plan_id: plan_id.map(ToString::to_string),
                    cache_state: CacheState::ColdLikely,
                    estimated_kv_break_tokens: kv_break_tokens,
                    net_token_delta: saved_tokens,
                    economic_evaluation: Some(econ),
                    evidence: DecisionEvidence {
                        session_id,
                        turn_seq,
                        provider,
                        model,
                        telemetry_provenance: cache_obs.provenance,
                        reported_cached_tokens: cache_obs.observed_cached_tokens,
                        is_append_only: false,
                        lifecycle_safe: true,
                        analysis_complete: true,
                    },
                    timestamp: Utc::now(),
                }
            }
        }
    }
}

/// Evaluates a multi-candidate optimization plan, accounting for overlapping KV breaks.
pub fn evaluate_plan_decisions(
    plan: &crate::optimizer::OptimizationStrategyPlan,
    turn: &ContextTurn,
    cache_obs: &CacheObservation,
    pricing: Option<&ModelPricingProfile>,
    horizon_turns: u32,
) -> (PolicyDecision, Vec<PolicyDecision>) {
    let mut candidate_decisions = Vec::new();
    let mut earliest_break_start: Option<u32> = None;
    let mut total_saved = 0i32;

    for item in &plan.items {
        let dec = evaluate_decision(
            Some(&item.item_id),
            Some(&plan.id),
            &item.mutation_op,
            turn,
            cache_obs,
            pricing,
            horizon_turns,
        );
        if !item.mutation_op.is_append_only() {
            // Find earliest start token for this candidate
            let item_start = turn
                .nodes
                .iter()
                .find(|n| item.node_id.as_deref() == Some(&n.id))
                .map(|n| n.token_span.start);
            if let Some(s) = item_start {
                earliest_break_start = Some(earliest_break_start.map(|e| e.min(s)).unwrap_or(s));
            }
        }
        total_saved += item.saved_tokens;
        candidate_decisions.push(dec);
    }

    // Evaluate plan as a whole with combined earliest break start
    let is_pure_append = plan.items.iter().all(|i| i.mutation_op.is_append_only());
    let econ = evaluate_economic_impact(
        turn.token_usage.input_tokens,
        total_saved,
        earliest_break_start,
        is_pure_append,
        cache_obs,
        pricing,
        horizon_turns,
    );

    let plan_decision = if candidate_decisions
        .iter()
        .any(|d| d.decision == OptimizationDecision::Reject)
    {
        PolicyDecision {
            decision: OptimizationDecision::Reject,
            reason_code: DecisionReasonCode::SafetyViolation,
            explanation: "Plan contains one or more rejected candidate operations.".to_string(),
            candidate_id: None,
            plan_id: Some(plan.id.clone()),
            cache_state: cache_obs.state,
            estimated_kv_break_tokens: plan.kv_cache_break_tokens,
            net_token_delta: total_saved,
            economic_evaluation: Some(econ),
            evidence: DecisionEvidence {
                session_id: turn.session_id.clone(),
                turn_seq: turn.sequence_number,
                provider: cache_obs.provider,
                model: cache_obs.model.clone(),
                telemetry_provenance: cache_obs.provenance,
                reported_cached_tokens: cache_obs.observed_cached_tokens,
                is_append_only: is_pure_append,
                lifecycle_safe: false,
                analysis_complete: turn.analysis_complete,
            },
            timestamp: Utc::now(),
        }
    } else if cache_obs.state == CacheState::WarmVerified && !is_pure_append {
        PolicyDecision {
            decision: OptimizationDecision::Defer,
            reason_code: DecisionReasonCode::CachePrefixAtRisk,
            explanation: format!(
                "Plan contains historical prefix mutations that risk invalidating active warm cache ({earliest_break_start:?} earliest token)."
            ),
            candidate_id: None,
            plan_id: Some(plan.id.clone()),
            cache_state: CacheState::WarmVerified,
            estimated_kv_break_tokens: plan.kv_cache_break_tokens,
            net_token_delta: total_saved,
            economic_evaluation: Some(econ),
            evidence: DecisionEvidence {
                session_id: turn.session_id.clone(),
                turn_seq: turn.sequence_number,
                provider: cache_obs.provider,
                model: cache_obs.model.clone(),
                telemetry_provenance: cache_obs.provenance,
                reported_cached_tokens: cache_obs.observed_cached_tokens,
                is_append_only: is_pure_append,
                lifecycle_safe: true,
                analysis_complete: turn.analysis_complete,
            },
            timestamp: Utc::now(),
        }
    } else if cache_obs.state == CacheState::ColdLikely
        && total_saved > 0
        && econ.net_benefit_usd > 0.0
    {
        PolicyDecision {
            decision: OptimizationDecision::Apply,
            reason_code: DecisionReasonCode::EconomicBenefitVerified,
            explanation: format!(
                "Cold cache verified: plan saves {total_saved} tokens (${:.5} USD) without cache penalty.",
                econ.net_benefit_usd
            ),
            candidate_id: None,
            plan_id: Some(plan.id.clone()),
            cache_state: CacheState::ColdLikely,
            estimated_kv_break_tokens: plan.kv_cache_break_tokens,
            net_token_delta: total_saved,
            economic_evaluation: Some(econ),
            evidence: DecisionEvidence {
                session_id: turn.session_id.clone(),
                turn_seq: turn.sequence_number,
                provider: cache_obs.provider,
                model: cache_obs.model.clone(),
                telemetry_provenance: cache_obs.provenance,
                reported_cached_tokens: cache_obs.observed_cached_tokens,
                is_append_only: is_pure_append,
                lifecycle_safe: true,
                analysis_complete: turn.analysis_complete,
            },
            timestamp: Utc::now(),
        }
    } else if is_pure_append {
        PolicyDecision {
            decision: OptimizationDecision::NoOp,
            reason_code: DecisionReasonCode::TailAppendPreservesPrefixNoDirectSaving,
            explanation:
                "Plan contains only prefix-preserving tail pins with 0 direct token reduction."
                    .to_string(),
            candidate_id: None,
            plan_id: Some(plan.id.clone()),
            cache_state: cache_obs.state,
            estimated_kv_break_tokens: 0,
            net_token_delta: total_saved,
            economic_evaluation: Some(econ),
            evidence: DecisionEvidence {
                session_id: turn.session_id.clone(),
                turn_seq: turn.sequence_number,
                provider: cache_obs.provider,
                model: cache_obs.model.clone(),
                telemetry_provenance: cache_obs.provenance,
                reported_cached_tokens: cache_obs.observed_cached_tokens,
                is_append_only: true,
                lifecycle_safe: true,
                analysis_complete: turn.analysis_complete,
            },
            timestamp: Utc::now(),
        }
    } else {
        PolicyDecision {
            decision: OptimizationDecision::Defer,
            reason_code: DecisionReasonCode::CacheTelemetryUnknown,
            explanation: "Cache telemetry absent or insufficient to guarantee economic benefit."
                .to_string(),
            candidate_id: None,
            plan_id: Some(plan.id.clone()),
            cache_state: cache_obs.state,
            estimated_kv_break_tokens: plan.kv_cache_break_tokens,
            net_token_delta: total_saved,
            economic_evaluation: Some(econ),
            evidence: DecisionEvidence {
                session_id: turn.session_id.clone(),
                turn_seq: turn.sequence_number,
                provider: cache_obs.provider,
                model: cache_obs.model.clone(),
                telemetry_provenance: cache_obs.provenance,
                reported_cached_tokens: cache_obs.observed_cached_tokens,
                is_append_only: is_pure_append,
                lifecycle_safe: true,
                analysis_complete: turn.analysis_complete,
            },
            timestamp: Utc::now(),
        }
    };

    (plan_decision, candidate_decisions)
}

/// Shadow-mode configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShadowPolicyConfig {
    pub enabled: bool,
    pub horizon_turns: u32,
    pub record_diagnostics: bool,
}

impl Default for ShadowPolicyConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            horizon_turns: 3,
            record_diagnostics: true,
        }
    }
}

/// Shadow-mode evaluator: evaluates plans and candidates, recording explainable decisions
/// without committing or applying any mutations.
pub fn evaluate_shadow_policy(
    config: &ShadowPolicyConfig,
    turn: &ContextTurn,
    cache_obs: &CacheObservation,
    pricing: Option<&ModelPricingProfile>,
    plans: &[crate::optimizer::OptimizationStrategyPlan],
) -> Vec<PolicyDecision> {
    if !config.enabled {
        return Vec::new();
    }

    let mut recorded_decisions = Vec::new();
    for plan in plans {
        let (plan_dec, item_decs) =
            evaluate_plan_decisions(plan, turn, cache_obs, pricing, config.horizon_turns);
        recorded_decisions.push(plan_dec);
        recorded_decisions.extend(item_decs);
    }
    recorded_decisions
}
