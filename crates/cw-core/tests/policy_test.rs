use chrono::Utc;
use cw_core::{
    analyze_turn_from_payload,
    autopilot::{AutoPilotConfig, evaluate_autopilot_policies},
    ir::*,
    optimizer::{OptimizationCandidateItem, OptimizationStrategyPlan},
    policy::*,
};
use serde_json::json;

fn make_synthetic_turn(
    session_id: &str,
    cached_tokens: Option<u32>,
    telemetry_available: bool,
) -> ContextTurn {
    let payload = json!({
        "model": "gpt-4o-mini",
        "messages": [
            { "role": "system", "content": "You are a helpful coding assistant with deep knowledge." },
            { "role": "user", "content": "Run tests and inspect src/math.rs" },
            {
                "role": "assistant",
                "content": null,
                "tool_calls": [
                    {
                        "id": "call_exec_1",
                        "type": "function",
                        "function": { "name": "exec_command", "arguments": "{\"command\":\"cargo test\"}" }
                    }
                ]
            },
            {
                "role": "tool",
                "tool_call_id": "call_exec_1",
                "content": "test result: ok. 14 passed; 0 failed. Some long output lines from verbose test logs.\n".repeat(40)
            },
            { "role": "assistant", "content": "I have executed the tests. All passed." },
            { "role": "user", "content": "Please continue with the fix." }
        ]
    });

    let (mut turn, _) = analyze_turn_from_payload(
        "turn_synth_1".into(),
        session_id.into(),
        None,
        2,
        &payload,
        ProviderKind::OpenAI,
        None,
        cached_tokens,
        None,
    )
    .expect("failed to analyze synthetic turn");

    turn.kv_cache_report.telemetry_available = telemetry_available;
    turn
}

#[test]
fn test_policy_1_warm_cache_early_historical_pruning_defers() {
    let turn = make_synthetic_turn("sess_warm_1", Some(1500), true);
    let prefix_fp = compute_prefix_fingerprint(&turn.nodes);

    let cache_obs = classify_cache_state(
        &turn.session_id,
        ProviderKind::OpenAI,
        "gpt-4o-mini",
        turn.sequence_number,
        Some(&prefix_fp),
        None,
        Some(&turn.kv_cache_report),
        false,
    );
    assert_eq!(cache_obs.state, CacheState::WarmVerified);
    assert_eq!(cache_obs.provenance, TelemetryProvenance::ObservedProvider);

    let pricing = find_pricing_profile(ProviderKind::OpenAI, "gpt-4o-mini");
    assert!(pricing.is_some());

    // Historical prune targeting tool result
    let op = MutationOp::PruneOrStub {
        target_fingerprint: None,
        target_call_id: Some("call_exec_1".into()),
        target_json_pointer: None,
        stub_text: "[cw:pruned 1000 tokens]".into(),
    };

    let decision = evaluate_decision(
        Some("prune_exec_1"),
        Some("surgical_log_diet"),
        &op,
        &turn,
        &cache_obs,
        pricing.as_ref(),
        3,
    );

    assert_eq!(decision.decision, OptimizationDecision::Defer);
    assert_eq!(decision.reason_code, DecisionReasonCode::CachePrefixAtRisk);
    assert!(decision.estimated_kv_break_tokens > 0);
    assert_eq!(decision.cache_state, CacheState::WarmVerified);

    let diag = decision.compact_diagnostic_summary();
    assert!(diag.contains("Decision: Defer"));
    assert!(diag.contains("Reason: CachePrefixAtRisk"));
    assert!(diag.contains("Actual mutation applied by shadow policy: false"));
}

#[test]
fn test_policy_2_warm_cache_append_only_tail_pin_noops() {
    let turn = make_synthetic_turn("sess_warm_2", Some(1500), true);
    let prefix_fp = compute_prefix_fingerprint(&turn.nodes);

    let cache_obs = classify_cache_state(
        &turn.session_id,
        ProviderKind::OpenAI,
        "gpt-4o-mini",
        turn.sequence_number,
        Some(&prefix_fp),
        None,
        Some(&turn.kv_cache_report),
        false,
    );
    assert_eq!(cache_obs.state, CacheState::WarmVerified);

    let pricing = find_pricing_profile(ProviderKind::OpenAI, "gpt-4o-mini");

    let op = MutationOp::PinContext {
        label: "REMINDER".into(),
        content: "Always write concise helper signatures.".into(),
        source_node_id: None,
    };

    let decision = evaluate_decision(
        Some("pin_tail_1"),
        Some("cache_safe_boost"),
        &op,
        &turn,
        &cache_obs,
        pricing.as_ref(),
        3,
    );

    // Append-only pin preserves prefix bit-for-bit (0 KV break) but has no standalone token reduction
    assert_eq!(decision.decision, OptimizationDecision::NoOp);
    assert_eq!(
        decision.reason_code,
        DecisionReasonCode::TailAppendPreservesPrefixNoDirectSaving
    );
    assert_eq!(decision.estimated_kv_break_tokens, 0);
}

#[test]
fn test_policy_3_unknown_cache_telemetry_defers() {
    let turn = make_synthetic_turn("sess_unknown_1", None, false);
    let prefix_fp = compute_prefix_fingerprint(&turn.nodes);

    let cache_obs = classify_cache_state(
        &turn.session_id,
        ProviderKind::OpenAI,
        "gpt-4o-mini",
        turn.sequence_number,
        Some(&prefix_fp),
        None,
        Some(&turn.kv_cache_report),
        false,
    );
    assert_eq!(cache_obs.state, CacheState::Unknown);

    let pricing = find_pricing_profile(ProviderKind::OpenAI, "gpt-4o-mini");

    let op = MutationOp::PruneOrStub {
        target_fingerprint: None,
        target_call_id: Some("call_exec_1".into()),
        target_json_pointer: None,
        stub_text: "[cw:pruned]".into(),
    };

    let decision = evaluate_decision(
        Some("prune_unknown"),
        Some("plan_1"),
        &op,
        &turn,
        &cache_obs,
        pricing.as_ref(),
        3,
    );

    assert_eq!(decision.decision, OptimizationDecision::Defer);
    assert_eq!(
        decision.reason_code,
        DecisionReasonCode::CacheTelemetryUnknown
    );
}

#[test]
fn test_policy_4_stale_or_incompatible_cache_observation() {
    let turn = make_synthetic_turn("sess_diverged_2", Some(1500), true);

    let prior_obs = CacheObservation {
        state: CacheState::WarmVerified,
        provenance: TelemetryProvenance::ObservedProvider,
        observed_cached_tokens: Some(1500),
        reported_input_tokens: Some(2000),
        provider: ProviderKind::Anthropic, // Different provider!
        model: "claude-3-5-sonnet".into(),
        session_id: "sess_diverged_1".into(), // Different session!
        prefix_fingerprint: Some("old_prefix_fp_12345678".into()),
        observed_at: Utc::now(),
        turn_seq: 1,
        notes: Vec::new(),
    };

    let new_fp = "new_prefix_fp_87654321";
    let cache_obs = classify_cache_state(
        &turn.session_id,
        ProviderKind::OpenAI,
        "gpt-4o-mini",
        turn.sequence_number,
        Some(new_fp),
        Some(&prior_obs),
        Some(&turn.kv_cache_report),
        false,
    );

    // Mismatches are documented in notes
    assert!(
        cache_obs
            .notes
            .iter()
            .any(|n| n.contains("Session ID mismatch"))
    );
    assert!(
        cache_obs
            .notes
            .iter()
            .any(|n| n.contains("Provider/model changed"))
    );
    assert!(
        cache_obs
            .notes
            .iter()
            .any(|n| n.contains("Prefix fingerprint diverged"))
    );
}

#[test]
fn test_policy_5_cold_cache_beneficial_candidate_applies() {
    // Provider confirmed 0 cached tokens on cache-eligible prompt (>1024 tokens)
    let turn = make_synthetic_turn("sess_cold_1", Some(0), true);
    let prefix_fp = compute_prefix_fingerprint(&turn.nodes);

    let cache_obs = classify_cache_state(
        &turn.session_id,
        ProviderKind::OpenAI,
        "gpt-4o-mini",
        turn.sequence_number,
        Some(&prefix_fp),
        None,
        Some(&turn.kv_cache_report),
        false,
    );
    assert_eq!(cache_obs.state, CacheState::ColdLikely);
    assert_eq!(cache_obs.provenance, TelemetryProvenance::ObservedProvider);

    let pricing = find_pricing_profile(ProviderKind::OpenAI, "gpt-4o-mini");

    let op = MutationOp::PruneOrStub {
        target_fingerprint: None,
        target_call_id: Some("call_exec_1".into()),
        target_json_pointer: None,
        stub_text: "[cw:pruned]".into(),
    };

    let decision = evaluate_decision(
        Some("prune_cold_1"),
        Some("plan_cold"),
        &op,
        &turn,
        &cache_obs,
        pricing.as_ref(),
        3,
    );

    assert_eq!(decision.decision, OptimizationDecision::Apply);
    assert_eq!(
        decision.reason_code,
        DecisionReasonCode::EconomicBenefitVerified
    );
    assert!(decision.net_token_delta > 0);
    assert!(
        decision
            .economic_evaluation
            .as_ref()
            .unwrap()
            .net_benefit_usd
            > 0.0
    );
}

#[test]
fn test_policy_6_missing_or_unsupported_pricing_defers() {
    let turn = make_synthetic_turn("sess_cold_unsupported", Some(0), true);
    let prefix_fp = compute_prefix_fingerprint(&turn.nodes);

    let cache_obs = classify_cache_state(
        &turn.session_id,
        ProviderKind::OpenAICompatible,
        "custom-local-model",
        turn.sequence_number,
        Some(&prefix_fp),
        None,
        Some(&turn.kv_cache_report),
        false,
    );

    // Unsupported pricing lookup
    let pricing = find_pricing_profile(ProviderKind::OpenAICompatible, "custom-local-model");
    assert!(pricing.is_none());

    let op = MutationOp::PruneOrStub {
        target_fingerprint: None,
        target_call_id: Some("call_exec_1".into()),
        target_json_pointer: None,
        stub_text: "[cw:pruned]".into(),
    };

    let decision = evaluate_decision(
        Some("prune_unsupported"),
        Some("plan_1"),
        &op,
        &turn,
        &cache_obs,
        None,
        3,
    );

    assert_eq!(decision.decision, OptimizationDecision::Defer);
    assert_eq!(
        decision.reason_code,
        DecisionReasonCode::MissingPricingProfile
    );
    assert_eq!(
        decision.economic_evaluation.as_ref().unwrap().confidence,
        EvaluationConfidence::Unverified
    );
}

#[test]
fn test_policy_7_multiple_mutations_overlapping_kv_break() {
    let payload = json!({
        "model": "gpt-4o-mini",
        "messages": [
            { "role": "system", "content": "You are a helpful coding assistant." },
            { "role": "user", "content": "Run tests and check git status" },
            {
                "role": "assistant",
                "content": null,
                "tool_calls": [
                    { "id": "call_1", "type": "function", "function": { "name": "exec_command", "arguments": "{\"cmd\":\"test\"}" } },
                    { "id": "call_2", "type": "function", "function": { "name": "exec_command", "arguments": "{\"cmd\":\"git status\"}" } }
                ]
            },
            { "role": "tool", "tool_call_id": "call_1", "content": "test output logs line\n".repeat(30) },
            { "role": "tool", "tool_call_id": "call_2", "content": "git status logs line\n".repeat(30) },
            { "role": "assistant", "content": "Both finished." },
            { "role": "user", "content": "Proceed." }
        ]
    });

    let (turn, _) = analyze_turn_from_payload(
        "turn_overlap".into(),
        "sess_overlap".into(),
        None,
        2,
        &payload,
        ProviderKind::OpenAI,
        None,
        Some(1500),
        None,
    )
    .unwrap();

    let prefix_fp = compute_prefix_fingerprint(&turn.nodes);

    let cache_obs = classify_cache_state(
        &turn.session_id,
        ProviderKind::OpenAI,
        "gpt-4o-mini",
        turn.sequence_number,
        Some(&prefix_fp),
        None,
        Some(&turn.kv_cache_report),
        false,
    );

    let pricing = find_pricing_profile(ProviderKind::OpenAI, "gpt-4o-mini");

    let item1 = OptimizationCandidateItem {
        item_id: "c1".into(),
        node_id: turn.nodes.get(3).map(|n| n.id.clone()),
        turn_seq: turn.sequence_number,
        timestamp: None,
        title: "Candidate 1".into(),
        action_kind: "cut_output".into(),
        what_we_do: "Prune".into(),
        why_it_matters: "Diet".into(),
        content_preview: "".into(),
        tokens_before: 500,
        tokens_after: 20,
        saved_tokens: 480,
        kv_break_tokens: 600,
        mutation_op: MutationOp::PruneOrStub {
            target_fingerprint: None,
            target_call_id: Some("call_1".into()),
            target_json_pointer: None,
            stub_text: "[pruned]".into(),
        },
    };

    let item2 = OptimizationCandidateItem {
        item_id: "c2".into(),
        node_id: turn.nodes.get(4).map(|n| n.id.clone()),
        turn_seq: turn.sequence_number,
        timestamp: None,
        title: "Candidate 2".into(),
        action_kind: "cut_output".into(),
        what_we_do: "Prune".into(),
        why_it_matters: "Diet".into(),
        content_preview: "".into(),
        tokens_before: 200,
        tokens_after: 20,
        saved_tokens: 180,
        kv_break_tokens: 300,
        mutation_op: MutationOp::PruneOrStub {
            target_fingerprint: None,
            target_call_id: Some("call_2".into()),
            target_json_pointer: None,
            stub_text: "[pruned]".into(),
        },
    };

    let plan = OptimizationStrategyPlan {
        id: "multi_candidate_plan".into(),
        name: "Test Plan".into(),
        tagline: "".into(),
        kv_badge: "".into(),
        what_and_how: "".into(),
        expected_quality_impact: "".into(),
        tokens_before: 700,
        tokens_after: 40,
        saved_tokens: 660,
        saved_pct: 90.0,
        kv_cache_break_tokens: 600,
        break_even_turns: 2,
        health_score_before: 60,
        health_score_after: 90,
        items: vec![item1, item2],
    };

    let (plan_dec, item_decs) =
        evaluate_plan_decisions(&plan, &turn, &cache_obs, pricing.as_ref(), 3);

    assert_eq!(item_decs.len(), 2);
    // In warm cache, historical mutations defer to preserve prefix
    assert_eq!(plan_dec.decision, OptimizationDecision::Defer);
    assert_eq!(plan_dec.reason_code, DecisionReasonCode::CachePrefixAtRisk);
}

#[test]
fn test_policy_8_unsafe_tool_lifecycle_protection_defers() {
    let payload = json!({
        "model": "gpt-4o-mini",
        "messages": [
            { "role": "user", "content": "Fetch latest metrics." },
            {
                "role": "assistant",
                "content": null,
                "tool_calls": [
                    { "id": "call_active_tail", "type": "function", "function": { "name": "exec_cmd", "arguments": "{}" } }
                ]
            },
            {
                "role": "tool",
                "tool_call_id": "call_active_tail",
                "content": "output from active tool awaiting assistant interpretation"
            }
        ]
    });

    let (turn, _) = analyze_turn_from_payload(
        "turn_lifecycle".into(),
        "sess_life".into(),
        None,
        1,
        &payload,
        ProviderKind::OpenAI,
        None,
        Some(1200),
        None,
    )
    .unwrap();

    let prefix_fp = compute_prefix_fingerprint(&turn.nodes);
    let cache_obs = classify_cache_state(
        &turn.session_id,
        ProviderKind::OpenAI,
        "gpt-4o-mini",
        turn.sequence_number,
        Some(&prefix_fp),
        None,
        Some(&turn.kv_cache_report),
        false,
    );

    let op = MutationOp::PruneOrStub {
        target_fingerprint: None,
        target_call_id: Some("call_active_tail".into()),
        target_json_pointer: None,
        stub_text: "[pruned]".into(),
    };

    let decision = evaluate_decision(
        Some("prune_active"),
        Some("plan_1"),
        &op,
        &turn,
        &cache_obs,
        find_pricing_profile(ProviderKind::OpenAI, "gpt-4o-mini").as_ref(),
        3,
    );

    // The tool result is at prompt tail awaiting assistant response -> ActiveToolLifecycleProtection
    assert_eq!(decision.decision, OptimizationDecision::Defer);
    assert_eq!(
        decision.reason_code,
        DecisionReasonCode::ActiveToolLifecycleProtection
    );
}

#[test]
fn test_policy_9_zero_or_negative_token_savings_noops() {
    let turn = make_synthetic_turn("sess_cold_zero", Some(0), true);
    let prefix_fp = compute_prefix_fingerprint(&turn.nodes);

    let cache_obs = classify_cache_state(
        &turn.session_id,
        ProviderKind::OpenAI,
        "gpt-4o-mini",
        turn.sequence_number,
        Some(&prefix_fp),
        None,
        Some(&turn.kv_cache_report),
        false,
    );
    assert_eq!(cache_obs.state, CacheState::ColdLikely);

    let pricing = find_pricing_profile(ProviderKind::OpenAI, "gpt-4o-mini");

    // Mutation with nonexistent target -> saves 0 tokens
    let op = MutationOp::PruneOrStub {
        target_fingerprint: Some("nonexistent_fp".into()),
        target_call_id: None,
        target_json_pointer: None,
        stub_text: "stub".into(),
    };

    let decision = evaluate_decision(
        Some("prune_nonexistent"),
        Some("plan_1"),
        &op,
        &turn,
        &cache_obs,
        pricing.as_ref(),
        3,
    );

    assert_eq!(decision.decision, OptimizationDecision::NoOp);
    assert_eq!(
        decision.reason_code,
        DecisionReasonCode::NoDemonstrableBenefit
    );
}

#[test]
fn test_policy_10_incomplete_turn_analysis_rejects() {
    let mut turn = make_synthetic_turn("sess_incomplete", Some(1500), true);
    turn.analysis_complete = false;

    let prefix_fp = compute_prefix_fingerprint(&turn.nodes);
    let cache_obs = classify_cache_state(
        &turn.session_id,
        ProviderKind::OpenAI,
        "gpt-4o-mini",
        turn.sequence_number,
        Some(&prefix_fp),
        None,
        Some(&turn.kv_cache_report),
        false,
    );

    let op = MutationOp::PinContext {
        label: "REMINDER".into(),
        content: "test".into(),
        source_node_id: None,
    };

    let decision = evaluate_decision(
        Some("cand_1"),
        Some("plan_1"),
        &op,
        &turn,
        &cache_obs,
        find_pricing_profile(ProviderKind::OpenAI, "gpt-4o-mini").as_ref(),
        3,
    );

    assert_eq!(decision.decision, OptimizationDecision::Reject);
    assert_eq!(
        decision.reason_code,
        DecisionReasonCode::IncompleteContextAnalysis
    );
}

#[test]
fn test_policy_11_feature_flag_disabled_leaves_behavior_unchanged() {
    let turn = make_synthetic_turn("sess_ap_off", Some(1500), true);
    let mut payload = json!({
        "messages": [
            { "role": "user", "content": "Hello world" }
        ]
    });
    let original = payload.clone();

    let config = AutoPilotConfig {
        enabled: false,
        shadow_policy: ShadowPolicyConfig {
            enabled: false,
            horizon_turns: 3,
            record_diagnostics: false,
        },
        ..Default::default()
    };

    let report = evaluate_autopilot_policies(
        &config,
        &turn.session_id,
        "gpt-4o-mini",
        ProviderKind::OpenAI,
        turn.sequence_number,
        &turn.nodes,
        Some(&turn),
        &mut payload,
    );

    assert!(report.triggered_policies.is_empty());
    assert!(report.generated_mutations.is_empty());
    assert!(report.shadow_decisions.is_empty());
    assert_eq!(payload, original);
}

#[test]
fn test_policy_12_shadow_mode_enabled_records_decisions_without_mutations() {
    let turn = make_synthetic_turn("sess_shadow_on", Some(1500), true);
    let mut payload = json!({
        "messages": [
            { "role": "user", "content": "Hello world" }
        ]
    });
    let original = payload.clone();

    let config = AutoPilotConfig {
        enabled: false, // Active mutations DISABLED
        shadow_policy: ShadowPolicyConfig {
            enabled: true, // Shadow mode ENABLED
            horizon_turns: 3,
            record_diagnostics: true,
        },
        ..Default::default()
    };

    let report = evaluate_autopilot_policies(
        &config,
        &turn.session_id,
        "gpt-4o-mini",
        ProviderKind::OpenAI,
        turn.sequence_number,
        &turn.nodes,
        Some(&turn),
        &mut payload,
    );

    // Decisions must be recorded
    assert!(!report.shadow_decisions.is_empty());
    assert!(
        report
            .triggered_policies
            .iter()
            .any(|p| p.contains("shadow_cache_policy_evaluator"))
    );

    // Active mutations must be ZERO
    assert!(report.generated_mutations.is_empty());

    // Payload must be 100% byte-for-byte identical (unmutated)
    assert_eq!(payload, original);

    // Verify diagnostic summary formatting
    let dec = &report.shadow_decisions[0];
    let summary = dec.compact_diagnostic_summary();
    assert!(summary.contains("Actual mutation applied by shadow policy: false"));
}

#[test]
fn test_policy_pricing_profiles_anthropic_write_surcharge_calculation() {
    // Anthropic pricing has +25% write surcharge ($3.75 / Mtok) and 90% read discount ($0.30 / Mtok)
    let profile = find_pricing_profile(ProviderKind::Anthropic, "claude-3-5-sonnet").unwrap();
    assert_eq!(profile.cached_input_read_per_mtok, 0.30);
    assert_eq!(profile.cache_write_per_mtok, 3.75);

    let obs_warm = CacheObservation {
        state: CacheState::WarmVerified,
        provenance: TelemetryProvenance::ObservedProvider,
        observed_cached_tokens: Some(10_000),
        reported_input_tokens: Some(12_000),
        provider: ProviderKind::Anthropic,
        model: "claude-3-5-sonnet".into(),
        session_id: "sess_anthropic".into(),
        prefix_fingerprint: None,
        observed_at: Utc::now(),
        turn_seq: 2,
        notes: Vec::new(),
    };

    // Prune 2,000 tokens breaking prefix at token 2,000 out of 12,000
    let econ = evaluate_economic_impact(
        12_000,
        2_000,
        Some(2_000),
        false,
        &obs_warm,
        Some(&profile),
        1, // 1 turn horizon
    );

    // On Turn 1, breaking 8,000 warm tokens causes re-reading 8,000 tokens uncached ($3.00)
    // PLUS writing them to cache ($3.75), which costs far more than reading them cached ($0.30).
    // Economic net benefit on Turn 1 is negative!
    assert!(econ.net_benefit_usd < 0.0);
    assert!(
        econ.reasons
            .iter()
            .any(|r| r.contains("Warm cache prefix broken"))
    );
}

#[test]
fn test_policy_pricing_profiles_gemini_threshold_handling() {
    let profile = find_pricing_profile(ProviderKind::Gemini, "gemini-1.5-flash").unwrap();
    assert_eq!(profile.min_cache_threshold_tokens, 32768);

    let turn = make_synthetic_turn("sess_gemini_small", Some(0), true);
    let cache_obs = classify_cache_state(
        &turn.session_id,
        ProviderKind::Gemini,
        "gemini-1.5-flash",
        turn.sequence_number,
        None,
        None,
        Some(&turn.kv_cache_report),
        false,
    );

    // Input tokens (~1500) < 32768 threshold -> Unknown eligibility rather than ColdLikely
    assert_eq!(cache_obs.state, CacheState::Unknown);
    assert!(
        cache_obs
            .notes
            .iter()
            .any(|n| n.contains("below provider caching threshold"))
    );
}
