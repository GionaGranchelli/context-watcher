use chrono::Utc;
use cw_core::{
    analyze_turn_from_payload, ir::*, mutation::apply_mutations_to_payload,
    normalizer::parse_request_to_nodes,
};
use serde_json::{Value, json};
fn rule(op: MutationOp) -> MutationRule {
    MutationRule {
        id: "safety".into(),
        session_id: "s".into(),
        op,
        status: MutationStatus::Committed,
        estimated_saved_tokens: 0,
        estimated_kv_break_tokens: 0,
        created_at: Utc::now(),
    }
}

#[test]
fn signed_requests_and_no_rule_requests_are_opaque() {
    let mut payload = json!({"messages":[{"role":"user","content":"literal <cw_pinned_context>\\nkeep this\\n</cw_pinned_context>"}]});
    let original = payload.clone();
    apply_mutations_to_payload(&mut payload, ProviderKind::Anthropic, &[], 1);
    assert_eq!(payload, original);
    payload["messages"].as_array_mut().unwrap().push(json!({"role":"assistant","content":[{"type":"thinking","thinking":"signed text","signature":"opaque"}]}));
    let original = payload.clone();
    let r = rule(MutationOp::PinContext {
        label: "reminder".into(),
        content: "added".into(),
        source_node_id: None,
    });
    apply_mutations_to_payload(&mut payload, ProviderKind::Anthropic, &[r], 1);
    assert_eq!(payload, original);
}
#[test]
fn selectors_reject_stale_content_and_system_fragments() {
    let mut payload = json!({"system":"base <RULE[AGENTS.md]>keep rule</RULE[AGENTS.md]> end","messages":[{"role":"user","content":"new content"}]});
    let original = payload.clone();
    for pointer in ["/messages/0/content", "/system#rule-0"] {
        let r = rule(MutationOp::PruneOrStub {
            target_fingerprint: Some("wrong identity".into()),
            target_call_id: Some("old".into()),
            target_json_pointer: Some(pointer.into()),
            stub_text: "removed".into(),
        });
        apply_mutations_to_payload(&mut payload, ProviderKind::Anthropic, &[r], 1);
        assert_eq!(payload, original);
    }
}
fn reads(first: (u32, u32), second: (u32, u32)) -> Value {
    json!({"messages":[
        {"role":"assistant","tool_calls":[{"id":"a","function":{"name":"read_file","arguments":json!({"path":"a.rs","start_line":first.0,"end_line":first.1}).to_string()}}]},
        {"role":"tool","tool_call_id":"a","content":(first.0..=first.1).map(|n|format!("line {n}")).collect::<Vec<_>>().join("\n")},
        {"role":"assistant","tool_calls":[{"id":"b","function":{"name":"read_file","arguments":json!({"path":"a.rs","start_line":second.0,"end_line":second.1}).to_string()}}]},
        {"role":"tool","tool_call_id":"b","content":(second.0..=second.1).map(|n|format!("line {n}")).collect::<Vec<_>>().join("\n")}]})
}
#[test]
fn dedupe_requires_a_retained_covering_range() {
    let mut separate = reads((1, 10), (90, 100));
    let original = separate.clone();
    let r = rule(MutationOp::ShadowDedupe { file_path: None });
    apply_mutations_to_payload(
        &mut separate,
        ProviderKind::OpenAI,
        std::slice::from_ref(&r),
        1,
    );
    assert_eq!(separate, original);
    let mut covered = reads((1, 10), (1, 100));
    apply_mutations_to_payload(&mut covered, ProviderKind::OpenAI, &[r], 1);
    assert!(
        covered["messages"][1]["content"]
            .as_str()
            .unwrap()
            .starts_with("[cw:shadow_deduped")
    );
    assert!(
        covered["messages"][3]["content"]
            .as_str()
            .unwrap()
            .contains("line 100")
    );
}
#[test]
fn responses_inputs_and_call_metadata_are_recognized() {
    let input = parse_request_to_nodes(
        &json!({"input":"actual user prompt"}),
        ProviderKind::OpenAI,
        1,
    )
    .unwrap();
    assert!(matches!(&input[0].kind,NodeKind::UserMessage{text} if text=="actual user prompt"));
    let payload = json!({"input":[{"type":"function_call","call_id":"a","name":"read_file","arguments":"{\"path\":\"a.rs\",\"offset\":90,\"limit\":10}"},{"type":"function_call_output","call_id":"a","output":"code"}]});
    let nodes = parse_request_to_nodes(&payload, ProviderKind::OpenAI, 1).unwrap();
    assert!(matches!(&nodes[0].kind, NodeKind::ToolCall { .. }));
    assert!(matches!(
        &nodes[1].kind,
        NodeKind::ToolResult {
            semantic_meta: Some(ToolSemanticMeta::FileRead {
                start_line: Some(90),
                end_line: Some(99),
                ..
            }),
            ..
        }
    ));
}
#[test]
fn unknown_cache_is_not_a_hit_and_minimum_plan_preserves_history() {
    let payload = reads((1, 10), (1, 100));
    let (mut turn, _) = analyze_turn_from_payload(
        "t".into(),
        "s".into(),
        None,
        1,
        &payload,
        ProviderKind::OpenAI,
        None,
        None,
        None,
    )
    .unwrap();
    // CW-100: When provider telemetry is None, the optimizer must never treat
    // unverified cache state as zero-break cost. Minimum plan ("cache_safe_boost")
    // must strictly preserve history and exclude prefix mutations without requiring
    // manual tampering of node kv_status.
    let plans = cw_core::optimizer::build_optimization_plans(&turn);
    let min = plans.iter().find(|p| p.id == "cache_safe_boost").unwrap();
    assert!(min.items.is_empty(), "Unverified prefix must not qualify for cache_safe_boost");
    assert_eq!(min.kv_cache_break_tokens, 0);

    let previous = turn.nodes.clone();
    let report = cw_core::radix::analyze_kv_cache(
        Some(&previous),
        &mut turn.nodes,
        ProviderKind::OpenAI,
        None,
    );
    assert_eq!(report.hit_tokens, 0);
    assert!(!report.telemetry_available, "Telemetry must be marked unavailable when actual_cached_tokens is None");
    // Prefix-matching nodes without provider telemetry must be marked Unknown
    assert_eq!(turn.nodes[0].kv_status, KvNodeStatus::Unknown);

    // Verify optimizer evaluation with explicit KvNodeStatus::Unknown nodes
    let plans_after_unknown = cw_core::optimizer::build_optimization_plans(&turn);
    let min_unknown = plans_after_unknown.iter().find(|p| p.id == "cache_safe_boost").unwrap();
    assert!(min_unknown.items.is_empty(), "Unknown cache prefix must never be included in cache_safe_boost");
    assert_eq!(min_unknown.kv_cache_break_tokens, 0);

    // Other plans (e.g. clean_code_state) can still identify dedupe opportunities,
    // but their kv_break_tokens must be non-zero (not masquerading as free)!
    let clean = plans_after_unknown.iter().find(|p| p.id == "clean_code_state").unwrap();
    assert!(!clean.items.is_empty());
    assert!(clean.kv_cache_break_tokens > 0, "Unknown cache invalidation must compute non-zero break cost");

    let report = cw_core::radix::analyze_kv_cache(
        None,
        &mut turn.nodes,
        ProviderKind::OpenAI,
        Some(1_000_000),
    );
    assert!(report.hit_ratio <= 1.0);
    assert!(report.telemetry_available);
}
#[test]
fn metadata_stripping_never_removes_application_keys() {
    let mut payload = json!({"messages":[{"cw_turn_seq":1,"role":"assistant","tool_calls":[{"function":{"arguments":{"cw_customer_id":42}}}]}],"tools":[{"parameters":{"properties":{"cw_customer_id":{"type":"integer"}}}}]});
    cw_core::local_llm::strip_internal_cw_fields(&mut payload);
    assert!(payload["messages"][0].get("cw_turn_seq").is_none());
    assert_eq!(
        payload.pointer("/messages/0/tool_calls/0/function/arguments/cw_customer_id"),
        Some(&json!(42))
    );
    assert!(
        payload
            .pointer("/tools/0/parameters/properties/cw_customer_id")
            .is_some()
    );
}

#[test]
fn changed_or_failed_read_cannot_replace_verified_contents() {
    let mut payload = reads((1, 10), (1, 10));
    payload["messages"][3]["content"] = json!("read failed");
    let original = payload.clone();
    apply_mutations_to_payload(
        &mut payload,
        ProviderKind::OpenAI,
        &[rule(MutationOp::ShadowDedupe { file_path: None })],
        1,
    );
    assert_eq!(payload, original);
}
#[test]
fn pins_with_quotes_and_backslashes_are_idempotent() {
    let mut payload = json!({"messages":[{"role":"user","content":"original"}]});
    let rules = [rule(MutationOp::PinContext {
        label: "path".into(),
        content: "Use \"C:\\files\"".into(),
        source_node_id: None,
    })];
    apply_mutations_to_payload(&mut payload, ProviderKind::OpenAI, &rules, 1);
    let first = payload.clone();
    apply_mutations_to_payload(&mut payload, ProviderKind::OpenAI, &rules, 1);
    assert_eq!(payload, first);
}

#[test]
fn native_tool_text_arrays_can_be_targeted_without_dropping_metadata() {
    let mut payload = json!({"system":"native session","messages":[
        {"role":"user","content":[{"type":"text","text":"keep the requirement"}]},
        {"role":"assistant","content":[{"type":"toolCall","id":"call","name":"bash","arguments":{"command":"echo log"}}]},
        {"role":"toolResult","toolCallId":"call","toolName":"bash","content":[{"type":"text","text":"old diagnostic output","extra":"preserved","cache_control":{"type":"ephemeral"}}]},
        {"role":"assistant","content":[{"type":"text","text":"read it"}]},
        {"role":"user","content":"next task"}]});
    let nodes = parse_request_to_nodes(&payload, ProviderKind::Anthropic, 1).unwrap();
    let node = nodes
        .iter()
        .find(|n| matches!(n.kind, NodeKind::ToolResult { .. }))
        .unwrap();
    let r = rule(MutationOp::PruneOrStub {
        target_fingerprint: Some(node.fingerprint.0.clone()),
        target_call_id: Some("call".into()),
        target_json_pointer: None,
        stub_text: "summary".into(),
    });
    let report = apply_mutations_to_payload(&mut payload, ProviderKind::Anthropic, &[r], 1);
    assert_eq!(report.applied_rule_ids, vec!["safety"]);
    assert_eq!(
        payload["messages"][2]["content"][0],
        json!({"type":"text","text":"summary","extra":"preserved","cache_control":{"type":"ephemeral"}})
    );
    assert_eq!(
        payload["messages"][0]["content"][0]["text"],
        "keep the requirement"
    );
}
