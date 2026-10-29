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
    // CW-100 / Finding 2: Minimum plan ("cache_safe_boost") strictly preserves
    // the historical tokenized prefix. Unverified or historical mutations never qualify.
    let plans = cw_core::optimizer::build_optimization_plans(&turn);
    let min = plans.iter().find(|p| p.id == "cache_safe_boost").unwrap();
    assert!(
        min.items.is_empty(),
        "Historical prefix mutation must not qualify for cache_safe_boost"
    );
    assert_eq!(min.kv_cache_break_tokens, 0);

    let previous = turn.nodes.clone();
    let report = cw_core::radix::analyze_kv_cache(
        Some(&previous),
        &mut turn.nodes,
        ProviderKind::OpenAI,
        None,
    );
    assert_eq!(report.hit_tokens, 0);
    assert!(
        !report.telemetry_available,
        "Telemetry must be marked unavailable when actual_cached_tokens is None"
    );
    // Prefix-matching nodes without provider telemetry must be marked Unknown
    assert_eq!(turn.nodes[0].kv_status, KvNodeStatus::Unknown);

    // Verify optimizer evaluation with explicit KvNodeStatus::Unknown nodes
    let plans_after_unknown = cw_core::optimizer::build_optimization_plans(&turn);
    let min_unknown = plans_after_unknown
        .iter()
        .find(|p| p.id == "cache_safe_boost")
        .unwrap();
    assert!(
        min_unknown.items.is_empty(),
        "Unknown cache prefix must never be included in cache_safe_boost"
    );
    assert_eq!(min_unknown.kv_cache_break_tokens, 0);

    // Other plans (e.g. clean_code_state) can still identify dedupe opportunities,
    // but their kv_break_tokens must be non-zero (not masquerading as free)!
    let clean = plans_after_unknown
        .iter()
        .find(|p| p.id == "clean_code_state")
        .unwrap();
    assert!(!clean.items.is_empty());
    assert!(
        clean.kv_cache_break_tokens > 0,
        "Unknown cache invalidation must compute non-zero break cost"
    );

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
fn consecutive_requests_telemetry_and_suffix_invalidation_matrix() {
    // Finding 1 & 2: Test 0, partial, and unknown telemetry across consecutive requests.
    let payload = reads((1, 10), (1, 100));
    let (turn_t1, _) = analyze_turn_from_payload(
        "t1".into(),
        "s".into(),
        None,
        1,
        &payload,
        ProviderKind::OpenAI,
        None,
        Some(0), // Turn 1: 0 cache hits observed
        None,
    )
    .unwrap();

    // 1. Telemetry present but 0 hits: historical mutations still cannot enter `min`
    let plans_t1 = cw_core::optimizer::build_optimization_plans(&turn_t1);
    let min_t1 = plans_t1
        .iter()
        .find(|p| p.id == "cache_safe_boost")
        .unwrap();
    assert!(
        min_t1.items.is_empty(),
        "min must preserve historical prefix even when turn 1 had 0 cache hits"
    );

    // 2. Turn 2 with partial cache hit (e.g. 4000 hit out of 10000 tokens)
    let total_tokens = 10_000u32;
    let report_partial = cw_core::ir::KvCacheReport {
        hit_tokens: 4_000,
        newly_cached_tokens: 6_000,
        invalidated_tokens: 0,
        uncached_tail_tokens: 0,
        hit_ratio: 0.4,
        divergence_token_index: None,
        invalidation_cause: None,
        telemetry_available: true,
    };

    // Candidate at token 8,000 (after the 4,000 hit, but within the 10,000 ingested tokens)
    // On Turn N+1, modifying token 8,000 invalidates tokens 8,000..10,000 that were newly cached on Turn N!
    let break_at_8k = cw_core::radix::calculate_kv_break_tokens(
        8_000,
        500,
        false, // in-place modification
        total_tokens,
        Some(&report_partial),
    );
    assert_eq!(
        break_at_8k, 2_000,
        "Modifying token 8,000 must account for invalidating suffix 8,000..10,000 for Turn N+1"
    );

    // In-place modification at final node (9,500..10,000) must NOT receive zero break cost!
    let break_at_9500 = cw_core::radix::calculate_kv_break_tokens(
        9_500,
        500,
        false, // in-place replacement / prune on final node
        total_tokens,
        Some(&report_partial),
    );
    assert_eq!(
        break_at_9500, 500,
        "In-place modification of final node must invalidate 9,500..10,000 on Turn N+1 (cannot receive 0 break)"
    );

    // 3. Centralized agreement: verify estimate_mutation_impact matches calculate_kv_break_tokens
    let dummy_node_prefix = cw_core::ir::ContextNode {
        id: "node_0".into(),
        turn_seq: 1,
        fingerprint: cw_core::ir::NodeFingerprint("fp0".into()),
        raw_json_pointer: "/messages/0".into(),
        role: cw_core::ir::Role::System,
        kind: cw_core::ir::NodeKind::SystemPrompt { text: "sys".into() },
        token_span: cw_core::ir::TokenSpan {
            start: 0,
            end: 8_000,
            count: 8_000,
        },
        kv_status: cw_core::ir::KvNodeStatus::CachedHit,
        attention_salience: 0.9,
        active_mutation: None,
        cache_control: None,
        timestamp: None,
    };
    let dummy_node = cw_core::ir::ContextNode {
        id: "node_8k".into(),
        turn_seq: 1,
        fingerprint: cw_core::ir::NodeFingerprint("fp".into()),
        raw_json_pointer: "/messages/1".into(),
        role: cw_core::ir::Role::Tool,
        kind: cw_core::ir::NodeKind::ToolResult {
            call_id: "call_1".into(),
            tool_name: "test".into(),
            content: "output".into(),
            is_error: false,
            semantic_meta: None,
        },
        token_span: cw_core::ir::TokenSpan {
            start: 8_000,
            end: 8_500,
            count: 500,
        },
        kv_status: cw_core::ir::KvNodeStatus::NewlyCached,
        attention_salience: 0.5,
        active_mutation: None,
        cache_control: None,
        timestamp: None,
    };
    let dummy_node_tail = cw_core::ir::ContextNode {
        id: "node_tail".into(),
        turn_seq: 2,
        fingerprint: cw_core::ir::NodeFingerprint("fp2".into()),
        raw_json_pointer: "/messages/2".into(),
        role: cw_core::ir::Role::User,
        kind: cw_core::ir::NodeKind::UserMessage {
            text: "next".into(),
        },
        token_span: cw_core::ir::TokenSpan {
            start: 8_500,
            end: 10_000,
            count: 1_500,
        },
        kv_status: cw_core::ir::KvNodeStatus::NewlyCached,
        attention_salience: 0.95,
        active_mutation: None,
        cache_control: None,
        timestamp: None,
    };
    let op = cw_core::ir::MutationOp::PruneOrStub {
        target_fingerprint: Some("fp".into()),
        target_call_id: Some("call_1".into()),
        target_json_pointer: None,
        stub_text: "[stub]".into(),
    };
    let (_saved, est_break) = cw_core::mutation::estimate_mutation_impact(
        &op,
        &[dummy_node_prefix, dummy_node, dummy_node_tail],
        Some(&report_partial),
    );
    assert_eq!(
        est_break, 2_000,
        "estimate_mutation_impact must match calculate_kv_break_tokens"
    );

    // 4. Pure append-only operation (PinContext) preserves 0 break across all states
    let pin_break_partial = cw_core::radix::calculate_kv_break_tokens(
        10_000,
        50,
        true, // is_append_only: true
        total_tokens,
        Some(&report_partial),
    );
    assert_eq!(pin_break_partial, 0, "Tail pinning must have 0 break cost");

    let pin_break_unknown = cw_core::radix::calculate_kv_break_tokens(
        10_000,
        50,
        true, // is_append_only: true
        total_tokens,
        None, // Unknown telemetry
    );
    assert_eq!(
        pin_break_unknown, 0,
        "Tail pinning must remain 0 break even when telemetry is unknown"
    );
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
