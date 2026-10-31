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

    // Incompatible evidence fails closed to Unknown
    assert_eq!(cache_obs.state, CacheState::Unknown);
    assert_eq!(cache_obs.provenance, TelemetryProvenance::None);
    assert!(
        cache_obs
            .notes
            .iter()
            .any(|n| n.contains("Failing closed to Unknown"))
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
        DecisionReasonCode::ModeledEconomicBenefitPositive
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
            strict_shadow_only: true,
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
            strict_shadow_only: true,
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

#[test]
fn test_policy_p0_plan_defers_when_candidate_deferred_for_lifecycle() {
    // P0 Review Finding: A positive financial projection on a cold cache must NEVER override
    // a candidate's lifecycle or safety deferral.
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
                "content": "output from active tool awaiting assistant interpretation with long execution log stream\n".repeat(120)
            }
        ]
    });

    let (turn, _) = analyze_turn_from_payload(
        "turn_lifecycle_p0".into(),
        "sess_life_p0".into(),
        None,
        1,
        &payload,
        ProviderKind::OpenAI,
        None,
        Some(0), // Cold cache
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
    assert_eq!(cache_obs.state, CacheState::ColdLikely);

    let pricing = find_pricing_profile(ProviderKind::OpenAI, "gpt-4o-mini");

    let candidate = OptimizationCandidateItem {
        item_id: "prune_protected_tail".into(),
        node_id: turn.nodes.last().map(|n| n.id.clone()),
        turn_seq: turn.sequence_number,
        timestamp: None,
        title: "Prune Active Tail".into(),
        action_kind: "cut_output".into(),
        what_we_do: "Prune".into(),
        why_it_matters: "Diet".into(),
        content_preview: "".into(),
        tokens_before: 500,
        tokens_after: 20,
        saved_tokens: 480, // Substantial positive token savings
        kv_break_tokens: 0,
        mutation_op: MutationOp::PruneOrStub {
            target_fingerprint: None,
            target_call_id: Some("call_active_tail".into()),
            target_json_pointer: None,
            stub_text: "[pruned]".into(),
        },
    };

    let plan = OptimizationStrategyPlan {
        id: "plan_with_protected_candidate".into(),
        name: "Aggressive Plan".into(),
        tagline: "".into(),
        kv_badge: "".into(),
        what_and_how: "".into(),
        expected_quality_impact: "".into(),
        tokens_before: 500,
        tokens_after: 20,
        saved_tokens: 480,
        saved_pct: 96.0,
        kv_cache_break_tokens: 0,
        break_even_turns: 1,
        health_score_before: 50,
        health_score_after: 90,
        items: vec![candidate],
    };

    let (plan_dec, item_decs) =
        evaluate_plan_decisions(&plan, &turn, &cache_obs, pricing.as_ref(), 3);

    // Candidate is deferred for lifecycle protection
    assert_eq!(item_decs[0].decision, OptimizationDecision::Defer);
    assert_eq!(
        item_decs[0].reason_code,
        DecisionReasonCode::ActiveToolLifecycleProtection
    );

    // Plan-level decision MUST be Defer, NOT Apply, despite ColdLikely cache and positive savings!
    assert_eq!(plan_dec.decision, OptimizationDecision::Defer);
    assert_eq!(
        plan_dec.reason_code,
        DecisionReasonCode::ActiveToolLifecycleProtection
    );
    assert!(
        plan_dec
            .explanation
            .contains("Plan deferred because candidate")
    );
}

#[test]
fn test_policy_zero_length_input_fails_closed_to_unknown() {
    let report = KvCacheReport {
        hit_tokens: 0,
        newly_cached_tokens: 0,
        invalidated_tokens: 0,
        uncached_tail_tokens: 0,
        hit_ratio: 0.0,
        divergence_token_index: None,
        invalidation_cause: None,
        telemetry_available: true,
    };

    let obs = classify_cache_state(
        "sess_zero",
        ProviderKind::OpenAI,
        "gpt-4o-mini",
        1,
        None,
        None,
        Some(&report),
        false,
    );

    // Zero-length input prompt must fail closed to Unknown, never ColdLikely
    assert_eq!(obs.state, CacheState::Unknown);
    assert!(
        obs.notes
            .iter()
            .any(|n| n.contains("below provider caching threshold"))
    );
}

#[test]
fn test_policy_loose_or_unverified_model_names_rejected() {
    // Unverified model strings and variants sharing prefixes must NOT receive pricing profiles
    assert!(find_pricing_profile(ProviderKind::OpenAI, "gpt-4o-custom-finetune").is_none());
    assert!(find_pricing_profile(ProviderKind::OpenAI, "gpt-4o-mini-experimental").is_none());
    assert!(find_pricing_profile(ProviderKind::OpenAI, "gpt-4o-custom").is_none());
    assert!(find_pricing_profile(ProviderKind::Anthropic, "custom-sonnet-v1").is_none());
    assert!(find_pricing_profile(ProviderKind::Anthropic, "claude-2-sonnet").is_none());
    assert!(
        find_pricing_profile(ProviderKind::Anthropic, "claude-3-5-sonnet-custom-tuning").is_none()
    );
    assert!(find_pricing_profile(ProviderKind::Anthropic, "claude-3-5-sonnet-preview").is_none());
    assert!(find_pricing_profile(ProviderKind::Gemini, "fine-tune-flash").is_none());
    assert!(find_pricing_profile(ProviderKind::Gemini, "gemini-1.0-pro").is_none());
    assert!(find_pricing_profile(ProviderKind::Gemini, "gemini-1.5-pro-preview-0514").is_none());
    assert!(find_pricing_profile(ProviderKind::Gemini, "gemini-1.5-pro-experimental").is_none());
    assert!(find_pricing_profile(ProviderKind::Gemini, "gemini-1.5-flash-tuning").is_none());

    // Verified explicit models and dated aliases must match correctly
    assert!(find_pricing_profile(ProviderKind::OpenAI, "gpt-4o-mini-2024-07-18").is_some());
    assert!(find_pricing_profile(ProviderKind::Anthropic, "claude-3-5-sonnet-20241022").is_some());
    assert!(find_pricing_profile(ProviderKind::Anthropic, "claude-3-5-sonnet-latest").is_some());
    assert!(find_pricing_profile(ProviderKind::Anthropic, "claude-3-haiku-20240307").is_some());
    assert!(find_pricing_profile(ProviderKind::Gemini, "gemini-1.5-flash-latest").is_some());
    assert!(find_pricing_profile(ProviderKind::Gemini, "gemini-1.5-pro-002").is_some());
}

#[test]
fn test_policy_strict_shadow_only_suppresses_active_mutations() {
    let turn = make_synthetic_turn("sess_strict_shadow", Some(1500), true);
    let mut payload = json!({
        "messages": [
            { "role": "user", "content": "Hello" }
        ]
    });
    let original = payload.clone();

    // Autopilot active enabled = true, BUT shadow_policy strict_shadow_only = true
    let config = AutoPilotConfig {
        enabled: true,
        auto_cache_safe_pin: true,
        pin_min_context_tokens: 100, // Would normally trigger auto pin
        shadow_policy: ShadowPolicyConfig {
            enabled: true,
            strict_shadow_only: true, // Strict shadow mode
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

    // Shadow decisions recorded
    assert!(!report.shadow_decisions.is_empty());

    // Active mutations suppressed
    assert!(report.generated_mutations.is_empty());

    // Payload 100% byte-for-byte unmutated
    assert_eq!(payload, original);
}

#[test]
fn test_policy_signed_thinking_block_rejection_candidate_and_plan() {
    // P1 Safety Invariant: The decision engine must explicitly reject candidates targeting
    // signed thinking content, preventing policy Apply for anything the mutation engine refuses.
    let payload = json!({
        "model": "claude-3-5-sonnet-20241022",
        "messages": [
            { "role": "user", "content": "Explain KV caching." },
            {
                "role": "assistant",
                "content": [
                    {
                        "type": "thinking",
                        "thinking": "Internal chain-of-thought about radix trees and attention cache residency.",
                        "signature": "sig_cryptographic_verification_token_anthropic_42"
                    },
                    {
                        "type": "text",
                        "content": "KV caching stores key-value projections across attention layers."
                    }
                ]
            }
        ]
    });

    let (turn, _) = analyze_turn_from_payload(
        "turn_signed_thinking".into(),
        "sess_signed_thinking".into(),
        None,
        1,
        &payload,
        ProviderKind::Anthropic,
        None,
        Some(0), // Cold cache
        None,
    )
    .unwrap();

    let thinking_node = turn
        .nodes
        .iter()
        .find(|n| {
            matches!(
                n.kind,
                NodeKind::AssistantThinking {
                    signature: Some(_),
                    ..
                }
            )
        })
        .expect("Signed thinking node must exist in parsed turn");

    let prefix_fp = compute_prefix_fingerprint(&turn.nodes);
    let cache_obs = classify_cache_state(
        &turn.session_id,
        ProviderKind::Anthropic,
        "claude-3-5-sonnet-20241022",
        turn.sequence_number,
        Some(&prefix_fp),
        None,
        Some(&turn.kv_cache_report),
        false,
    );

    let pricing = find_pricing_profile(ProviderKind::Anthropic, "claude-3-5-sonnet-20241022");

    let candidate = OptimizationCandidateItem {
        item_id: format!("cut_think_{}", thinking_node.id),
        node_id: Some(thinking_node.id.clone()),
        turn_seq: thinking_node.turn_seq,
        timestamp: None,
        title: "Prune Signed Thinking".into(),
        action_kind: "prune_thinking".into(),
        what_we_do: "Prune".into(),
        why_it_matters: "Diet".into(),
        content_preview: "".into(),
        tokens_before: 200,
        tokens_after: 10,
        saved_tokens: 190,
        kv_break_tokens: 0,
        mutation_op: MutationOp::PruneOrStub {
            target_fingerprint: Some(thinking_node.fingerprint.0.clone()),
            target_call_id: None,
            target_json_pointer: Some(thinking_node.raw_json_pointer.clone()),
            stub_text: "[pruned]".into(),
        },
    };

    let plan = OptimizationStrategyPlan {
        id: "plan_with_signed_thinking".into(),
        name: "Signed Thinking Plan".into(),
        tagline: "".into(),
        kv_badge: "".into(),
        what_and_how: "".into(),
        expected_quality_impact: "".into(),
        tokens_before: 200,
        tokens_after: 10,
        saved_tokens: 190,
        saved_pct: 95.0,
        kv_cache_break_tokens: 0,
        break_even_turns: 1,
        health_score_before: 50,
        health_score_after: 90,
        items: vec![candidate.clone()],
    };

    // 1. Candidate-level evaluation: MUST REJECT with SafetyViolation
    let candidate_decision = evaluate_decision(
        Some(&candidate.item_id),
        Some(&plan.id),
        &candidate.mutation_op,
        &turn,
        &cache_obs,
        pricing.as_ref(),
        3,
    );
    assert_eq!(candidate_decision.decision, OptimizationDecision::Reject);
    assert_eq!(
        candidate_decision.reason_code,
        DecisionReasonCode::SafetyViolation
    );
    assert!(candidate_decision.explanation.contains("signed thinking"));

    // 2. Plan-level evaluation: MUST REJECT with SafetyViolation
    let (plan_decision, item_decisions) =
        evaluate_plan_decisions(&plan, &turn, &cache_obs, pricing.as_ref(), 3);
    assert_eq!(item_decisions[0].decision, OptimizationDecision::Reject);
    assert_eq!(plan_decision.decision, OptimizationDecision::Reject);
    assert_eq!(
        plan_decision.reason_code,
        DecisionReasonCode::SafetyViolation
    );
    assert!(plan_decision.explanation.contains("rejected"));
}

#[test]
fn test_policy_plan_rebuilding_and_cold_cache_no_benefit_reason_codes() {
    let turn = make_synthetic_turn("sess_codes", Some(0), true);
    let prefix_fp = compute_prefix_fingerprint(&turn.nodes);

    // Case 1: Rebuilding cache state must return CacheRebuilding, NOT CacheTelemetryUnknown
    let rebuilding_obs = CacheObservation {
        state: CacheState::Rebuilding,
        provenance: TelemetryProvenance::ObservedProvider,
        observed_cached_tokens: Some(500),
        reported_input_tokens: Some(1500),
        provider: ProviderKind::OpenAI,
        model: "gpt-4o-mini".to_string(),
        session_id: turn.session_id.clone(),
        prefix_fingerprint: Some(prefix_fp.clone()),
        observed_at: Utc::now(),
        turn_seq: turn.sequence_number,
        notes: vec!["Prefix rebuilding".to_string()],
    };
    let pricing = find_pricing_profile(ProviderKind::OpenAI, "gpt-4o-mini");

    let plan = OptimizationStrategyPlan {
        id: "plan_rebuilding".into(),
        name: "Test Plan".into(),
        tagline: "".into(),
        kv_badge: "".into(),
        what_and_how: "".into(),
        expected_quality_impact: "".into(),
        tokens_before: 500,
        tokens_after: 200,
        saved_tokens: 300,
        saved_pct: 60.0,
        kv_cache_break_tokens: 100,
        break_even_turns: 1,
        health_score_before: 50,
        health_score_after: 90,
        items: vec![],
    };

    let (rebuilding_decision, _) =
        evaluate_plan_decisions(&plan, &turn, &rebuilding_obs, pricing.as_ref(), 3);
    assert_eq!(rebuilding_decision.decision, OptimizationDecision::Defer);
    assert_eq!(
        rebuilding_decision.reason_code,
        DecisionReasonCode::CacheRebuilding
    );

    // Case 2: Cold cache with zero savings must return NoDemonstrableBenefit, NOT CacheTelemetryUnknown
    let cold_obs = CacheObservation {
        state: CacheState::ColdLikely,
        provenance: TelemetryProvenance::ObservedProvider,
        observed_cached_tokens: Some(0),
        reported_input_tokens: Some(1500),
        provider: ProviderKind::OpenAI,
        model: "gpt-4o-mini".to_string(),
        session_id: turn.session_id.clone(),
        prefix_fingerprint: Some(prefix_fp),
        observed_at: Utc::now(),
        turn_seq: turn.sequence_number,
        notes: vec![],
    };

    let zero_savings_plan = OptimizationStrategyPlan {
        id: "plan_zero_savings".into(),
        name: "Zero Savings Plan".into(),
        tagline: "".into(),
        kv_badge: "".into(),
        what_and_how: "".into(),
        expected_quality_impact: "".into(),
        tokens_before: 500,
        tokens_after: 500,
        saved_tokens: 0,
        saved_pct: 0.0,
        kv_cache_break_tokens: 0,
        break_even_turns: 1,
        health_score_before: 50,
        health_score_after: 50,
        items: vec![],
    };

    let (zero_decision, _) =
        evaluate_plan_decisions(&zero_savings_plan, &turn, &cold_obs, pricing.as_ref(), 3);
    assert_eq!(zero_decision.decision, OptimizationDecision::NoOp);
    assert_eq!(
        zero_decision.reason_code,
        DecisionReasonCode::NoDemonstrableBenefit
    );
}
