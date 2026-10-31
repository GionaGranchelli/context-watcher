use chrono::Utc;
use cw_core::calibration::{POLICY_CALIBRATION_VERSION, ShadowDecisionEvidence};
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
        candidate_id: Some("cand_1".to_string()),
        plan_id: Some("plan_1".to_string()),
        decision: OptimizationDecision::Apply,
        reason_code: DecisionReasonCode::ModeledEconomicBenefitPositive,
        explanation: "Cold compaction authorized.".to_string(),
        estimated_saved_tokens: 300,
        estimated_kv_break_tokens: 0,
        predicted_baseline_cost_usd: 0.005,
        predicted_optimized_cost_usd: 0.003,
        predicted_net_benefit_usd: 0.002,
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
    assert_eq!(list[0].decision, OptimizationDecision::Apply);
    assert_eq!(list[0].estimated_saved_tokens, 300);

    // List all
    let all = store
        .list_shadow_decision_evidence(None)
        .expect("Must query all");
    assert_eq!(all.len(), 1);
}
