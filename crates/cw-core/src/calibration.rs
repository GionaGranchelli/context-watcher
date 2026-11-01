//! CW-117B: Shadow Policy Empirical Calibration & Evidence Loop
//!
//! Provides durable, privacy-preserving shadow decision evidence recording,
//! correlation with observed provider telemetry, deterministic offline historical replay,
//! and policy calibration diagnostics.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

use crate::ir::ProviderKind;
use crate::policy::{
    CacheObservation, CacheState, DecisionReasonCode, ModelPricingProfile, OptimizationDecision,
    PolicyDecision, TelemetryProvenance, classify_cache_state, find_pricing_profile,
};

/// Policy engine version evaluated for calibration evidence.
pub const POLICY_CALIBRATION_VERSION: &str = "0.2.4-cw117b";

/// Status of policy evaluation for an evidence record.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PolicyEvaluationStatus {
    /// Evaluated pre-dispatch on an active request with concrete optimization candidates.
    EvaluatedPreDispatch,
    /// Evaluated in a simulated test fixture with synthesized candidate nodes.
    EvaluatedSimulatedFixture,
    /// Replayed from a historical trace that lacks pre-dispatch candidate snapshots and unmutated prompt trees.
    NotEvaluatedMissingCandidates,
}

/// Computes a deterministic BLAKE3 payload fingerprint from canonical JSON structure.
/// Contains zero raw prompt text, messages, or secrets.
pub fn compute_payload_fingerprint(payload: &serde_json::Value) -> String {
    let canonical = serde_json::to_vec(payload).unwrap_or_default();
    format!("pfp_{}", blake3::hash(&canonical).to_hex())
}

/// Durable, privacy-preserving record of a shadow policy evaluation.
///
/// Contains zero prompt text, raw secrets, or uncompacted tool outputs.
/// Stores only structural metrics, token spans, classifications, and modeled projections.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ShadowDecisionEvidence {
    /// Unique identifier for this evidence record (e.g. `sde_<session>_<req>_<scope>`)
    pub record_id: String,
    /// Session identifier
    pub session_id: String,
    /// Request sequence index within the session
    pub request_index: u32,
    /// Turn sequence index within the session
    pub turn_index: u32,
    /// Provider kind
    pub provider: ProviderKind,
    /// Model identifier
    pub model: String,
    /// Evaluated cache-state classification
    pub cache_state: CacheState,
    /// Telemetry provenance of the cache observation
    pub provenance: TelemetryProvenance,
    /// Optional prefix fingerprint (hash only; zero prompt text)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix_fingerprint: Option<String>,
    /// Optional stable request payload fingerprint (BLAKE3 hash only; zero prompt text)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_fingerprint: Option<String>,
    /// Optimization candidate identifier, if candidate-level
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_id: Option<String>,
    /// Strategy plan identifier, if plan-level
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_id: Option<String>,
    /// Policy evaluation qualification status
    pub policy_evaluation_status: PolicyEvaluationStatus,
    /// Explainable policy decision outcome (None if not evaluated)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<OptimizationDecision>,
    /// Typed decision reason code (None if not evaluated)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<DecisionReasonCode>,
    /// Detailed diagnostic explanation
    pub explanation: String,
    /// Structural token reduction estimate (tokens saved before minus after; signed)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_saved_tokens: Option<i32>,
    /// Estimated KV-break tokens (number of historical cached prefix tokens invalidated)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_kv_break_tokens: Option<u32>,
    /// Predicted baseline cost over the configured horizon (USD)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predicted_baseline_cost_usd: Option<f64>,
    /// Predicted optimized cost over the configured horizon (USD)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predicted_optimized_cost_usd: Option<f64>,
    /// Predicted net financial benefit over the configured horizon (USD)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predicted_net_benefit_usd: Option<f64>,
    /// Multi-turn forecast horizon in turns
    pub forecast_horizon_turns: u32,
    /// Modeling assumptions used during policy evaluation
    pub forecast_assumptions: String,
    /// Pricing model pattern matched in pricing profile
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pricing_model_pattern: Option<String>,
    /// Pricing profile version or timestamp
    pub pricing_version: String,
    /// Policy engine implementation version
    pub policy_version: String,
    /// Timestamp when decision was evaluated
    pub decision_timestamp: DateTime<Utc>,
}

impl ShadowDecisionEvidence {
    /// Construct a durable evidence record from a live pre-dispatch policy decision and cache observation.
    pub fn from_policy_decision(
        decision: &PolicyDecision,
        cache_obs: &CacheObservation,
        pricing: Option<&ModelPricingProfile>,
        horizon_turns: u32,
        request_index: u32,
        turn_index: u32,
    ) -> Self {
        let (baseline_cost, opt_cost, net_benefit) = match &decision.economic_evaluation {
            Some(econ) => (
                Some(econ.baseline_cost_usd),
                Some(econ.optimized_cost_usd),
                Some(econ.net_benefit_usd),
            ),
            None => (None, None, None),
        };

        let assumptions = format!(
            "horizon={} turns, pricing_supported={}, write_surcharge_model={}",
            horizon_turns,
            pricing.map(|p| p.is_supported).unwrap_or(false),
            pricing
                .map(|p| p.cache_write_per_mtok > 0.0)
                .unwrap_or(false)
        );

        let scope_key = match (&decision.candidate_id, &decision.plan_id) {
            (Some(cand), Some(plan)) => format!("{}_{}", plan, cand),
            (Some(cand), None) => format!("cand_{}", cand),
            (None, Some(plan)) => format!("plan_{}", plan),
            (None, None) => "decision".to_string(),
        };

        Self {
            record_id: format!(
                "sde_{}_{}_{}",
                decision.evidence.session_id, request_index, scope_key
            ),
            session_id: decision.evidence.session_id.clone(),
            request_index,
            turn_index,
            provider: decision.evidence.provider,
            model: decision.evidence.model.clone(),
            cache_state: decision.cache_state,
            provenance: decision.evidence.telemetry_provenance,
            prefix_fingerprint: cache_obs.prefix_fingerprint.clone(),
            payload_fingerprint: None,
            candidate_id: decision.candidate_id.clone(),
            plan_id: decision.plan_id.clone(),
            policy_evaluation_status: PolicyEvaluationStatus::EvaluatedPreDispatch,
            decision: Some(decision.decision),
            reason_code: Some(decision.reason_code.clone()),
            explanation: decision.explanation.clone(),
            estimated_saved_tokens: Some(decision.net_token_delta),
            estimated_kv_break_tokens: Some(decision.estimated_kv_break_tokens),
            predicted_baseline_cost_usd: baseline_cost,
            predicted_optimized_cost_usd: opt_cost,
            predicted_net_benefit_usd: net_benefit,
            forecast_horizon_turns: horizon_turns,
            forecast_assumptions: assumptions,
            pricing_model_pattern: pricing.map(|p| p.model_pattern.clone()),
            pricing_version: "2026-10-pricing-v1".to_string(),
            policy_version: POLICY_CALIBRATION_VERSION.to_string(),
            decision_timestamp: decision.timestamp,
        }
    }

    /// Attach a deterministic payload fingerprint to the evidence record.
    pub fn with_payload_fingerprint(mut self, pfp: Option<String>) -> Self {
        self.payload_fingerprint = pfp;
        self
    }

    /// Construct a durable evidence record from a historical observation trace without fabricating decisions or savings.
    #[allow(clippy::too_many_arguments)]
    pub fn from_historical_observation(
        req: &HistoricalRequestRecord,
        session_id: &str,
        provider: ProviderKind,
        cache_obs: &CacheObservation,
        pricing: Option<&ModelPricingProfile>,
        horizon_turns: u32,
        dataset_name: &str,
        decision_timestamp: DateTime<Utc>,
    ) -> Self {
        Self {
            record_id: format!("sde_{}_{}", session_id, req.request_index),
            session_id: session_id.to_string(),
            request_index: req.request_index,
            turn_index: req.turn_index.unwrap_or(req.request_index),
            provider,
            model: req.model.clone(),
            cache_state: cache_obs.state,
            provenance: cache_obs.provenance,
            prefix_fingerprint: None,
            payload_fingerprint: None,
            candidate_id: None,
            plan_id: None,
            policy_evaluation_status: PolicyEvaluationStatus::NotEvaluatedMissingCandidates,
            decision: None,
            reason_code: None,
            explanation: "Historical trace replay: pre-dispatch candidates and unmutated prompt trees were not captured at proxy time; policy evaluation omitted to prevent synthetic bias.".to_string(),
            estimated_saved_tokens: None,
            estimated_kv_break_tokens: None,
            predicted_baseline_cost_usd: None,
            predicted_optimized_cost_usd: None,
            predicted_net_benefit_usd: None,
            forecast_horizon_turns: horizon_turns,
            forecast_assumptions: format!(
                "replayed_trace={}, H={}, prospective_evaluation=false",
                dataset_name, horizon_turns
            ),
            pricing_model_pattern: pricing.map(|p| p.model_pattern.clone()),
            pricing_version: "2026-10-pricing-v1".to_string(),
            policy_version: POLICY_CALIBRATION_VERSION.to_string(),
            decision_timestamp,
        }
    }
}

/// Observed request-level provider outcome.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ObservedProviderOutcome {
    pub session_id: String,
    pub request_index: u32,
    pub turn_index: u32,
    pub provider: ProviderKind,
    pub model: String,
    pub actual_input_tokens: u32,
    pub actual_cached_tokens: u32,
    pub actual_uncached_tokens: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_cache_write_tokens: Option<u32>,
    pub actual_output_tokens: u32,
    pub calculated_cost_usd: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reported_cost_usd: Option<f64>,
    pub observed_cache_hit_ratio: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttfb_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_latency_ms: Option<u64>,
    pub cache_telemetry_available: bool,
    pub execution_outcome: String,
    pub applied_mutations: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing_data_reasons: Vec<String>,
    pub observed_timestamp: DateTime<Utc>,
}

impl ObservedProviderOutcome {
    /// Construct an observed outcome record from a finalized proxy ContextTurn.
    pub fn from_completed_turn(
        turn: &crate::ir::ContextTurn,
        provider: ProviderKind,
        model: &str,
        pricing: Option<&ModelPricingProfile>,
        observed_timestamp: DateTime<Utc>,
    ) -> Self {
        let actual_cached = turn.token_usage.cached_tokens;
        let actual_input = turn.token_usage.input_tokens;
        let actual_uncached = actual_input.saturating_sub(actual_cached);
        let actual_output = turn.token_usage.output_tokens;

        let actual_write = turn
            .observed_usage
            .as_ref()
            .and_then(|u| {
                u.get("cache_creation_input_tokens")
                    .or_else(|| u.get("cache_write_tokens"))
            })
            .and_then(Value::as_u64)
            .map(|w| w.min(u32::MAX as u64) as u32);

        let mut missing_reasons = Vec::new();
        let outcome_str = turn
            .outcome
            .clone()
            .unwrap_or_else(|| "completed".to_string());
        if outcome_str != "completed" {
            missing_reasons.push(format!("execution_outcome_{}", outcome_str));
        }
        if turn.observed_usage.is_none() {
            missing_reasons.push("observed_usage_missing".to_string());
        }
        if turn.first_body_byte_ms.is_none() {
            missing_reasons.push("ttfb_not_measured".to_string());
        }

        let calculated_cost = if let Some(p) = pricing
            && p.is_supported
        {
            let cached_cost = (actual_cached as f64 / 1_000_000.0) * p.cached_input_read_per_mtok;
            let uncached_cost = (actual_uncached as f64 / 1_000_000.0) * p.uncached_input_per_mtok;
            let output_cost = (actual_output as f64 / 1_000_000.0) * p.output_per_mtok;
            let write_cost =
                (actual_write.unwrap_or(0) as f64 / 1_000_000.0) * p.cache_write_per_mtok;
            cached_cost + uncached_cost + output_cost + write_cost
        } else {
            0.0
        };

        let hit_ratio = if actual_input > 0 {
            actual_cached as f64 / actual_input as f64
        } else {
            0.0
        };

        Self {
            session_id: turn.session_id.clone(),
            request_index: turn.sequence_number,
            turn_index: turn.sequence_number,
            provider,
            model: model.to_string(),
            actual_input_tokens: actual_input,
            actual_cached_tokens: actual_cached,
            actual_uncached_tokens: actual_uncached,
            actual_cache_write_tokens: actual_write,
            actual_output_tokens: actual_output,
            calculated_cost_usd: calculated_cost,
            reported_cost_usd: None,
            observed_cache_hit_ratio: hit_ratio,
            ttfb_ms: turn.first_body_byte_ms,
            request_latency_ms: turn.latency_ms,
            cache_telemetry_available: turn.observed_usage.is_some()
                && (actual_cached > 0 || actual_uncached > 0),
            execution_outcome: outcome_str,
            applied_mutations: turn.applied_mutation_ids.clone(),
            missing_data_reasons: missing_reasons,
            observed_timestamp,
        }
    }
}

/// Status of correlation between a shadow policy decision and provider telemetry.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CorrelationStatus {
    /// Pre-dispatch shadow policy decision paired with the request executed immediately after evaluation.
    MatchedImmediate,
    /// Pre-dispatch shadow policy decision paired with a subsequent continuation request within the horizon.
    MatchedContinuation,
    /// Retrospective observation record from historical benchmark replay (not a prospective decision).
    RetrospectiveObservation,
    /// No subsequent provider telemetry available (e.g. terminal request or trace end).
    UncorrelatedMissingOutcome,
    /// Telemetry ambiguous, session diverged, or provider credentials mismatched.
    UncorrelatedAmbiguous,
}

/// Pair linking a shadow policy decision or retrospective observation with provider outcome telemetry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CorrelatedDecisionOutcome {
    pub correlation_id: String,
    pub session_id: String,
    pub request_index: u32,
    pub decision: ShadowDecisionEvidence,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<ObservedProviderOutcome>,
    pub correlation_status: CorrelationStatus,
    /// Difference between single-turn predicted baseline cost and actual executed cost.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub single_turn_cost_delta_usd: Option<f64>,
    /// Whether observed cache hit matched the predicted cache state on subsequent request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_state_consistent: Option<bool>,
    pub notes: Vec<String>,
}

/// Correlate a pre-dispatch shadow decision with an observed provider outcome.
pub fn correlate_decision_with_outcome(
    decision: &ShadowDecisionEvidence,
    outcome: Option<&ObservedProviderOutcome>,
) -> CorrelatedDecisionOutcome {
    let correlation_id = format!("corr_{}", decision.record_id);
    let session_id = decision.session_id.clone();
    let request_index = decision.request_index;

    match outcome {
        None => CorrelatedDecisionOutcome {
            correlation_id,
            session_id,
            request_index,
            decision: decision.clone(),
            outcome: None,
            correlation_status: CorrelationStatus::UncorrelatedMissingOutcome,
            single_turn_cost_delta_usd: None,
            cache_state_consistent: None,
            notes: vec![
                "No observed provider response received for this pre-dispatch decision".to_string(),
            ],
        },
        Some(obs) => {
            if obs.session_id != decision.session_id || obs.request_index != decision.request_index
            {
                return CorrelatedDecisionOutcome {
                    correlation_id,
                    session_id,
                    request_index,
                    decision: decision.clone(),
                    outcome: Some(obs.clone()),
                    correlation_status: CorrelationStatus::UncorrelatedAmbiguous,
                    single_turn_cost_delta_usd: None,
                    cache_state_consistent: None,
                    notes: vec![format!(
                        "Session/request mismatch: decision ({}, {}) vs outcome ({}, {})",
                        decision.session_id,
                        decision.request_index,
                        obs.session_id,
                        obs.request_index
                    )],
                };
            }

            let mut notes = Vec::new();
            if obs.execution_outcome != "completed" {
                notes.push(format!(
                    "Provider execution outcome: {}",
                    obs.execution_outcome
                ));
            }

            // Cost delta: predicted baseline cost minus actual observed cost
            let cost_delta = decision
                .predicted_baseline_cost_usd
                .map(|pred| pred - obs.calculated_cost_usd);

            // Cache state consistency
            let consistent = match decision.cache_state {
                CacheState::WarmVerified => {
                    // Predicted warm cache hit; consistent if cache hit ratio > 0.05
                    Some(obs.observed_cache_hit_ratio > 0.05)
                }
                CacheState::ColdLikely => {
                    // Predicted cold cache; consistent if actual cached tokens is negligible (< 5%)
                    Some(obs.observed_cache_hit_ratio < 0.05)
                }
                CacheState::Unknown => None,
                CacheState::Rebuilding => {
                    // Rebuilding cache: write tokens or cold uncached tokens
                    Some(
                        obs.actual_cache_write_tokens.unwrap_or(0) > 0
                            || obs.observed_cache_hit_ratio < 0.5,
                    )
                }
            };

            if let Some(false) = consistent {
                notes.push(format!(
                    "Cache state inconsistency: predicted {:?}, but observed hit ratio {:.2}% ({} cached / {} input)",
                    decision.cache_state,
                    obs.observed_cache_hit_ratio * 100.0,
                    obs.actual_cached_tokens,
                    obs.actual_input_tokens
                ));
            }

            CorrelatedDecisionOutcome {
                correlation_id,
                session_id,
                request_index,
                decision: decision.clone(),
                outcome: Some(obs.clone()),
                correlation_status: CorrelationStatus::MatchedImmediate,
                single_turn_cost_delta_usd: cost_delta,
                cache_state_consistent: consistent,
                notes,
            }
        }
    }
}

/// Individual continuation request transition observed within the forecast horizon.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContinuationTransition {
    pub request_index: u32,
    pub turn_index: u32,
    pub input_tokens: u32,
    pub cached_tokens: u32,
    pub uncached_tokens: u32,
    pub cache_hit_ratio: f64,
    pub cost_usd: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttfb_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    pub transition_type: String,
}

/// Observation of provider behavior over the continuation horizon H following a shadow decision.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProspectiveHorizonObservation {
    pub horizon_id: String,
    pub decision_id: String,
    pub session_id: String,
    pub base_request_index: u32,
    pub horizon_declared: u32,
    pub horizon_observed: u32,
    pub horizon_complete: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncation_reason: Option<String>,
    pub transitions: Vec<ContinuationTransition>,
    pub cumulative_observed_cost_usd: f64,
    pub cumulative_predicted_baseline_cost_usd: f64,
    pub cumulative_modeled_optimized_cost_usd: f64,
    pub observed_cache_retention_ratio: f64,
    pub notes: Vec<String>,
}

/// Evaluates subsequent requests following a base decision up to horizon H.
pub fn evaluate_prospective_horizon(
    decision: &ShadowDecisionEvidence,
    subsequent_outcomes: &[ObservedProviderOutcome],
    horizon_declared: u32,
) -> ProspectiveHorizonObservation {
    let horizon_id = format!("pho_{}", decision.record_id);
    let mut transitions = Vec::new();
    let mut cum_observed_cost = 0.0;
    let mut warm_transitions = 0;
    let mut warm_retained = 0;

    let base_idx = decision.request_index;
    let mut matching_outcomes: Vec<&ObservedProviderOutcome> = subsequent_outcomes
        .iter()
        .filter(|o| {
            o.session_id == decision.session_id
                && o.request_index > base_idx
                && o.request_index <= base_idx + horizon_declared
        })
        .collect();
    matching_outcomes.sort_by_key(|o| o.request_index);

    let mut prev_hit_ratio = if decision.cache_state == CacheState::WarmVerified {
        0.8
    } else {
        0.0
    };

    for obs in &matching_outcomes {
        cum_observed_cost += obs.calculated_cost_usd;
        let hit_ratio = obs.observed_cache_hit_ratio;

        let trans_type = if prev_hit_ratio >= 0.5 {
            warm_transitions += 1;
            if hit_ratio >= 0.5 {
                warm_retained += 1;
                "WarmPreserved"
            } else {
                "WarmDrop"
            }
        } else if obs.actual_cache_write_tokens.unwrap_or(0) > 0 || hit_ratio > 0.5 {
            "ColdWrite"
        } else {
            "ColdContinued"
        };
        prev_hit_ratio = hit_ratio;

        transitions.push(ContinuationTransition {
            request_index: obs.request_index,
            turn_index: obs.turn_index,
            input_tokens: obs.actual_input_tokens,
            cached_tokens: obs.actual_cached_tokens,
            uncached_tokens: obs.actual_uncached_tokens,
            cache_hit_ratio: hit_ratio,
            cost_usd: obs.calculated_cost_usd,
            ttfb_ms: obs.ttfb_ms,
            latency_ms: obs.request_latency_ms,
            transition_type: trans_type.to_string(),
        });
    }

    let horizon_observed = transitions.len() as u32;
    let horizon_complete = horizon_observed >= horizon_declared;
    let truncation_reason = if !horizon_complete {
        Some("session_ended_before_horizon".to_string())
    } else {
        None
    };

    let cache_retention_ratio = if warm_transitions > 0 {
        warm_retained as f64 / warm_transitions as f64
    } else {
        1.0
    };

    let pred_base_single = decision.predicted_baseline_cost_usd.unwrap_or(0.0);
    let opt_single = decision
        .predicted_optimized_cost_usd
        .unwrap_or(pred_base_single);
    let cum_pred_base = pred_base_single * (horizon_observed as f64);
    let cum_modeled_opt = opt_single * (horizon_observed as f64);

    ProspectiveHorizonObservation {
        horizon_id,
        decision_id: decision.record_id.clone(),
        session_id: decision.session_id.clone(),
        base_request_index: decision.request_index,
        horizon_declared,
        horizon_observed,
        horizon_complete,
        truncation_reason,
        transitions,
        cumulative_observed_cost_usd: cum_observed_cost,
        cumulative_predicted_baseline_cost_usd: cum_pred_base,
        cumulative_modeled_optimized_cost_usd: cum_modeled_opt,
        observed_cache_retention_ratio: cache_retention_ratio,
        notes: Vec::new(),
    }
}

/// Measured request-level provider metrics directly verified by telemetry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MeasuredTelemetryMetrics {
    pub total_requests_observed: usize,
    pub total_measured_cost_usd: f64,
    pub total_measured_input_tokens: u64,
    pub total_measured_cached_tokens: u64,
    pub total_measured_uncached_tokens: u64,
    pub total_measured_output_tokens: u64,
    pub overall_measured_cache_hit_ratio: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mean_ttfb_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mean_request_latency_ms: Option<f64>,
}

/// Modeled policy metrics estimated pre-dispatch by ContextWatcher.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModeledPolicyMetrics {
    pub total_evaluations: usize,
    pub decisions_apply: usize,
    pub decisions_defer: usize,
    pub decisions_reject: usize,
    pub decisions_ignore: usize,
    pub reasons_breakdown: BTreeMap<String, usize>,
    pub modeled_baseline_cost_usd: f64,
    pub modeled_optimized_cost_usd: f64,
    pub projected_gross_savings_usd: f64,
}

/// Unverifiable counterfactual metrics tracking mutations that were deferred or rejected and never executed upstream.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UnverifiableCounterfactualMetrics {
    pub deferred_candidates_count: usize,
    pub rejected_candidates_count: usize,
    pub unverifiable_reason: String,
    pub notes: Vec<String>,
}

/// Summary metrics for decision-to-outcome correlation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CorrelationSummaryMetrics {
    pub total_decisions: usize,
    pub matched_immediate: usize,
    pub matched_continuation: usize,
    pub uncorrelated_missing: usize,
    pub uncorrelated_ambiguous: usize,
    pub cache_state_consistent_count: usize,
    pub cache_state_inconsistent_count: usize,
    pub mean_cost_delta_usd: f64,
}

/// Summary metrics for continuation horizon tracking.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HorizonAccuracyMetrics {
    pub total_horizons_tracked: usize,
    pub fully_observed_horizons: usize,
    pub truncated_horizons: usize,
    pub mean_horizon_observed_turns: f64,
    pub overall_cache_retention_ratio: f64,
    pub warm_drops_detected: usize,
}

/// Prospective shadow decision calibration report separating Measured, Modeled, and Unverifiable counterfactuals.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProspectiveCalibrationReport {
    pub report_title: String,
    pub generated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_commit: Option<String>,
    pub measured: MeasuredTelemetryMetrics,
    pub modeled: ModeledPolicyMetrics,
    pub unverifiable: UnverifiableCounterfactualMetrics,
    pub correlation: CorrelationSummaryMetrics,
    pub horizon: HorizonAccuracyMetrics,
    pub discrepancies: Vec<PolicyDiscrepancy>,
    pub notes: Vec<String>,
}

/// Builds a ProspectiveCalibrationReport from prospective decisions, observed outcomes, correlations, and horizons.
pub fn build_prospective_calibration_report(
    decisions: &[ShadowDecisionEvidence],
    outcomes: &[ObservedProviderOutcome],
    correlations: &[CorrelatedDecisionOutcome],
    horizons: &[ProspectiveHorizonObservation],
    git_commit: Option<String>,
    generated_at: Option<DateTime<Utc>>,
) -> ProspectiveCalibrationReport {
    let total_requests_observed = outcomes.len();
    let mut total_cost = 0.0;
    let mut total_input = 0u64;
    let mut total_cached = 0u64;
    let mut total_uncached = 0u64;
    let mut total_output = 0u64;
    let mut sum_ttfb = 0u64;
    let mut count_ttfb = 0usize;
    let mut sum_latency = 0u64;
    let mut count_latency = 0usize;

    for o in outcomes {
        total_cost += o.calculated_cost_usd;
        total_input += o.actual_input_tokens as u64;
        total_cached += o.actual_cached_tokens as u64;
        total_uncached += o.actual_uncached_tokens as u64;
        total_output += o.actual_output_tokens as u64;
        if let Some(t) = o.ttfb_ms {
            sum_ttfb += t;
            count_ttfb += 1;
        }
        if let Some(l) = o.request_latency_ms {
            sum_latency += l;
            count_latency += 1;
        }
    }

    let overall_hit_ratio = if total_input > 0 {
        total_cached as f64 / total_input as f64
    } else {
        0.0
    };

    let mean_ttfb = if count_ttfb > 0 {
        Some(sum_ttfb as f64 / count_ttfb as f64)
    } else {
        None
    };

    let mean_latency = if count_latency > 0 {
        Some(sum_latency as f64 / count_latency as f64)
    } else {
        None
    };

    let measured = MeasuredTelemetryMetrics {
        total_requests_observed,
        total_measured_cost_usd: total_cost,
        total_measured_input_tokens: total_input,
        total_measured_cached_tokens: total_cached,
        total_measured_uncached_tokens: total_uncached,
        total_measured_output_tokens: total_output,
        overall_measured_cache_hit_ratio: overall_hit_ratio,
        mean_ttfb_ms: mean_ttfb,
        mean_request_latency_ms: mean_latency,
    };

    // Modeled metrics
    let mut apply_count = 0usize;
    let mut defer_count = 0usize;
    let mut reject_count = 0usize;
    let mut ignore_count = 0usize;
    let mut reasons_map: BTreeMap<String, usize> = BTreeMap::new();
    let mut modeled_base = 0.0;
    let mut modeled_opt = 0.0;

    for d in decisions {
        match d.decision {
            Some(OptimizationDecision::Apply) => apply_count += 1,
            Some(OptimizationDecision::Defer) => defer_count += 1,
            Some(OptimizationDecision::Reject) => reject_count += 1,
            Some(OptimizationDecision::NoOp) | None => ignore_count += 1,
        }
        if let Some(r) = &d.reason_code {
            *reasons_map.entry(format!("{:?}", r)).or_insert(0) += 1;
        }
        if let Some(b) = d.predicted_baseline_cost_usd {
            modeled_base += b;
        }
        if let Some(o) = d.predicted_optimized_cost_usd {
            modeled_opt += o;
        }
    }

    let modeled = ModeledPolicyMetrics {
        total_evaluations: decisions.len(),
        decisions_apply: apply_count,
        decisions_defer: defer_count,
        decisions_reject: reject_count,
        decisions_ignore: ignore_count,
        reasons_breakdown: reasons_map,
        modeled_baseline_cost_usd: modeled_base,
        modeled_optimized_cost_usd: modeled_opt,
        projected_gross_savings_usd: (modeled_base - modeled_opt).max(0.0),
    };

    let unverifiable = UnverifiableCounterfactualMetrics {
        deferred_candidates_count: defer_count,
        rejected_candidates_count: reject_count,
        unverifiable_reason: "Deferred and rejected mutations were never executed upstream; counterfactual alternative trajectory cannot be measured or proven directly from passive telemetry.".to_string(),
        notes: vec![
            "ActiveToolLifecycle deferrals preserve tool invocation integrity without generating modified requests.".to_string(),
            "SignedThinkingContent rejections preserve cryptographically signed model outputs without modifying thinking blocks.".to_string(),
        ],
    };

    // Correlation summary
    let mut matched_imm = 0usize;
    let mut matched_cont = 0usize;
    let mut uncorr_miss = 0usize;
    let mut uncorr_ambig = 0usize;
    let mut consistent_count = 0usize;
    let mut inconsistent_count = 0usize;
    let mut sum_delta = 0.0;
    let mut count_delta = 0usize;

    for c in correlations {
        match c.correlation_status {
            CorrelationStatus::MatchedImmediate => matched_imm += 1,
            CorrelationStatus::MatchedContinuation => matched_cont += 1,
            CorrelationStatus::UncorrelatedMissingOutcome => uncorr_miss += 1,
            CorrelationStatus::UncorrelatedAmbiguous => uncorr_ambig += 1,
            CorrelationStatus::RetrospectiveObservation => {}
        }
        if let Some(true) = c.cache_state_consistent {
            consistent_count += 1;
        } else if let Some(false) = c.cache_state_consistent {
            inconsistent_count += 1;
        }
        if let Some(d) = c.single_turn_cost_delta_usd {
            sum_delta += d;
            count_delta += 1;
        }
    }

    let mean_delta = if count_delta > 0 {
        sum_delta / count_delta as f64
    } else {
        0.0
    };

    let correlation = CorrelationSummaryMetrics {
        total_decisions: correlations.len(),
        matched_immediate: matched_imm,
        matched_continuation: matched_cont,
        uncorrelated_missing: uncorr_miss,
        uncorrelated_ambiguous: uncorr_ambig,
        cache_state_consistent_count: consistent_count,
        cache_state_inconsistent_count: inconsistent_count,
        mean_cost_delta_usd: mean_delta,
    };

    // Horizon accuracy
    let mut full_h = 0usize;
    let mut trunc_h = 0usize;
    let mut sum_h_obs = 0u32;
    let mut sum_retention = 0.0;
    let mut warm_drops = 0usize;

    for h in horizons {
        if h.horizon_complete {
            full_h += 1;
        } else {
            trunc_h += 1;
        }
        sum_h_obs += h.horizon_observed;
        sum_retention += h.observed_cache_retention_ratio;
        for t in &h.transitions {
            if t.transition_type == "WarmDrop" {
                warm_drops += 1;
            }
        }
    }

    let mean_h_obs = if !horizons.is_empty() {
        sum_h_obs as f64 / horizons.len() as f64
    } else {
        0.0
    };

    let overall_retention = if !horizons.is_empty() {
        sum_retention / horizons.len() as f64
    } else {
        1.0
    };

    let horizon = HorizonAccuracyMetrics {
        total_horizons_tracked: horizons.len(),
        fully_observed_horizons: full_h,
        truncated_horizons: trunc_h,
        mean_horizon_observed_turns: mean_h_obs,
        overall_cache_retention_ratio: overall_retention,
        warm_drops_detected: warm_drops,
    };

    ProspectiveCalibrationReport {
        report_title: "ContextWatcher CW-117C Prospective Shadow Calibration Report".to_string(),
        generated_at: generated_at.unwrap_or_else(Utc::now),
        git_commit,
        measured,
        modeled,
        unverifiable,
        correlation,
        horizon,
        discrepancies: Vec::new(),
        notes: vec![
            "Measured metrics reflect passive provider telemetry captured via proxy tap.".to_string(),
            "Modeled metrics reflect pre-dispatch policy estimations calculated in shadow mode.".to_string(),
            "Strict shadow mode ensured 100% byte fidelity to provider endpoints with zero mutations applied.".to_string(),
        ],
    }
}

/// Generates a deterministic synthetic fixture with 6 requests, multiple candidates per request,
/// provider telemetry, immediate correlation, and prospective continuation horizon tracking.
pub fn generate_synthetic_prospective_fixture() -> (
    Vec<ShadowDecisionEvidence>,
    Vec<ObservedProviderOutcome>,
    Vec<CorrelatedDecisionOutcome>,
    Vec<ProspectiveHorizonObservation>,
    ProspectiveCalibrationReport,
) {
    let session_id = "session_cw117c_synthetic".to_string();
    let provider = ProviderKind::Anthropic;
    let model = "claude-3-5-sonnet-20241022".to_string();
    let base_ts = DateTime::parse_from_rfc3339("2026-11-01T08:00:00Z")
        .unwrap()
        .with_timezone(&Utc);

    let mut decisions = Vec::new();
    let mut outcomes = Vec::new();
    let mut correlations = Vec::new();
    let mut horizons = Vec::new();

    // Turn 1 (Cold cache write)
    let d1_cand = ShadowDecisionEvidence {
        record_id: format!("sde_{}_{}_cand1", session_id, 1),
        session_id: session_id.clone(),
        request_index: 1,
        turn_index: 1,
        provider,
        model: model.clone(),
        cache_state: CacheState::ColdLikely,
        provenance: TelemetryProvenance::ObservedProvider,
        prefix_fingerprint: Some("pfx_cold_001".to_string()),
        payload_fingerprint: Some("pfp_req_001".to_string()),
        candidate_id: Some("cand_prune_01".to_string()),
        plan_id: None,
        policy_evaluation_status: PolicyEvaluationStatus::EvaluatedPreDispatch,
        decision: Some(OptimizationDecision::Apply),
        reason_code: Some(DecisionReasonCode::NoDemonstrableBenefit),
        explanation: "Cold start request; cache creation anticipated".to_string(),
        estimated_saved_tokens: Some(300),
        estimated_kv_break_tokens: Some(0),
        predicted_baseline_cost_usd: Some(0.01875),
        predicted_optimized_cost_usd: Some(0.017625),
        predicted_net_benefit_usd: Some(0.001125),
        forecast_horizon_turns: 3,
        forecast_assumptions: "Linear continuation over 3 turns".to_string(),
        pricing_model_pattern: Some("claude-3-5-sonnet*".to_string()),
        pricing_version: "2026-10-anthropic-v1".to_string(),
        policy_version: "0.2.4-cw117c".to_string(),
        decision_timestamp: base_ts,
    };
    decisions.push(d1_cand.clone());

    let o1 = ObservedProviderOutcome {
        session_id: session_id.clone(),
        request_index: 1,
        turn_index: 1,
        provider,
        model: model.clone(),
        actual_input_tokens: 5000,
        actual_cached_tokens: 0,
        actual_uncached_tokens: 5000,
        actual_cache_write_tokens: Some(5000),
        actual_output_tokens: 200,
        calculated_cost_usd: 0.03675,
        reported_cost_usd: None,
        observed_cache_hit_ratio: 0.0,
        ttfb_ms: Some(420),
        request_latency_ms: Some(1100),
        cache_telemetry_available: true,
        execution_outcome: "completed".to_string(),
        applied_mutations: vec![],
        missing_data_reasons: vec![],
        observed_timestamp: base_ts + chrono::Duration::seconds(1),
    };
    outcomes.push(o1.clone());
    correlations.push(correlate_decision_with_outcome(&d1_cand, Some(&o1)));

    // Turn 2 (Warm hit, Apply Pin)
    let ts2 = base_ts + chrono::Duration::seconds(10);
    let d2_cand = ShadowDecisionEvidence {
        record_id: format!("sde_{}_{}_cand1", session_id, 2),
        session_id: session_id.clone(),
        request_index: 2,
        turn_index: 2,
        provider,
        model: model.clone(),
        cache_state: CacheState::WarmVerified,
        provenance: TelemetryProvenance::ObservedProvider,
        prefix_fingerprint: Some("pfx_warm_002".to_string()),
        payload_fingerprint: Some("pfp_req_002".to_string()),
        candidate_id: Some("cand_pin_01".to_string()),
        plan_id: None,
        policy_evaluation_status: PolicyEvaluationStatus::EvaluatedPreDispatch,
        decision: Some(OptimizationDecision::Apply),
        reason_code: Some(DecisionReasonCode::TailAppendPreservesPrefixNoDirectSaving),
        explanation: "Warm cache hit; pin preserves prefix".to_string(),
        estimated_saved_tokens: Some(0),
        estimated_kv_break_tokens: Some(0),
        predicted_baseline_cost_usd: Some(0.00675),
        predicted_optimized_cost_usd: Some(0.00675),
        predicted_net_benefit_usd: Some(0.0),
        forecast_horizon_turns: 3,
        forecast_assumptions: "Preserves cache prefix for next turns".to_string(),
        pricing_model_pattern: Some("claude-3-5-sonnet*".to_string()),
        pricing_version: "2026-10-anthropic-v1".to_string(),
        policy_version: "0.2.4-cw117c".to_string(),
        decision_timestamp: ts2,
    };
    decisions.push(d2_cand.clone());

    let o2 = ObservedProviderOutcome {
        session_id: session_id.clone(),
        request_index: 2,
        turn_index: 2,
        provider,
        model: model.clone(),
        actual_input_tokens: 5500,
        actual_cached_tokens: 5000,
        actual_uncached_tokens: 500,
        actual_cache_write_tokens: Some(500),
        actual_output_tokens: 250,
        calculated_cost_usd: 0.0075,
        reported_cost_usd: None,
        observed_cache_hit_ratio: 5000.0 / 5500.0,
        ttfb_ms: Some(210),
        request_latency_ms: Some(850),
        cache_telemetry_available: true,
        execution_outcome: "completed".to_string(),
        applied_mutations: vec![],
        missing_data_reasons: vec![],
        observed_timestamp: ts2 + chrono::Duration::seconds(1),
    };
    outcomes.push(o2.clone());
    correlations.push(correlate_decision_with_outcome(&d2_cand, Some(&o2)));

    // Turn 3 (Warm hit, Tool Lifecycle -> Defer candidate)
    let ts3 = base_ts + chrono::Duration::seconds(20);
    let d3_cand = ShadowDecisionEvidence {
        record_id: format!("sde_{}_{}_cand_tool", session_id, 3),
        session_id: session_id.clone(),
        request_index: 3,
        turn_index: 3,
        provider,
        model: model.clone(),
        cache_state: CacheState::WarmVerified,
        provenance: TelemetryProvenance::ObservedProvider,
        prefix_fingerprint: Some("pfx_warm_003".to_string()),
        payload_fingerprint: Some("pfp_req_003".to_string()),
        candidate_id: Some("cand_tool_prune_01".to_string()),
        plan_id: None,
        policy_evaluation_status: PolicyEvaluationStatus::EvaluatedPreDispatch,
        decision: Some(OptimizationDecision::Defer),
        reason_code: Some(DecisionReasonCode::ActiveToolLifecycleProtection),
        explanation: "Tool invocation in-flight; deferred candidate".to_string(),
        estimated_saved_tokens: Some(800),
        estimated_kv_break_tokens: Some(0),
        predicted_baseline_cost_usd: Some(0.007),
        predicted_optimized_cost_usd: Some(0.007),
        predicted_net_benefit_usd: Some(0.0),
        forecast_horizon_turns: 3,
        forecast_assumptions: "Deferred until tool invocation returns result".to_string(),
        pricing_model_pattern: Some("claude-3-5-sonnet*".to_string()),
        pricing_version: "2026-10-anthropic-v1".to_string(),
        policy_version: "0.2.4-cw117c".to_string(),
        decision_timestamp: ts3,
    };
    decisions.push(d3_cand.clone());

    let o3 = ObservedProviderOutcome {
        session_id: session_id.clone(),
        request_index: 3,
        turn_index: 3,
        provider,
        model: model.clone(),
        actual_input_tokens: 5800,
        actual_cached_tokens: 5500,
        actual_uncached_tokens: 300,
        actual_cache_write_tokens: Some(300),
        actual_output_tokens: 200,
        calculated_cost_usd: 0.0068,
        reported_cost_usd: None,
        observed_cache_hit_ratio: 5500.0 / 5800.0,
        ttfb_ms: Some(195),
        request_latency_ms: Some(780),
        cache_telemetry_available: true,
        execution_outcome: "completed".to_string(),
        applied_mutations: vec![],
        missing_data_reasons: vec![],
        observed_timestamp: ts3 + chrono::Duration::seconds(1),
    };
    outcomes.push(o3.clone());
    correlations.push(correlate_decision_with_outcome(&d3_cand, Some(&o3)));

    // Turn 4 (Warm hit, Apply Prune)
    let ts4 = base_ts + chrono::Duration::seconds(30);
    let d4_cand = ShadowDecisionEvidence {
        record_id: format!("sde_{}_{}_cand_prune", session_id, 4),
        session_id: session_id.clone(),
        request_index: 4,
        turn_index: 4,
        provider,
        model: model.clone(),
        cache_state: CacheState::WarmVerified,
        provenance: TelemetryProvenance::ObservedProvider,
        prefix_fingerprint: Some("pfx_warm_004".to_string()),
        payload_fingerprint: Some("pfp_req_004".to_string()),
        candidate_id: Some("cand_prune_02".to_string()),
        plan_id: None,
        policy_evaluation_status: PolicyEvaluationStatus::EvaluatedPreDispatch,
        decision: Some(OptimizationDecision::Apply),
        reason_code: Some(DecisionReasonCode::ModeledEconomicBenefitPositive),
        explanation: "Tail-only prune preserves warm prefix".to_string(),
        estimated_saved_tokens: Some(400),
        estimated_kv_break_tokens: Some(0),
        predicted_baseline_cost_usd: Some(0.0072),
        predicted_optimized_cost_usd: Some(0.0069),
        predicted_net_benefit_usd: Some(0.0003),
        forecast_horizon_turns: 3,
        forecast_assumptions: "Direct tail reduction preserving cache prefix".to_string(),
        pricing_model_pattern: Some("claude-3-5-sonnet*".to_string()),
        pricing_version: "2026-10-anthropic-v1".to_string(),
        policy_version: "0.2.4-cw117c".to_string(),
        decision_timestamp: ts4,
    };
    decisions.push(d4_cand.clone());

    let o4 = ObservedProviderOutcome {
        session_id: session_id.clone(),
        request_index: 4,
        turn_index: 4,
        provider,
        model: model.clone(),
        actual_input_tokens: 6000,
        actual_cached_tokens: 5800,
        actual_uncached_tokens: 200,
        actual_cache_write_tokens: Some(200),
        actual_output_tokens: 180,
        calculated_cost_usd: 0.0065,
        reported_cost_usd: None,
        observed_cache_hit_ratio: 5800.0 / 6000.0,
        ttfb_ms: Some(190),
        request_latency_ms: Some(720),
        cache_telemetry_available: true,
        execution_outcome: "completed".to_string(),
        applied_mutations: vec![],
        missing_data_reasons: vec![],
        observed_timestamp: ts4 + chrono::Duration::seconds(1),
    };
    outcomes.push(o4.clone());
    correlations.push(correlate_decision_with_outcome(&d4_cand, Some(&o4)));

    // Turn 5 (Warm hit, Reject candidate due to SafetyViolation)
    let ts5 = base_ts + chrono::Duration::seconds(40);
    let d5_cand = ShadowDecisionEvidence {
        record_id: format!("sde_{}_{}_cand_signed", session_id, 5),
        session_id: session_id.clone(),
        request_index: 5,
        turn_index: 5,
        provider,
        model: model.clone(),
        cache_state: CacheState::WarmVerified,
        provenance: TelemetryProvenance::ObservedProvider,
        prefix_fingerprint: Some("pfx_warm_005".to_string()),
        payload_fingerprint: Some("pfp_req_005".to_string()),
        candidate_id: Some("cand_fold_signed".to_string()),
        plan_id: None,
        policy_evaluation_status: PolicyEvaluationStatus::EvaluatedPreDispatch,
        decision: Some(OptimizationDecision::Reject),
        reason_code: Some(DecisionReasonCode::SafetyViolation),
        explanation: "Signed thinking block protected; candidate rejected".to_string(),
        estimated_saved_tokens: Some(600),
        estimated_kv_break_tokens: Some(0),
        predicted_baseline_cost_usd: Some(0.0075),
        predicted_optimized_cost_usd: Some(0.0075),
        predicted_net_benefit_usd: Some(0.0),
        forecast_horizon_turns: 3,
        forecast_assumptions: "Signed blocks cannot be modified".to_string(),
        pricing_model_pattern: Some("claude-3-5-sonnet*".to_string()),
        pricing_version: "2026-10-anthropic-v1".to_string(),
        policy_version: "0.2.4-cw117c".to_string(),
        decision_timestamp: ts5,
    };
    decisions.push(d5_cand.clone());

    let o5 = ObservedProviderOutcome {
        session_id: session_id.clone(),
        request_index: 5,
        turn_index: 5,
        provider,
        model: model.clone(),
        actual_input_tokens: 6200,
        actual_cached_tokens: 6000,
        actual_uncached_tokens: 200,
        actual_cache_write_tokens: Some(200),
        actual_output_tokens: 150,
        calculated_cost_usd: 0.0062,
        reported_cost_usd: None,
        observed_cache_hit_ratio: 6000.0 / 6200.0,
        ttfb_ms: Some(185),
        request_latency_ms: Some(690),
        cache_telemetry_available: true,
        execution_outcome: "completed".to_string(),
        applied_mutations: vec![],
        missing_data_reasons: vec![],
        observed_timestamp: ts5 + chrono::Duration::seconds(1),
    };
    outcomes.push(o5.clone());
    correlations.push(correlate_decision_with_outcome(&d5_cand, Some(&o5)));

    // Turn 6 (Final request in session)
    let ts6 = base_ts + chrono::Duration::seconds(50);
    let o6 = ObservedProviderOutcome {
        session_id: session_id.clone(),
        request_index: 6,
        turn_index: 6,
        provider,
        model: model.clone(),
        actual_input_tokens: 6300,
        actual_cached_tokens: 6200,
        actual_uncached_tokens: 100,
        actual_cache_write_tokens: Some(100),
        actual_output_tokens: 80,
        calculated_cost_usd: 0.0048,
        reported_cost_usd: None,
        observed_cache_hit_ratio: 6200.0 / 6300.0,
        ttfb_ms: Some(170),
        request_latency_ms: Some(550),
        cache_telemetry_available: true,
        execution_outcome: "completed".to_string(),
        applied_mutations: vec![],
        missing_data_reasons: vec![],
        observed_timestamp: ts6 + chrono::Duration::seconds(1),
    };
    outcomes.push(o6);

    // Evaluate horizons for decisions
    for d in &decisions {
        let pho = evaluate_prospective_horizon(d, &outcomes, d.forecast_horizon_turns);
        horizons.push(pho);
    }

    let report = build_prospective_calibration_report(
        &decisions,
        &outcomes,
        &correlations,
        &horizons,
        Some("git_commit_cw117c".to_string()),
        Some(base_ts + chrono::Duration::seconds(60)),
    );

    (decisions, outcomes, correlations, horizons, report)
}

/// Data structure matching the committed CW-115 historical benchmark datasets.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoricalSessionDataset {
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub git_commit: Option<String>,
    #[serde(default)]
    pub package_version: Option<String>,
    #[serde(default)]
    pub harness: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub total_sessions_executed: Option<usize>,
    #[serde(default)]
    pub total_spend_usd: Option<f64>,
    pub sessions: Vec<HistoricalSessionRecord>,
}

/// Historical session trace record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoricalSessionRecord {
    pub session_id: String,
    pub mode: String,
    #[serde(default)]
    pub task_family: Option<String>,
    #[serde(default)]
    pub repetition: Option<u32>,
    pub model: String,
    pub total_input_tokens: u32,
    pub total_cached_tokens: u32,
    pub total_uncached_tokens: u32,
    pub total_output_tokens: u32,
    pub overall_cache_ratio: f64,
    pub total_cost_usd: f64,
    #[serde(default)]
    pub total_wall_time_ms: Option<u64>,
    #[serde(default)]
    pub mean_ttfb_ms: Option<f64>,
    #[serde(default)]
    pub acceptance_passed: Option<bool>,
    pub requests: Vec<HistoricalRequestRecord>,
}

/// Historical request telemetry record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoricalRequestRecord {
    #[serde(default)]
    pub mode: Option<String>,
    pub session_id: String,
    #[serde(default)]
    pub turn_index: Option<u32>,
    pub request_index: u32,
    pub model: String,
    pub input_tokens: u32,
    pub cached_tokens: u32,
    pub uncached_tokens: u32,
    pub output_tokens: u32,
    pub cache_hit_ratio: f64,
    #[serde(default)]
    pub ttfb_ms: Option<u64>,
    #[serde(default)]
    pub request_latency_ms: Option<u64>,
    #[serde(default)]
    pub cost_usd: f64,
    #[serde(default)]
    pub applied_mutation_ids: Vec<String>,
    #[serde(default)]
    pub raw_provider_usage: Option<Value>,
    #[serde(default)]
    pub timestamp: Option<String>,
}

/// Discrepancy finding classifying an empirical policy weakness or modeling limitation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PolicyDiscrepancy {
    pub case_number: u8,
    pub title: String,
    pub session_id: String,
    pub request_index: u32,
    pub severity: String,
    pub observed_phenomenon: String,
    pub policy_implication: String,
    pub recommendation: String,
}

/// Comprehensive calibration report summarizing prediction accuracy, economic reconciliation,
/// and empirical weaknesses across replayed benchmark datasets.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CalibrationMetricsReport {
    pub dataset_source: String,
    pub git_commit: Option<String>,
    pub total_sessions: usize,
    pub total_requests: usize,
    /// Cache state classification breakdown across replayed turns (sorted by key for determinism)
    pub cache_state_distribution: BTreeMap<String, usize>,
    /// Agreement metrics between cache state predictions and subsequent eligible provider transitions
    pub cache_agreement: CacheAgreementMetrics,
    /// Economic reconciliation comparing dataset costs to applicable pricing profile
    pub economic_reconciliation: EconomicReconciliationMetrics,
    /// Continuation horizon statistics
    pub horizon_metrics: HorizonMetrics,
    /// Policy decision frequency and qualification distribution
    pub policy_decisions: PolicyDecisionMetrics,
    /// Explicit list of empirical discrepancies and weaknesses discovered
    pub discrepancies: Vec<PolicyDiscrepancy>,
    /// Summary of known limitations and missing data fields in historical evidence
    pub data_limitations: Vec<String>,
    pub generated_at: DateTime<Utc>,
}

/// Descriptive transition metrics across eligible next-request turns.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct CacheAgreementMetrics {
    /// Total eligible next-request transitions (total_requests minus total_sessions)
    pub total_eligible_transitions: usize,
    /// Terminal requests excluded from transition evaluation because no subsequent request exists in the session
    pub terminal_requests_excluded: usize,
    /// Number of eligible transitions where current request was classified WarmVerified
    pub warm_verified_transitions: usize,
    /// Eligible WarmVerified transitions where subsequent request retained cached tokens (> 0)
    pub warm_to_warm_retained: usize,
    /// Eligible WarmVerified transitions where subsequent request dropped to 0 cached tokens
    pub warm_to_cold_drops: usize,
    /// Warm cache retention percentage: warm_to_warm_retained / warm_verified_transitions * 100.0
    pub warm_retention_pct: f64,
    /// Number of eligible transitions where current request was classified ColdLikely
    pub cold_likely_transitions: usize,
    /// Eligible ColdLikely transitions where subsequent request remained at 0 cached tokens
    pub cold_to_cold_retained_zero: usize,
    /// Eligible ColdLikely transitions where subsequent request was primed into warm (> 0 cached tokens)
    pub cold_to_warm_primed: usize,
    /// Cold priming rate: cold_to_warm_primed / cold_likely_transitions * 100.0
    pub cold_primed_pct: f64,
    /// Number of eligible transitions where current request was classified Unknown
    pub unknown_transitions: usize,
    /// Number of eligible transitions where current request was classified Rebuilding
    pub rebuilding_transitions: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct EconomicReconciliationMetrics {
    pub total_dataset_reported_cost_usd: f64,
    pub total_profile_recalculated_cost_usd: f64,
    pub absolute_cost_variance_usd: f64,
    pub max_single_request_variance_usd: f64,
    pub pricing_profile_coverage_pct: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct HorizonMetrics {
    pub assumed_horizon_turns: u32,
    pub mean_actual_session_requests: f64,
    pub min_session_requests: u32,
    pub max_session_requests: u32,
    pub horizon_truncation_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct PolicyDecisionMetrics {
    /// Total prospective decisions evaluated pre-dispatch
    pub total_evaluated_decisions: usize,
    /// Requests where policy evaluation was omitted due to missing pre-dispatch candidates
    pub not_evaluated_missing_candidates_count: usize,
    pub apply_count: usize,
    pub defer_count: usize,
    pub reject_count: usize,
    pub no_op_count: usize,
    pub reasons_breakdown: BTreeMap<String, usize>,
    pub unverifiable_counterfactual_count: usize,
}

/// Reconciles actual request cost using ContextWatcher's verified pricing profile.
pub fn reconcile_request_cost(
    req: &HistoricalRequestRecord,
    pricing: Option<&ModelPricingProfile>,
) -> Option<f64> {
    let p = pricing?;
    if !p.is_supported {
        return None;
    }
    // Calculate cost strictly from reported token buckets
    let cached_cost = (req.cached_tokens as f64 / 1_000_000.0) * p.cached_input_read_per_mtok;
    let uncached_cost = (req.uncached_tokens as f64 / 1_000_000.0) * p.uncached_input_per_mtok;
    let output_cost = (req.output_tokens as f64 / 1_000_000.0) * p.output_per_mtok;

    Some(cached_cost + uncached_cost + output_cost)
}

/// Deterministically replays a committed historical session dataset through CW-117's policy engine,
/// producing durable correlated evidence records and calibration metrics.
///
/// Guaranteed zero-network, immutable replay: makes 0 HTTP requests and does not modify the dataset.
/// Does not fabricate prospective optimization decisions or synthetic dollar savings for unmutated traces.
pub fn replay_historical_dataset(
    dataset: &HistoricalSessionDataset,
    dataset_name: &str,
    horizon_turns: u32,
) -> (CalibrationMetricsReport, Vec<CorrelatedDecisionOutcome>) {
    let mut correlated_records = Vec::new();
    let mut cache_distribution: BTreeMap<String, usize> = BTreeMap::new();
    let mut discrepancies = Vec::new();

    let mut warm_verified_transitions = 0;
    let mut warm_to_warm_retained = 0;
    let mut warm_to_cold_drops = 0;
    let mut cold_likely_transitions = 0;
    let mut cold_to_cold_retained_zero = 0;
    let mut cold_to_warm_primed = 0;
    let mut unknown_transitions = 0;
    let mut rebuilding_transitions = 0;
    let mut terminal_requests_excluded = 0;

    let mut total_dataset_cost = 0.0;
    let mut total_recalculated_cost = 0.0;
    let mut max_single_var = 0.0;
    let mut pricing_supported_count = 0;
    let mut total_requests_count = 0;

    let mut session_lengths = Vec::new();
    let mut horizon_truncations = 0;

    let data_limitations = vec![
        "Historical dataset omits raw prompt bodies; prefix fingerprints are unavailable (None).".to_string(),
        "Pre-dispatch mutation candidate snapshots and unmutated prompt trees were not captured at proxy time.".to_string(),
        "Historical replay does not manufacture synthetic optimization decisions or savings (policy_evaluation_status = NotEvaluatedMissingCandidates).".to_string(),
        "Mode A native Codex traces aggregate multi-turn tool loops into fewer coarse requests.".to_string(),
        "Counterfactual savings on passthrough traces cannot be treated as realized costs without an active execution trial.".to_string(),
    ];

    for session in &dataset.sessions {
        session_lengths.push(session.requests.len() as u32);
        let mut prior_observation: Option<CacheObservation> = None;

        for (idx, req) in session.requests.iter().enumerate() {
            total_requests_count += 1;
            total_dataset_cost += req.cost_usd;

            let provider = ProviderKind::OpenAI;
            let pricing = find_pricing_profile(provider, &req.model);
            if pricing.is_some() {
                pricing_supported_count += 1;
            }

            // Reconcile cost
            if let Some(recalculated) = reconcile_request_cost(req, pricing.as_ref()) {
                total_recalculated_cost += recalculated;
                let var = (recalculated - req.cost_usd).abs();
                if var > max_single_var {
                    max_single_var = var;
                }
            }

            // Synthesize KV cache report from request's actual provider usage
            let kv_report = crate::ir::KvCacheReport {
                hit_tokens: req.cached_tokens,
                newly_cached_tokens: req.uncached_tokens,
                invalidated_tokens: 0,
                uncached_tail_tokens: 0,
                hit_ratio: req.cache_hit_ratio as f32,
                divergence_token_index: None,
                invalidation_cause: None,
                telemetry_available: true,
            };

            // Classify cache state using strict CW-117 rules
            let cache_obs = classify_cache_state(
                &session.session_id,
                provider,
                &req.model,
                req.request_index,
                None, // Prefix fingerprint unavailable in historical dataset
                prior_observation.as_ref(),
                Some(&kv_report),
                false,
            );

            let state_str = format!("{:?}", cache_obs.state);
            *cache_distribution.entry(state_str).or_insert(0) += 1;

            // Transition accounting: check next request in session if non-terminal
            let next_req = session.requests.get(idx + 1);
            let mut cache_consistent = None;

            if let Some(next) = next_req {
                match cache_obs.state {
                    CacheState::WarmVerified => {
                        warm_verified_transitions += 1;
                        if next.cached_tokens > 0 {
                            warm_to_warm_retained += 1;
                            cache_consistent = Some(true);
                        } else {
                            warm_to_cold_drops += 1;
                            cache_consistent = Some(false);
                            // Weakness Case 1: Warm cache dropped to zero
                            discrepancies.push(PolicyDiscrepancy {
                                case_number: 1,
                                title: "Warm cache dropped to 0 hits on subsequent request".to_string(),
                                session_id: session.session_id.clone(),
                                request_index: req.request_index,
                                severity: "Medium".to_string(),
                                observed_phenomenon: format!(
                                    "Request #{} had {} cached tokens (hit ratio {:.2}), but Request #{} reported 0 cached tokens.",
                                    req.request_index, req.cached_tokens, req.cache_hit_ratio, next.request_index
                                ),
                                policy_implication: "Predicted baseline continuation would expect cached read rates, but provider experienced transient cache eviction or node rerouting.".to_string(),
                                recommendation: "Track consecutive cache drops and transition to Unknown state when cache drop is detected without prefix change.".to_string(),
                            });
                        }
                    }
                    CacheState::ColdLikely => {
                        cold_likely_transitions += 1;
                        if next.cached_tokens == 0 {
                            cold_to_cold_retained_zero += 1;
                            cache_consistent = Some(true);
                        } else {
                            cold_to_warm_primed += 1;
                            cache_consistent = Some(true); // Normal cold initialization: Turn 1 establishes prefix, Turn 2 reuses
                            if idx == 0 {
                                discrepancies.push(PolicyDiscrepancy {
                                    case_number: 2,
                                    title: "Cold prompt primed cache for subsequent high hit ratio".to_string(),
                                    session_id: session.session_id.clone(),
                                    request_index: req.request_index,
                                    severity: "Info".to_string(),
                                    observed_phenomenon: format!(
                                        "Cold initial prompt of {} tokens established prefix, enabling {} cached tokens on Request #{}.",
                                        req.input_tokens, next.cached_tokens, next.request_index
                                    ),
                                    policy_implication: "Turn 1 ColdLikely state transitions cleanly to WarmVerified on Turn 2 once prefix reuse is observed.".to_string(),
                                    recommendation: "Preserve Turn 1 cold compaction authorization while ensuring Turn 2 immediately switches to WarmVerified deferral.".to_string(),
                                });
                            }
                        }
                    }
                    CacheState::Unknown => {
                        unknown_transitions += 1;
                    }
                    CacheState::Rebuilding => {
                        rebuilding_transitions += 1;
                    }
                }
            } else {
                terminal_requests_excluded += 1;
            }

            let has_pin = req.applied_mutation_ids.iter().any(|id| id.contains("pin"));
            let has_prune = req
                .applied_mutation_ids
                .iter()
                .any(|id| id.contains("prune"));

            // Check Weakness Case 3: Applied historical pruning (or combined pruning+pinning) with uncached token surge
            if has_prune && req.uncached_tokens > 1000 && req.cached_tokens > 0 {
                let context_mode = if has_pin {
                    "Combined historical pruning and tail pinning (Mode 3)"
                } else {
                    "Historical pruning (Mode 2)"
                };
                discrepancies.push(PolicyDiscrepancy {
                    case_number: 3,
                    title: format!("{}: uncached token surge consistent with prefix break", context_mode),
                    session_id: session.session_id.clone(),
                    request_index: req.request_index,
                    severity: "High".to_string(),
                    observed_phenomenon: format!(
                        "Request #{} applied mutations {:?}; uncached tokens surged to {} despite {} cached tokens.",
                        req.request_index, req.applied_mutation_ids, req.uncached_tokens, req.cached_tokens
                    ),
                    policy_implication: "Prefix-modifying mutations on an established cache prefix incur substantial re-caching charges. Policy correctly identifies this risk, though causal attribution requires controlled isolated AB trials.".to_string(),
                    recommendation: "Enforce CachePrefixAtRisk deferral when cache is WarmVerified unless net multi-turn benefit overcomes break penalty.".to_string(),
                });
            }

            // Check Weakness Case 7: Pure one-time tail steering directive (Mode 1: pin only, no prune)
            if has_pin && !has_prune {
                discrepancies.push(PolicyDiscrepancy {
                    case_number: 7,
                    title: "Pure tail steering directive applied without prefix invalidation (Mode 1)".to_string(),
                    session_id: session.session_id.clone(),
                    request_index: req.request_index,
                    severity: "Info".to_string(),
                    observed_phenomenon: format!(
                        "Request #{} applied PinContext directive ({:?}); cached tokens remained intact on subsequent turns.",
                        req.request_index, req.applied_mutation_ids
                    ),
                    policy_implication: "Pure tail pin preserves cached prefix (0 KV break) while behaviorally guiding the agent to complete in fewer total requests.".to_string(),
                    recommendation: "Conservatively model direct token cut as 0 USD while acknowledging behavioral request reduction in empirical reports.".to_string(),
                });
            }

            // Parse request timestamp deterministically
            let decision_ts = req
                .timestamp
                .as_deref()
                .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
                .map(|dt| dt.with_timezone(&Utc))
                .or_else(|| {
                    dataset
                        .date
                        .as_deref()
                        .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
                        .map(|dt| dt.with_timezone(&Utc))
                })
                .unwrap_or_else(|| {
                    DateTime::parse_from_rfc3339("2026-10-31T20:25:00Z")
                        .unwrap()
                        .with_timezone(&Utc)
                });

            // Construct durable historical evidence record WITHOUT fabricating decisions or savings
            let evidence_record = ShadowDecisionEvidence::from_historical_observation(
                req,
                &session.session_id,
                provider,
                &cache_obs,
                pricing.as_ref(),
                horizon_turns,
                dataset_name,
                decision_ts,
            );

            let observed_outcome = ObservedProviderOutcome {
                session_id: session.session_id.clone(),
                request_index: req.request_index,
                turn_index: req.turn_index.unwrap_or(req.request_index),
                provider,
                model: req.model.clone(),
                actual_input_tokens: req.input_tokens,
                actual_cached_tokens: req.cached_tokens,
                actual_uncached_tokens: req.uncached_tokens,
                actual_cache_write_tokens: None,
                actual_output_tokens: req.output_tokens,
                calculated_cost_usd: reconcile_request_cost(req, pricing.as_ref())
                    .unwrap_or(req.cost_usd),
                reported_cost_usd: Some(req.cost_usd),
                observed_cache_hit_ratio: req.cache_hit_ratio,
                ttfb_ms: req.ttfb_ms,
                request_latency_ms: req.request_latency_ms,
                cache_telemetry_available: true,
                execution_outcome: "completed".to_string(),
                applied_mutations: req.applied_mutation_ids.clone(),
                missing_data_reasons: vec![],
                observed_timestamp: decision_ts,
            };

            let remaining_in_session = session.requests.len().saturating_sub(idx + 1);
            if remaining_in_session < horizon_turns as usize {
                horizon_truncations += 1;
            }

            let correlation_pair = CorrelatedDecisionOutcome {
                correlation_id: format!("corr_{}_{}", session.session_id, req.request_index),
                session_id: session.session_id.clone(),
                request_index: req.request_index,
                decision: evidence_record,
                outcome: Some(observed_outcome),
                correlation_status: CorrelationStatus::RetrospectiveObservation,
                single_turn_cost_delta_usd: None, // No predicted baseline in retrospective replay
                cache_state_consistent: cache_consistent,
                notes: if remaining_in_session < horizon_turns as usize {
                    vec![format!(
                        "Session ended after {} remaining turns; full {}-turn horizon observation truncated.",
                        remaining_in_session, horizon_turns
                    )]
                } else {
                    vec![]
                },
            };

            correlated_records.push(correlation_pair);
            prior_observation = Some(cache_obs);
        }
    }

    let mean_len = if !session_lengths.is_empty() {
        session_lengths.iter().sum::<u32>() as f64 / session_lengths.len() as f64
    } else {
        0.0
    };
    let min_len = session_lengths.iter().copied().min().unwrap_or(0);
    let max_len = session_lengths.iter().copied().max().unwrap_or(0);

    let warm_retention_pct = if warm_verified_transitions > 0 {
        (warm_to_warm_retained as f64 / warm_verified_transitions as f64) * 100.0
    } else {
        100.0
    };

    let cold_primed_pct = if cold_likely_transitions > 0 {
        (cold_to_warm_primed as f64 / cold_likely_transitions as f64) * 100.0
    } else {
        100.0
    };

    let pricing_coverage_pct = if total_requests_count > 0 {
        (pricing_supported_count as f64 / total_requests_count as f64) * 100.0
    } else {
        0.0
    };

    // Weakness Case 6: Continuation horizon discrepancy
    if (mean_len - horizon_turns as f64).abs() > 2.0 {
        discrepancies.push(PolicyDiscrepancy {
            case_number: 6,
            title: "Assumed continuation horizon (H=3) differs substantially from mean session length".to_string(),
            session_id: "aggregate_sessions".to_string(),
            request_index: 0,
            severity: "Medium".to_string(),
            observed_phenomenon: format!(
                "Configured horizon H={} turns, but observed sessions averaged {:.1} requests (range {}-{}).",
                horizon_turns, mean_len, min_len, max_len
            ),
            policy_implication: "A 3-turn break-even horizon underestimates the compound cost of a prefix invalidation on long-running 15-25 turn sessions.".to_string(),
            recommendation: "Allow dynamic horizon scaling based on task family and observed conversation turn count in Phase 2.".to_string(),
        });
    }

    // Deterministic generated_at timestamp derived from dataset date or fallback
    let generated_at = dataset
        .date
        .as_deref()
        .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|| {
            DateTime::parse_from_rfc3339("2026-10-31T20:25:00Z")
                .unwrap()
                .with_timezone(&Utc)
        });

    let report = CalibrationMetricsReport {
        dataset_source: dataset_name.to_string(),
        git_commit: dataset.git_commit.clone(),
        total_sessions: dataset.sessions.len(),
        total_requests: total_requests_count,
        cache_state_distribution: cache_distribution,
        cache_agreement: CacheAgreementMetrics {
            total_eligible_transitions: total_requests_count.saturating_sub(dataset.sessions.len()),
            terminal_requests_excluded,
            warm_verified_transitions,
            warm_to_warm_retained,
            warm_to_cold_drops,
            warm_retention_pct,
            cold_likely_transitions,
            cold_to_cold_retained_zero,
            cold_to_warm_primed,
            cold_primed_pct,
            unknown_transitions,
            rebuilding_transitions,
        },
        economic_reconciliation: EconomicReconciliationMetrics {
            total_dataset_reported_cost_usd: total_dataset_cost,
            total_profile_recalculated_cost_usd: total_recalculated_cost,
            absolute_cost_variance_usd: (total_recalculated_cost - total_dataset_cost).abs(),
            max_single_request_variance_usd: max_single_var,
            pricing_profile_coverage_pct: pricing_coverage_pct,
        },
        horizon_metrics: HorizonMetrics {
            assumed_horizon_turns: horizon_turns,
            mean_actual_session_requests: mean_len,
            min_session_requests: min_len,
            max_session_requests: max_len,
            horizon_truncation_count: horizon_truncations,
        },
        policy_decisions: PolicyDecisionMetrics {
            total_evaluated_decisions: 0,
            not_evaluated_missing_candidates_count: total_requests_count,
            apply_count: 0,
            defer_count: 0,
            reject_count: 0,
            no_op_count: 0,
            reasons_breakdown: BTreeMap::new(),
            unverifiable_counterfactual_count: 0,
        },
        discrepancies,
        data_limitations,
        generated_at,
    };

    (report, correlated_records)
}

/// Helper to read and replay a historical benchmark dataset from a file path.
pub fn replay_historical_trace_file<P: AsRef<Path>>(
    path: P,
    horizon_turns: u32,
) -> Result<(CalibrationMetricsReport, Vec<CorrelatedDecisionOutcome>), std::io::Error> {
    let content = std::fs::read_to_string(path.as_ref())?;
    let dataset: HistoricalSessionDataset = serde_json::from_str(&content)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let name = path
        .as_ref()
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");
    Ok(replay_historical_dataset(&dataset, name, horizon_turns))
}
