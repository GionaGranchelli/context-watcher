use chrono::Utc;
use cw_core::calibration::{
    CorrelationStatus, HistoricalRequestRecord, POLICY_CALIBRATION_VERSION, PolicyEvaluationStatus,
    ShadowDecisionEvidence, reconcile_request_cost, replay_historical_trace_file,
};
use cw_core::ir::ProviderKind;
use cw_core::policy::{
    CacheObservation, CacheState, DecisionReasonCode, OptimizationDecision, PolicyDecision,
    TelemetryProvenance, find_pricing_profile,
};
use serde_json::json;
use std::path::{Path, PathBuf};

fn resolve_dataset_path(rel: &str) -> PathBuf {
    let p = Path::new(rel);
    if p.exists() {
        return p.to_path_buf();
    }
    let from_manifest = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates dir")
        .parent()
        .expect("repo root")
        .join(rel);
    assert!(
        from_manifest.exists(),
        "Required benchmark dataset fixture must exist at {:?} or {:?}",
        p,
        from_manifest
    );
    from_manifest
}

#[test]
fn test_calibration_evidence_creation_and_privacy_guarantee() {
    // Workstream 1: Privacy-preserving shadow decision evidence record
    let cache_obs = CacheObservation {
        state: CacheState::WarmVerified,
        provenance: TelemetryProvenance::ObservedProvider,
        observed_cached_tokens: Some(12544),
        reported_input_tokens: Some(13000),
        provider: ProviderKind::OpenAI,
        model: "gpt-4o-mini".to_string(),
        session_id: "sess_privacy_test".to_string(),
        prefix_fingerprint: Some("fp_sha256_mock_hash".to_string()),
        observed_at: Utc::now(),
        turn_seq: 6,
        notes: vec!["Prefix stable".to_string()],
    };

    let pricing = find_pricing_profile(ProviderKind::OpenAI, "gpt-4o-mini");

    let decision = PolicyDecision {
        decision: OptimizationDecision::Defer,
        reason_code: DecisionReasonCode::CachePrefixAtRisk,
        explanation: "Active warm cache prefix would be invalidated (12544 tokens at risk)."
            .to_string(),
        candidate_id: Some("cand_prune_1".to_string()),
        plan_id: Some("plan_aggressive".to_string()),
        cache_state: CacheState::WarmVerified,
        estimated_kv_break_tokens: 12544,
        net_token_delta: 250,
        economic_evaluation: None,
        evidence: cw_core::policy::DecisionEvidence {
            session_id: "sess_privacy_test".to_string(),
            turn_seq: 6,
            provider: ProviderKind::OpenAI,
            model: "gpt-4o-mini".to_string(),
            telemetry_provenance: TelemetryProvenance::ObservedProvider,
            reported_cached_tokens: Some(12544),
            is_append_only: false,
            lifecycle_safe: true,
            analysis_complete: true,
        },
        timestamp: Utc::now(),
    };

    let evidence = ShadowDecisionEvidence::from_policy_decision(
        &decision,
        &cache_obs,
        pricing.as_ref(),
        3,
        6,
        6,
    );

    // Verify invariants:
    assert_eq!(evidence.session_id, "sess_privacy_test");
    assert_eq!(evidence.request_index, 6);
    assert_eq!(
        evidence.record_id,
        "sde_sess_privacy_test_6_plan_aggressive_cand_prune_1"
    );
    assert_eq!(
        evidence.policy_evaluation_status,
        PolicyEvaluationStatus::EvaluatedPreDispatch
    );
    assert_eq!(evidence.decision, Some(OptimizationDecision::Defer));
    assert_eq!(
        evidence.reason_code,
        Some(DecisionReasonCode::CachePrefixAtRisk)
    );
    assert_eq!(evidence.policy_version, POLICY_CALIBRATION_VERSION);
    assert_eq!(evidence.estimated_kv_break_tokens, Some(12544));

    // Serialize to JSON and verify ZERO prompt text is contained
    let json_str = serde_json::to_string(&evidence).expect("Must serialize cleanly");
    assert!(!json_str.contains("prompt"));
    assert!(!json_str.contains("content"));
    assert!(!json_str.contains("messages"));
    assert!(!json_str.contains("user_query"));
    assert!(json_str.contains("fp_sha256_mock_hash"));
}

#[test]
fn test_calibration_historical_observation_omits_synthetic_decisions_and_savings() {
    // Finding P0: Historical replay must never manufacture optimization decisions or forecast savings
    let req = HistoricalRequestRecord {
        mode: Some("mode_0_passthrough".to_string()),
        session_id: "sess_hist_1".to_string(),
        turn_index: Some(1),
        request_index: 1,
        model: "gpt-4o-mini".to_string(),
        input_tokens: 11981,
        cached_tokens: 0,
        uncached_tokens: 11981,
        output_tokens: 22,
        cache_hit_ratio: 0.0,
        ttfb_ms: Some(2552),
        request_latency_ms: Some(2713),
        cost_usd: 0.00181035,
        applied_mutation_ids: vec![],
        raw_provider_usage: None,
        timestamp: Some("2026-10-31T19:20:38Z".to_string()),
    };

    let cache_obs = CacheObservation {
        state: CacheState::ColdLikely,
        provenance: TelemetryProvenance::ObservedProvider,
        observed_cached_tokens: Some(0),
        reported_input_tokens: Some(11981),
        provider: ProviderKind::OpenAI,
        model: "gpt-4o-mini".to_string(),
        session_id: "sess_hist_1".to_string(),
        prefix_fingerprint: None,
        observed_at: Utc::now(),
        turn_seq: 1,
        notes: vec![],
    };

    let pricing = find_pricing_profile(ProviderKind::OpenAI, "gpt-4o-mini");
    let evidence = ShadowDecisionEvidence::from_historical_observation(
        &req,
        "sess_hist_1",
        ProviderKind::OpenAI,
        &cache_obs,
        pricing.as_ref(),
        3,
        "test_dataset",
        Utc::now(),
    );

    assert_eq!(
        evidence.policy_evaluation_status,
        PolicyEvaluationStatus::NotEvaluatedMissingCandidates
    );
    assert_eq!(evidence.decision, None);
    assert_eq!(evidence.reason_code, None);
    assert_eq!(evidence.estimated_saved_tokens, None);
    assert_eq!(evidence.estimated_kv_break_tokens, None);
    assert_eq!(evidence.predicted_baseline_cost_usd, None);
    assert_eq!(evidence.predicted_optimized_cost_usd, None);
    assert_eq!(evidence.predicted_net_benefit_usd, None);
}

#[test]
fn test_calibration_pricing_reconciliation_exact_decimal() {
    // Workstream 2: Reconcile actual request cost from pricing profile
    let req = HistoricalRequestRecord {
        mode: Some("mode_0_passthrough".to_string()),
        session_id: "sess_econ_1".to_string(),
        turn_index: Some(1),
        request_index: 1,
        model: "gpt-4o-mini".to_string(),
        input_tokens: 11981,
        cached_tokens: 0,
        uncached_tokens: 11981,
        output_tokens: 22,
        cache_hit_ratio: 0.0,
        ttfb_ms: Some(2552),
        request_latency_ms: Some(2713),
        cost_usd: 0.00181035,
        applied_mutation_ids: vec![],
        raw_provider_usage: Some(json!({
            "input_tokens": 11981,
            "output_tokens": 22
        })),
        timestamp: Some("2026-10-31T19:20:38Z".to_string()),
    };

    let pricing = find_pricing_profile(ProviderKind::OpenAI, "gpt-4o-mini");
    let reconciled = reconcile_request_cost(&req, pricing.as_ref())
        .expect("Pricing profile must reconcile supported gpt-4o-mini model");

    let diff = (reconciled - req.cost_usd).abs();
    assert!(
        diff < 1e-12,
        "Reconciled cost delta must be at floating-point precision limit"
    );
}

#[test]
fn test_calibration_unsupported_or_incorrect_pricing_returns_none() {
    let req = HistoricalRequestRecord {
        mode: None,
        session_id: "sess_custom".to_string(),
        turn_index: Some(1),
        request_index: 1,
        model: "gpt-4o-custom-finetune".to_string(),
        input_tokens: 5000,
        cached_tokens: 0,
        uncached_tokens: 5000,
        output_tokens: 100,
        cache_hit_ratio: 0.0,
        ttfb_ms: None,
        request_latency_ms: None,
        cost_usd: 0.01,
        applied_mutation_ids: vec![],
        raw_provider_usage: None,
        timestamp: None,
    };

    // Unverified model returns None
    let pricing = find_pricing_profile(ProviderKind::OpenAI, &req.model);
    assert!(pricing.is_none());
    assert!(reconcile_request_cost(&req, pricing.as_ref()).is_none());
}

#[test]
fn test_calibration_offline_replay_reproducibility_and_immutability() {
    // Workstream 3 & 4: Replaying the same dataset twice produces bit-for-bit identical results
    let dataset_path = resolve_dataset_path("docs/cw115_four_mode_benchmark_data.json");

    let (rep1, recs1) = replay_historical_trace_file(&dataset_path, 3)
        .expect("Dataset docs/cw115_four_mode_benchmark_data.json must replay cleanly");
    let (rep2, recs2) = replay_historical_trace_file(&dataset_path, 3).expect(
        "Dataset docs/cw115_four_mode_benchmark_data.json must replay cleanly on second pass",
    );

    // Exact identity
    assert_eq!(rep1.total_sessions, 4);
    assert_eq!(rep1.total_requests, 71);
    assert_eq!(rep1.total_sessions, rep2.total_sessions);
    assert_eq!(rep1.total_requests, rep2.total_requests);
    assert_eq!(rep1.cache_state_distribution, rep2.cache_state_distribution);
    assert_eq!(recs1.len(), recs2.len());

    // Finding P1: Retrospective correlation status
    assert!(
        recs1
            .iter()
            .all(|r| r.correlation_status == CorrelationStatus::RetrospectiveObservation)
    );

    // Finding P1: Transition metrics and eligible transitions
    assert_eq!(rep1.cache_agreement.total_eligible_transitions, 67);
    assert_eq!(rep1.cache_agreement.terminal_requests_excluded, 4);
    assert_eq!(rep1.cache_agreement.warm_verified_transitions, 63);
    assert_eq!(rep1.cache_agreement.warm_to_warm_retained, 62);
    assert_eq!(rep1.cache_agreement.warm_to_cold_drops, 1);
    assert!((rep1.cache_agreement.warm_retention_pct - 98.412698).abs() < 1e-4);
    assert_eq!(rep1.cache_agreement.cold_likely_transitions, 4);
    assert_eq!(rep1.cache_agreement.cold_to_warm_primed, 4);
    assert_eq!(rep1.cache_agreement.cold_to_cold_retained_zero, 0);
    assert_eq!(rep1.cache_agreement.cold_primed_pct, 100.0);

    // Finding P0: Zero manufactured decisions
    assert_eq!(rep1.policy_decisions.total_evaluated_decisions, 0);
    assert_eq!(
        rep1.policy_decisions.not_evaluated_missing_candidates_count,
        71
    );
    assert_eq!(rep1.policy_decisions.apply_count, 0);
    assert_eq!(rep1.policy_decisions.defer_count, 0);

    // Cost reconciliation exact variance
    assert!(rep1.economic_reconciliation.max_single_request_variance_usd < 1e-12);
    assert!(rep1.economic_reconciliation.absolute_cost_variance_usd < 1e-10);
    assert_eq!(
        rep1.economic_reconciliation.pricing_profile_coverage_pct,
        100.0
    );
}

#[test]
fn test_calibration_discrepancy_detection_warm_drop_and_prefix_break() {
    // Workstream 5: Verify detection of Case 1 (Warm drop), Case 3 (Prefix break), and Case 7 (Pure Tail Pin)
    let dataset_path = resolve_dataset_path("docs/cw115_four_mode_benchmark_data.json");

    let (rep, _) = replay_historical_trace_file(&dataset_path, 3).unwrap();

    // Verify Case 1 was detected in Session m0 at Request 8/9
    let case1 = rep.discrepancies.iter().find(|d| d.case_number == 1);
    assert!(
        case1.is_some(),
        "Case 1 (Warm cache drop to 0) must be detected in m0"
    );
    let c1 = case1.unwrap();
    assert_eq!(c1.session_id, "cw115_m0_passthrough_f1_rep1");
    assert!(c1.observed_phenomenon.contains("Request #8"));

    // Verify Case 3 was detected in Session m2 (applied historical pruning)
    let case3_m2 = rep
        .discrepancies
        .iter()
        .find(|d| d.case_number == 3 && d.session_id.contains("m2_prune"));
    assert!(
        case3_m2.is_some(),
        "Case 3 (Historical pruning break) must be detected in m2"
    );
    let c3_m2 = case3_m2.unwrap();
    assert_eq!(c3_m2.severity, "High");
    assert!(c3_m2.observed_phenomenon.contains("uncached tokens surged"));
    assert!(c3_m2.title.contains("Historical pruning (Mode 2)"));

    // Finding P1: Verify combined prune+pin (m3) is classified under Case 3 as combined, NOT Case 7
    let case3_m3 = rep
        .discrepancies
        .iter()
        .find(|d| d.case_number == 3 && d.session_id.contains("m3_combined"));
    assert!(
        case3_m3.is_some(),
        "Case 3 must detect combined pruning and pinning in m3"
    );
    let c3_m3 = case3_m3.unwrap();
    assert!(
        c3_m3
            .title
            .contains("Combined historical pruning and tail pinning (Mode 3)")
    );

    // Finding P1: Verify Case 7 is detected ONLY in Session m1 (pure tail pin directive) and NOT in m3
    let case7_m1 = rep
        .discrepancies
        .iter()
        .find(|d| d.case_number == 7 && d.session_id.contains("m1_tail"));
    assert!(
        case7_m1.is_some(),
        "Case 7 (Pure one-time tail directive) must be detected in m1"
    );

    let case7_m3 = rep
        .discrepancies
        .iter()
        .find(|d| d.case_number == 7 && d.session_id.contains("m3_combined"));
    assert!(
        case7_m3.is_none(),
        "Case 7 must NOT fire on combined prune+pin run (m3)"
    );
}

#[test]
fn test_calibration_horizon_truncation_tracking() {
    let dataset_path = resolve_dataset_path("docs/cw115_benchmark_data.json");

    let (rep, recs) = replay_historical_trace_file(&dataset_path, 3).unwrap();
    assert_eq!(rep.total_sessions, 6);
    assert_eq!(rep.total_requests, 59);

    // Verify horizon truncation count matches requests occurring within last H-1 requests of session
    assert!(rep.horizon_metrics.horizon_truncation_count > 0);

    // Terminal requests must have truncation note
    let last_record = recs
        .iter()
        .find(|r| r.session_id == "cw115_passthrough_f1_rep1" && r.request_index == 12)
        .unwrap();
    assert!(last_record.notes.iter().any(|n| n.contains("truncated")));
}
