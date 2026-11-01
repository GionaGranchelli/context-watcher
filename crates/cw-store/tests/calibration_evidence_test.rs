use chrono::Utc;
use cw_core::calibration::{
    POLICY_CALIBRATION_VERSION, PolicyEvaluationStatus, ShadowDecisionEvidence,
};
use cw_core::ir::ProviderKind;
use cw_core::policy::{CacheState, DecisionReasonCode, OptimizationDecision, TelemetryProvenance};
use cw_store::CwStore;

#[test]
fn test_calibration_cw_store_persistence_roundtrip() {
    let store = CwStore::open_in_memory().unwrap();

    let evidence = ShadowDecisionEvidence {
        record_id: "sde_test_persistence_1".to_string(),
        session_id: "sess_persist".to_string(),
        request_index: 2,
        turn_index: 2,
        provider: ProviderKind::OpenAI,
        model: "gpt-4o-mini".to_string(),
        cache_state: CacheState::ColdLikely,
        provenance: TelemetryProvenance::ObservedProvider,
        prefix_fingerprint: None,
        payload_fingerprint: None,
        candidate_id: Some("cand_1".to_string()),
        plan_id: Some("plan_1".to_string()),
        policy_evaluation_status: PolicyEvaluationStatus::EvaluatedPreDispatch,
        decision: Some(OptimizationDecision::Apply),
        reason_code: Some(DecisionReasonCode::ModeledEconomicBenefitPositive),
        explanation: "Cold compaction authorized.".to_string(),
        estimated_saved_tokens: Some(300),
        estimated_kv_break_tokens: Some(0),
        predicted_baseline_cost_usd: Some(0.005),
        predicted_optimized_cost_usd: Some(0.003),
        predicted_net_benefit_usd: Some(0.002),
        forecast_horizon_turns: 3,
        forecast_assumptions: "test".to_string(),
        pricing_model_pattern: Some("gpt-4o-mini".to_string()),
        pricing_version: "2026-10-pricing-v1".to_string(),
        policy_version: POLICY_CALIBRATION_VERSION.to_string(),
        decision_timestamp: Utc::now(),
    };

    // Save
    store
        .save_shadow_decision_evidence(&evidence)
        .expect("Must save shadow evidence to SQLite");

    // List by session
    let list = store
        .list_shadow_decision_evidence(Some("sess_persist"))
        .expect("Must query by session");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].record_id, "sde_test_persistence_1");
    assert_eq!(
        list[0].policy_evaluation_status,
        PolicyEvaluationStatus::EvaluatedPreDispatch
    );
    assert_eq!(list[0].decision, Some(OptimizationDecision::Apply));
    assert_eq!(list[0].estimated_saved_tokens, Some(300));

    // List all
    let all = store
        .list_shadow_decision_evidence(None)
        .expect("Must query all");
    assert_eq!(all.len(), 1);
}

#[test]
fn test_calibration_cw_store_persistence_retrospective_nulls_roundtrip() {
    let store = CwStore::open_in_memory().unwrap();

    let retrospective = ShadowDecisionEvidence {
        record_id: "sde_test_retrospective_1".to_string(),
        session_id: "sess_retro".to_string(),
        request_index: 5,
        turn_index: 5,
        provider: ProviderKind::OpenAI,
        model: "gpt-4o-mini".to_string(),
        cache_state: CacheState::WarmVerified,
        provenance: TelemetryProvenance::ObservedProvider,
        prefix_fingerprint: None,
        payload_fingerprint: None,
        candidate_id: None,
        plan_id: None,
        policy_evaluation_status: PolicyEvaluationStatus::NotEvaluatedMissingCandidates,
        decision: None,
        reason_code: None,
        explanation: "Historical trace replay: no candidate snapshot".to_string(),
        estimated_saved_tokens: None,
        estimated_kv_break_tokens: None,
        predicted_baseline_cost_usd: None,
        predicted_optimized_cost_usd: None,
        predicted_net_benefit_usd: None,
        forecast_horizon_turns: 3,
        forecast_assumptions: "retrospective".to_string(),
        pricing_model_pattern: Some("gpt-4o-mini".to_string()),
        pricing_version: "2026-10-pricing-v1".to_string(),
        policy_version: POLICY_CALIBRATION_VERSION.to_string(),
        decision_timestamp: Utc::now(),
    };

    store
        .save_shadow_decision_evidence(&retrospective)
        .expect("Must save retrospective shadow evidence with NULL decisions to SQLite");

    let list = store
        .list_shadow_decision_evidence(Some("sess_retro"))
        .expect("Must query by session");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].record_id, "sde_test_retrospective_1");
    assert_eq!(
        list[0].policy_evaluation_status,
        PolicyEvaluationStatus::NotEvaluatedMissingCandidates
    );
    assert_eq!(list[0].decision, None);
    assert_eq!(list[0].reason_code, None);
    assert_eq!(list[0].estimated_saved_tokens, None);
    assert_eq!(list[0].predicted_net_benefit_usd, None);
}

#[test]
fn test_calibration_multiple_decisions_single_request_independently_retrievable() {
    let store = CwStore::open_in_memory().unwrap();

    let cache_obs = cw_core::policy::CacheObservation {
        state: CacheState::WarmVerified,
        provenance: TelemetryProvenance::ObservedProvider,
        observed_cached_tokens: Some(10000),
        reported_input_tokens: Some(12000),
        provider: ProviderKind::OpenAI,
        model: "gpt-4o-mini".to_string(),
        session_id: "sess_multi".to_string(),
        prefix_fingerprint: Some("fp_sha256_mock".to_string()),
        observed_at: Utc::now(),
        turn_seq: 4,
        notes: vec![],
    };

    let pricing = cw_core::policy::find_pricing_profile(ProviderKind::OpenAI, "gpt-4o-mini");

    // Plan decision (plan_id set, candidate_id None)
    let plan_decision = cw_core::policy::PolicyDecision {
        decision: OptimizationDecision::Defer,
        reason_code: DecisionReasonCode::CachePrefixAtRisk,
        explanation: "Plan deferred: active warm prefix at risk.".to_string(),
        candidate_id: None,
        plan_id: Some("surgical_log_diet".to_string()),
        cache_state: CacheState::WarmVerified,
        estimated_kv_break_tokens: 10000,
        net_token_delta: 600,
        economic_evaluation: None,
        evidence: cw_core::policy::DecisionEvidence {
            session_id: "sess_multi".to_string(),
            turn_seq: 4,
            provider: ProviderKind::OpenAI,
            model: "gpt-4o-mini".to_string(),
            telemetry_provenance: TelemetryProvenance::ObservedProvider,
            reported_cached_tokens: Some(10000),
            is_append_only: false,
            lifecycle_safe: true,
            analysis_complete: true,
        },
        timestamp: Utc::now(),
    };

    // Candidate 1 decision (plan_id set, candidate_id set)
    let cand1_decision = cw_core::policy::PolicyDecision {
        decision: OptimizationDecision::Defer,
        reason_code: DecisionReasonCode::CachePrefixAtRisk,
        explanation: "Candidate 1 deferred.".to_string(),
        candidate_id: Some("cand_prune_tool_1".to_string()),
        plan_id: Some("surgical_log_diet".to_string()),
        cache_state: CacheState::WarmVerified,
        estimated_kv_break_tokens: 10000,
        net_token_delta: 350,
        economic_evaluation: None,
        evidence: cw_core::policy::DecisionEvidence {
            session_id: "sess_multi".to_string(),
            turn_seq: 4,
            provider: ProviderKind::OpenAI,
            model: "gpt-4o-mini".to_string(),
            telemetry_provenance: TelemetryProvenance::ObservedProvider,
            reported_cached_tokens: Some(10000),
            is_append_only: false,
            lifecycle_safe: true,
            analysis_complete: true,
        },
        timestamp: Utc::now(),
    };

    // Candidate 2 decision (plan_id set, candidate_id set)
    let cand2_decision = cw_core::policy::PolicyDecision {
        decision: OptimizationDecision::Defer,
        reason_code: DecisionReasonCode::CachePrefixAtRisk,
        explanation: "Candidate 2 deferred.".to_string(),
        candidate_id: Some("cand_prune_tool_2".to_string()),
        plan_id: Some("surgical_log_diet".to_string()),
        cache_state: CacheState::WarmVerified,
        estimated_kv_break_tokens: 10000,
        net_token_delta: 250,
        economic_evaluation: None,
        evidence: cw_core::policy::DecisionEvidence {
            session_id: "sess_multi".to_string(),
            turn_seq: 4,
            provider: ProviderKind::OpenAI,
            model: "gpt-4o-mini".to_string(),
            telemetry_provenance: TelemetryProvenance::ObservedProvider,
            reported_cached_tokens: Some(10000),
            is_append_only: false,
            lifecycle_safe: true,
            analysis_complete: true,
        },
        timestamp: Utc::now(),
    };

    let ev_plan = ShadowDecisionEvidence::from_policy_decision(
        &plan_decision,
        &cache_obs,
        pricing.as_ref(),
        3,
        4,
        4,
    );
    let ev_cand1 = ShadowDecisionEvidence::from_policy_decision(
        &cand1_decision,
        &cache_obs,
        pricing.as_ref(),
        3,
        4,
        4,
    );
    let ev_cand2 = ShadowDecisionEvidence::from_policy_decision(
        &cand2_decision,
        &cache_obs,
        pricing.as_ref(),
        3,
        4,
        4,
    );

    // Verify all 3 have distinct record_id keys
    assert_ne!(ev_plan.record_id, ev_cand1.record_id);
    assert_ne!(ev_plan.record_id, ev_cand2.record_id);
    assert_ne!(ev_cand1.record_id, ev_cand2.record_id);

    // Save all three to SQLite store for the exact same request
    store.save_shadow_decision_evidence(&ev_plan).unwrap();
    store.save_shadow_decision_evidence(&ev_cand1).unwrap();
    store.save_shadow_decision_evidence(&ev_cand2).unwrap();

    // Query back from SQLite: all 3 must remain independently retrievable (no overwriting)
    let list = store
        .list_shadow_decision_evidence(Some("sess_multi"))
        .expect("Query multi-decision session");
    assert_eq!(
        list.len(),
        3,
        "All 3 decisions from Request #4 must persist without overwriting"
    );

    let plan_rec = list.iter().find(|d| d.candidate_id.is_none()).unwrap();
    assert_eq!(plan_rec.plan_id.as_deref(), Some("surgical_log_diet"));
    assert_eq!(plan_rec.estimated_saved_tokens, Some(600));

    let c1_rec = list
        .iter()
        .find(|d| d.candidate_id.as_deref() == Some("cand_prune_tool_1"))
        .unwrap();
    assert_eq!(c1_rec.estimated_saved_tokens, Some(350));

    let c2_rec = list
        .iter()
        .find(|d| d.candidate_id.as_deref() == Some("cand_prune_tool_2"))
        .unwrap();
    assert_eq!(c2_rec.estimated_saved_tokens, Some(250));
}

#[test]
fn test_prospective_correlation_and_horizon_store_persistence() {
    let store = CwStore::open_in_memory().unwrap();
    let (decisions, _outcomes, correlations, horizons, _report) =
        cw_core::calibration::generate_synthetic_prospective_fixture();

    assert!(!decisions.is_empty());
    assert!(!correlations.is_empty());
    assert!(!horizons.is_empty());

    // Save all correlated decision outcomes
    for corr in &correlations {
        store
            .save_correlated_decision_outcome(corr)
            .expect("Save correlated outcome");
    }

    // Save all prospective horizon observations
    for h in &horizons {
        store
            .save_prospective_horizon_observation(h)
            .expect("Save horizon observation");
    }

    // List correlated outcomes
    let loaded_corrs = store
        .list_correlated_decision_outcomes(Some(&decisions[0].session_id))
        .expect("List correlated outcomes by session");
    assert_eq!(loaded_corrs.len(), correlations.len());
    assert_eq!(
        loaded_corrs[0].correlation_id,
        correlations[0].correlation_id
    );
    assert!(loaded_corrs[0].outcome.is_some());

    // List horizons
    let loaded_horizons = store
        .list_prospective_horizon_observations(Some(&decisions[0].session_id))
        .expect("List horizon observations by session");
    assert_eq!(loaded_horizons.len(), horizons.len());
    assert_eq!(loaded_horizons[0].horizon_id, horizons[0].horizon_id);
    assert_eq!(loaded_horizons[0].horizon_declared, 3);
}
