use chrono::Utc;
use cw_core::{
    analyze_turn_from_payload,
    ir::{
        FirewallAction, FirewallRule, FirewallTrigger, LintRuleId, MutationOp, MutationRule,
        MutationStatus, ProviderKind,
    },
    mutation::{apply_mutations_to_payload, evaluate_firewall},
    normalizer::detect_provider,
    radix::build_radix_graph,
};
use serde_json::Value;

#[test]
fn test_recorded_anthropic_fixture() {
    let raw_str = include_str!("fixtures/anthropic/turn1.json");
    let payload: Value = serde_json::from_str(raw_str).unwrap();
    assert_eq!(
        detect_provider("/anthropic/v1/messages", &payload),
        ProviderKind::Anthropic
    );

    let (turn, files) = analyze_turn_from_payload(
        "t_anth".into(),
        "s_anth".into(),
        None,
        1,
        &payload,
        ProviderKind::Anthropic,
        None,
        Some(40),
        None,
    )
    .unwrap();

    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "src/lib.rs");
    let graph = build_radix_graph(&turn, std::slice::from_ref(&turn));
    assert!(!graph.nodes.is_empty());
    assert!(!graph.edges.is_empty());
}

#[test]
fn test_recorded_openai_fixture_and_failed_loop_folding() {
    let raw_str = include_str!("fixtures/openai/turn1.json");
    let mut payload: Value = serde_json::from_str(raw_str).unwrap();
    assert_eq!(
        detect_provider("/openai/v1/chat/completions", &payload),
        ProviderKind::OpenAI
    );

    let (turn, _) = analyze_turn_from_payload(
        "t_oai".into(),
        "s_oai".into(),
        None,
        4,
        &payload,
        ProviderKind::OpenAI,
        None,
        None,
        None,
    )
    .unwrap();

    // Verify CW-L004 (FailedDebuggingLoop) triggers on 3 consecutive failed `cargo test` commands
    assert!(
        turn.lint_findings
            .iter()
            .any(|f| f.rule_id == LintRuleId::FailedDebuggingLoop)
    );

    // Apply FoldSubtree mutation and verify intermediate failed tool results are folded
    let fold_rule = MutationRule {
        id: "m_fold".into(),
        session_id: "s_oai".into(),
        op: MutationOp::FoldSubtree {
            start_turn: 1,
            end_turn: 4,
            summary_text: "3 cargo test failures resolved.".into(),
        },
        status: MutationStatus::Committed,
        estimated_saved_tokens: 50,
        estimated_kv_break_tokens: 0,
        created_at: Utc::now(),
    };
    let rep = apply_mutations_to_payload(&mut payload, ProviderKind::OpenAI, &[fold_rule], 4);
    assert_eq!(rep.applied_rule_ids, vec!["m_fold".to_string()]);
    assert!(
        payload["messages"][3]["content"]
            .as_str()
            .unwrap()
            .contains("[cw:folded_subtree")
    );
}

#[test]
fn test_recorded_gemini_fixture_preserves_protected_read() {
    let raw_str = include_str!("fixtures/gemini/turn1.json");
    let mut payload: Value = serde_json::from_str(raw_str).unwrap();
    assert_eq!(
        detect_provider("/v1beta/models/gemini:generateContent", &payload),
        ProviderKind::Gemini
    );

    let (turn, files) = analyze_turn_from_payload(
        "t_gem".into(),
        "s_gem".into(),
        None,
        1,
        &payload,
        ProviderKind::Gemini,
        None,
        None,
        None,
    )
    .unwrap();

    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "Cargo.toml");

    let original = payload.clone();
    // A regex rule cannot override signed-content and file-read protections.
    let fw_rules = vec![FirewallRule {
        id: "fw_1".into(),
        session_id: None,
        name: "Truncate workspace output".into(),
        trigger: FirewallTrigger::ToolResultMatchesRegex {
            pattern: "resolver".into(),
        },
        action: FirewallAction::AutoTruncateHeadTail {
            head_chars: 5,
            tail_chars: 5,
        },
        enabled: true,
    }];

    let verdict = evaluate_firewall(
        &mut payload,
        ProviderKind::Gemini,
        &turn.nodes,
        &turn.kv_cache_report,
        &fw_rules,
        1,
    );
    assert!(verdict.auto_applied_actions.is_empty());
    assert_eq!(payload, original);
}
