pub mod autopilot;
pub mod control;
pub mod hud;
pub mod ir;
pub mod linter;
pub mod local_llm;
pub mod mutation;
pub mod normalizer;
pub mod optimizer;
pub mod radix;
pub mod repository;
pub mod slot_bridge;
pub mod swarm;
pub mod tokenizer;
pub mod validator;

use chrono::Utc;
use ir::{ContextNode, ContextTurn, ProviderKind, SessionId, TokenUsageSummary, TurnId};
use linter::analyze_context_health;
use normalizer::{NormalizeError, parse_request_to_nodes};
use radix::analyze_kv_cache;
use serde_json::Value;
use std::path::PathBuf;

/// Returns the current user's home directory cross-platform (Linux, macOS, Windows).
/// On Windows, falls back to `USERPROFILE` if `HOME` is unset, and falls back to temp_dir if neither is present.
pub fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .ok()
        .filter(|h| !h.trim().is_empty())
        .or_else(|| {
            std::env::var("USERPROFILE")
                .ok()
                .filter(|p| !p.trim().is_empty())
        })
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

/// End-to-end turn pipeline: parses a raw request payload into `ContextNode`s,
/// runs KV Cache Prefix Diffing against `prev_turn`, and executes the 7-rule Context Health Linter.
#[allow(clippy::too_many_arguments)]
pub fn analyze_turn_from_payload(
    turn_id: TurnId,
    session_id: SessionId,
    parent_turn_id: Option<TurnId>,
    sequence_number: u32,
    payload: &Value,
    provider: ProviderKind,
    prev_nodes: Option<&[ContextNode]>,
    actual_cached_tokens: Option<u32>,
    workspace_root: Option<&str>,
) -> Result<(ContextTurn, Vec<ir::FileContextState>), NormalizeError> {
    let mut nodes = parse_request_to_nodes(payload, provider, sequence_number)?;
    let kv_report = analyze_kv_cache(prev_nodes, &mut nodes, provider, actual_cached_tokens);
    let lint_out = analyze_context_health(&nodes, &kv_report, workspace_root);
    let input_tokens: u32 = nodes.iter().map(|n| n.token_span.count).sum();

    let turn = ContextTurn {
        analysis_complete: normalizer::supports_mutation_payload(payload, provider),
        normalization_provider: Some(provider),
        observed_usage: None,
        outcome: None,
        first_body_byte_ms: None,
        applied_mutation_ids: Vec::new(),
        id: turn_id,
        session_id,
        parent_turn_id,
        sequence_number,
        nodes,
        response_nodes: Vec::new(),
        token_usage: TokenUsageSummary {
            input_tokens,
            output_tokens: 0,
            cached_tokens: kv_report.hit_tokens,
            cache_write_tokens: kv_report.newly_cached_tokens,
            context_limit: 200_000,
            saved_by_mutations: 0,
        },
        kv_cache_report: kv_report,
        category_breakdown: lint_out.category_breakdown,
        lint_findings: lint_out.findings,
        health_score: lint_out.health_score,
        latency_ms: None,
        created_at: Utc::now(),
    };

    Ok((turn, lint_out.file_states))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{
        CacheInvalidationCause, KvNodeStatus, LintRuleId, MutationOp, MutationRule, MutationStatus,
    };
    use crate::mutation::{apply_mutations_to_payload, estimate_mutation_impact};
    use serde_json::json;

    #[test]
    fn test_anthropic_normalization_and_system_deconstruction() {
        let payload = json!({
            "model": "claude-sonnet-4-20250514",
            "max_tokens": 4096,
            "system": "You are an agent.\n<RULE[AGENTS.md]>\nAlways write unit tests.\n</RULE[AGENTS.md]>\n<skills>\n- ponytail (/skills/ponytail/SKILL.md): Write minimal code.\n</skills>",
            "tools": [
                {
                    "name": "mcp_chrome_click",
                    "description": "Click element",
                    "input_schema": { "type": "object" }
                },
                {
                    "name": "read_file",
                    "description": "Read a file",
                    "input_schema": { "type": "object" }
                }
            ],
            "messages": [
                { "role": "user", "content": "Check src/lib.rs" },
                {
                    "role": "assistant",
                    "content": [
                        { "type": "thinking", "thinking": "Let me read src/lib.rs", "signature": "sig_123" },
                        { "type": "tool_use", "id": "toolu_1", "name": "read_file", "input": { "Path": "src/lib.rs" } }
                    ]
                },
                {
                    "role": "user",
                    "content": [
                        { "type": "tool_result", "tool_use_id": "toolu_1", "content": "pub fn add(a: i32, b: i32) -> i32 { a + b }" }
                    ]
                }
            ]
        });

        let nodes = parse_request_to_nodes(&payload, ProviderKind::Anthropic, 1).unwrap();
        assert!(nodes.len() >= 8);
        // Verify Anthropic prefix serialization puts tools before system
        let tool_node = nodes
            .iter()
            .find(|n| matches!(n.kind, ir::NodeKind::McpToolSchema { .. }))
            .unwrap();
        let sys_node = nodes
            .iter()
            .find(|n| matches!(n.kind, ir::NodeKind::SystemPrompt { .. }))
            .unwrap();
        assert!(tool_node.token_span.start < sys_node.token_span.start);
    }

    #[test]
    fn test_kv_cache_prefix_diff_and_invalidation_cause() {
        let turn1 = json!({
            "model": "claude-sonnet-4",
            "max_tokens": 1024,
            "system": "Static system prompt",
            "tools": [{ "name": "read_file", "description": "Read file", "input_schema": {} }],
            "messages": [
                { "role": "user", "content": "Hello world" }
            ]
        });
        let nodes1 = parse_request_to_nodes(&turn1, ProviderKind::Anthropic, 1).unwrap();

        // Turn 2: normal append -> 100% of turn1 prefix is CachedHit!
        let turn2_good = json!({
            "model": "claude-sonnet-4",
            "max_tokens": 1024,
            "system": "Static system prompt",
            "tools": [{ "name": "read_file", "description": "Read file", "input_schema": {} }],
            "messages": [
                { "role": "user", "content": "Hello world" },
                { "role": "assistant", "content": "Hi there!" },
                { "role": "user", "content": "Next turn" }
            ]
        });
        let mut nodes2 = parse_request_to_nodes(&turn2_good, ProviderKind::Anthropic, 2).unwrap();
        let rep2 = analyze_kv_cache(Some(&nodes1), &mut nodes2, ProviderKind::Anthropic, None);
        assert_eq!(rep2.invalidated_tokens, 0);
        assert_eq!(rep2.hit_tokens, 0);
        assert_ne!(nodes2[0].kv_status, KvNodeStatus::CachedHit);

        // Turn 2 (bad): system prompt injected a dynamic timestamp -> invalidates messages!
        let turn2_bad = json!({
            "model": "claude-sonnet-4",
            "max_tokens": 1024,
            "system": "Mutated system prompt with timestamp 12:01",
            "tools": [{ "name": "read_file", "description": "Read file", "input_schema": {} }],
            "messages": [
                { "role": "user", "content": "Hello world" },
                { "role": "assistant", "content": "Hi there!" }
            ]
        });
        let mut nodes2_bad =
            parse_request_to_nodes(&turn2_bad, ProviderKind::Anthropic, 2).unwrap();
        let rep2_bad = analyze_kv_cache(
            Some(&nodes1),
            &mut nodes2_bad,
            ProviderKind::Anthropic,
            None,
        );
        assert!(rep2_bad.invalidated_tokens > 0);
        assert!(matches!(
            rep2_bad.invalidation_cause,
            Some(CacheInvalidationCause::DynamicSystemInjection { .. })
        ));
    }

    #[test]
    fn test_stale_and_duplicate_file_linter_and_shadow_dedupe_mutation() {
        let mut payload = json!({
            "model": "gpt-4o",
            "messages": [
                { "role": "system", "content": "System" },
                {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [{
                        "id": "call_1",
                        "function": { "name": "read_file", "arguments": "{\"path\":\"src/main.rs\",\"start_line\":1,\"end_line\":1}" }
                    }]
                },
                {
                    "role": "tool",
                    "tool_call_id": "call_1",
                    "content": "fn main() { println!(\"v2\"); }"
                },
                {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [{
                        "id": "call_2",
                        "function": { "name": "edit_file", "arguments": "{\"path\":\"src/main.rs\",\"content\":\"fn main() { println!(\\\"v2\\\"); }\"}" }
                    }]
                },
                {
                    "role": "tool",
                    "tool_call_id": "call_2",
                    "content": "Edited src/main.rs"
                },
                {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [{
                        "id": "call_3",
                        "function": { "name": "read_file", "arguments": "{\"path\":\"src/main.rs\",\"start_line\":1,\"end_line\":1}" }
                    }]
                },
                {
                    "role": "tool",
                    "tool_call_id": "call_3",
                    "content": "fn main() { println!(\"v2\"); }"
                }
            ]
        });

        let (turn, file_states) = analyze_turn_from_payload(
            "t1".into(),
            "s1".into(),
            None,
            4,
            &payload,
            ProviderKind::OpenAI,
            None,
            None,
            None,
        )
        .unwrap();

        // Must detect CW-L001 (Stale File) and CW-L002 (Duplicate File Read)
        assert!(
            turn.lint_findings
                .iter()
                .any(|f| f.rule_id == LintRuleId::StaleFileInContext)
        );
        assert!(
            turn.lint_findings
                .iter()
                .any(|f| f.rule_id == LintRuleId::DuplicateFileRead)
        );
        assert_eq!(file_states.len(), 1);
        assert_eq!(file_states[0].path, "src/main.rs");
        assert_eq!(file_states[0].read_count, 2);

        // Apply ShadowDedupe mutation and verify call_1 is replaced while call_3 stays intact!
        let rule = MutationRule {
            id: "mut_1".into(),
            session_id: "s1".into(),
            op: MutationOp::ShadowDedupe { file_path: None },
            status: MutationStatus::Committed,
            estimated_saved_tokens: 10,
            estimated_kv_break_tokens: 0,
            created_at: Utc::now(),
        };
        let (_, _kv_break) =
            estimate_mutation_impact(&rule.op, &turn.nodes, Some(&turn.kv_cache_report));
        let report = apply_mutations_to_payload(&mut payload, ProviderKind::OpenAI, &[rule], 4);
        assert_eq!(report.applied_rule_ids, vec!["mut_1".to_string()]);

        let msg2_content = payload["messages"][2]["content"].as_str().unwrap();
        let msg6_content = payload["messages"][6]["content"].as_str().unwrap();
        assert!(msg2_content.contains("[cw:shadow_deduped path=\"src/main.rs\""));
        assert_eq!(msg6_content, "fn main() { println!(\"v2\"); }");
    }

    #[test]
    fn test_pin_context_and_tool_gating() {
        let mut payload = json!({
            "model": "claude-sonnet-4",
            "max_tokens": 1024,
            "tools": [
                { "name": "mcp_chrome_click", "description": "Click", "input_schema": {} },
                { "name": "read_file", "description": "Read", "input_schema": {} }
            ],
            "messages": [
                { "role": "user", "content": "Fix the bug" }
            ]
        });

        let rules = vec![
            MutationRule {
                id: "m_gate".into(),
                session_id: "s1".into(),
                op: MutationOp::GateToolsAndSkills {
                    disabled_tools: vec![],
                    disabled_mcp_servers: vec!["chrome".into()],
                    disabled_skills: vec![],
                },
                status: MutationStatus::Committed,
                estimated_saved_tokens: 50,
                estimated_kv_break_tokens: 0,
                created_at: Utc::now(),
            },
            MutationRule {
                id: "m_pin".into(),
                session_id: "s1".into(),
                op: MutationOp::PinContext {
                    label: "Arch Invariant".into(),
                    content: "Never use .unwrap() in cw-core".into(),
                    source_node_id: None,
                },
                status: MutationStatus::Committed,
                estimated_saved_tokens: 0,
                estimated_kv_break_tokens: 0,
                created_at: Utc::now(),
            },
        ];

        apply_mutations_to_payload(&mut payload, ProviderKind::Anthropic, &rules, 1);
        let tools = payload["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "read_file");

        let last_msg = payload["messages"][0]["content"].as_str().unwrap();
        assert!(last_msg.contains("<cw_pinned_context>"));
        assert!(last_msg.contains("Never use .unwrap() in cw-core"));
    }

    #[test]
    fn test_local_llm_kv_optimization_and_cw_field_stripping() {
        use crate::local_llm::{
            analyze_local_model_kv, calculate_cost_and_perf, inject_local_engine_kv_optimizations,
            strip_internal_cw_fields,
        };

        let mut payload = json!({
            "model": "qwen2.5-coder:32b",
            "system": "You are a local coding agent.\n<RULE[AGENTS.md]>\nKeep KV cache pinned.\n</RULE[AGENTS.md]>",
            "messages": [
                {
                    "role": "user",
                    "content": "Optimize my local Ollama KV cache",
                    "cw_timestamp": "2026-10-21T22:00:00Z",
                    "cw_turn_seq": 1
                }
            ]
        });

        let (turn, _) = analyze_turn_from_payload(
            "t_local".into(),
            "sess_local".into(),
            None,
            1,
            &payload,
            ProviderKind::OpenAICompatible,
            None,
            None,
            None,
        )
        .unwrap();

        let local_analysis = analyze_local_model_kv(
            &turn,
            "sess_local",
            "qwen2.5-coder:32b",
            ProviderKind::OpenAICompatible,
        );
        assert!(local_analysis.is_local_model);
        assert_eq!(local_analysis.engine_kind, "ollama");
        assert!(local_analysis.recommended_n_keep_tokens >= 256);

        let cost =
            calculate_cost_and_perf(&turn, "qwen2.5-coder:32b", ProviderKind::OpenAICompatible);
        assert!(cost.is_local_model);

        let injected =
            inject_local_engine_kv_optimizations(&mut payload, "sess_local", &turn.nodes, "ollama");
        assert!(!injected.is_empty());
        assert_eq!(payload["keep_alive"], "30m");
        assert!(payload["options"]["num_keep"].as_u64().unwrap() >= 256);

        strip_internal_cw_fields(&mut payload);
        assert!(payload["messages"][0].get("cw_timestamp").is_none());
        assert!(payload["messages"][0].get("cw_turn_seq").is_none());
    }

    #[test]
    fn test_gemini_tool_gating_and_sorting() {
        use crate::ir::{MutationOp, MutationRule, MutationStatus, ProviderKind};
        use crate::mutation::apply_mutations_to_payload;
        use serde_json::json;

        let mut payload = json!({
            "contents": [
                { "role": "user", "parts": [{ "text": "Hello" }] }
            ],
            "tools": [
                {
                    "function_declarations": [
                        { "name": "z_tool", "description": "tool z" },
                        { "name": "a_tool", "description": "tool a" },
                        { "name": "mcp_github_create_issue", "description": "gh" }
                    ]
                }
            ]
        });

        // Test sorting
        let sort_rule = MutationRule {
            id: "sort_1".into(),
            session_id: "s_gem".into(),
            op: MutationOp::NormalizePrefix {
                sort_tools_deterministically: true,
                relocate_dynamic_system_metadata: false,
            },
            status: MutationStatus::Committed,
            estimated_saved_tokens: 0,
            estimated_kv_break_tokens: 0,
            created_at: chrono::Utc::now(),
        };
        let rep1 = apply_mutations_to_payload(&mut payload, ProviderKind::Gemini, &[sort_rule], 1);
        assert!(!rep1.applied_rule_ids.is_empty());
        let decls = payload["tools"][0]["function_declarations"]
            .as_array()
            .unwrap();
        assert_eq!(decls[0]["name"], "a_tool");
        assert_eq!(decls[1]["name"], "mcp_github_create_issue");
        assert_eq!(decls[2]["name"], "z_tool");

        // Test gating
        let gate_rule = MutationRule {
            id: "gate_1".into(),
            session_id: "s_gem".into(),
            op: MutationOp::GateToolsAndSkills {
                disabled_tools: vec!["a_tool".into()],
                disabled_mcp_servers: vec!["github".into()],
                disabled_skills: vec![],
            },
            status: MutationStatus::Committed,
            estimated_saved_tokens: 10,
            estimated_kv_break_tokens: 0,
            created_at: chrono::Utc::now(),
        };
        let rep2 = apply_mutations_to_payload(&mut payload, ProviderKind::Gemini, &[gate_rule], 1);
        assert!(!rep2.applied_rule_ids.is_empty());
        let decls_after = payload["tools"][0]["function_declarations"]
            .as_array()
            .unwrap();
        assert_eq!(decls_after.len(), 1);
        assert_eq!(decls_after[0]["name"], "z_tool");
    }

    #[test]
    fn test_ambient_savings_pulse_generation() {
        use crate::autopilot::{generate_ambient_savings_pulse, should_emit_savings_pulse};

        assert!(should_emit_savings_pulse(5000, 0, 1000));
        assert!(should_emit_savings_pulse(6500, 5000, 1000));
        assert!(!should_emit_savings_pulse(5500, 5000, 1000));

        let payload = json!({
            "model": "claude-sonnet-4",
            "messages": [
                { "role": "user", "content": "Hello world" }
            ]
        });
        let (mut turn, _) = analyze_turn_from_payload(
            "t1".to_string(),
            "s1".to_string(),
            None,
            1,
            &payload,
            ProviderKind::Anthropic,
            None,
            None,
            None,
        )
        .unwrap();

        turn.token_usage.input_tokens = 14488;
        turn.token_usage.saved_by_mutations = 12966;
        turn.kv_cache_report.hit_ratio = 0.982;
        turn.kv_cache_report.hit_tokens = 14227;

        let pulse = generate_ambient_savings_pulse(
            "s1",
            &turn,
            "claude-sonnet-4",
            ProviderKind::Anthropic,
            12966,
        );

        assert_eq!(pulse.tokens_saved_this_pulse, 12966);
        assert_eq!(pulse.session_id, "s1");
        assert!(
            pulse
                .formatted_message
                .contains("⚡ [cw] Estimated context reduction: 12966 tokens")
        );
        assert!(pulse.formatted_message.contains("KV Cache 98.2% preserved"));
    }

    #[test]
    fn test_opencode_pin_growth_and_unread_tool_output_regression() {
        use crate::autopilot::{AutoPilotConfig, evaluate_autopilot_policies};

        // Simulate an OpenCode session on Turn 15 where Turn 1 was "do npm test, see if it runs",
        // tools schemas are present (which previously inflated `max_turn` to 15),
        // and the latest message is a 110-char `cw_context_status` result from `execute`.
        let mut payload = json!({
            "model": "litellm-estate/swift-1.5-iq3_xxs",
            "tools": [
                {
                    "type": "function",
                    "function": {
                        "name": "execute",
                        "description": "Execute MCP or shell tool",
                        "parameters": { "type": "object" }
                    }
                }
            ],
            "messages": [
                { "role": "system", "content": "You are an OpenCode agent." },
                { "role": "user", "content": "do npm test, see if it runs" },
                {
                    "role": "assistant",
                    "content": "Running npm test.",
                    "tool_calls": [
                        {
                            "id": "call_status_1",
                            "type": "function",
                            "function": { "name": "execute", "arguments": "{\"tool\":\"cw_context_status\"}" }
                        }
                    ]
                },
                {
                    "role": "tool",
                    "tool_call_id": "call_status_1",
                    "name": "execute",
                    "content": "{\"session_id\":\"s_oc\",\"health_score\":88,\"kv_cache_report\":{\"hit_ratio\":95.0}}"
                }
            ]
        });

        let (turn, _) = analyze_turn_from_payload(
            "turn_15".to_string(),
            "s_oc".to_string(),
            None,
            15,
            &payload,
            ProviderKind::OpenAI,
            None,
            None,
            None,
        )
        .unwrap();

        // 1. Verify no optimization plan auto-pins the initial UserMessage or prunes the unread tail ToolResult
        let plans = crate::optimizer::build_optimization_plans(&turn);
        for plan in &plans {
            for item in &plan.items {
                assert!(
                    !matches!(item.mutation_op, MutationOp::PinContext { .. }),
                    "UserMessage must never be auto-pinned in plan {}",
                    plan.id
                );
                assert!(
                    !matches!(item.mutation_op, MutationOp::PruneOrStub { .. }),
                    "Unread tail cw_context_status output must never be pruned in plan {}",
                    plan.id
                );
            }
        }

        // 2. Verify AutoPilot default config does not inject any PinContext or prune the tail tool output
        let ap_cfg = AutoPilotConfig::default();
        let ap_report = evaluate_autopilot_policies(
            &ap_cfg,
            "s_oc",
            "litellm-estate/swift-1.5-iq3_xxs",
            ProviderKind::OpenAI,
            15,
            &turn.nodes,
            Some(&turn),
            &mut payload,
        );
        assert!(ap_report.generated_mutations.is_empty());
        let serialized = serde_json::to_string(&payload).unwrap();
        assert!(!serialized.contains("<cw_pinned_context>"));
        assert!(!serialized.contains("[cw:pruned"));

        // 3. Even if duplicate PinContext rules or pre-existing <cw_pinned_context> blocks exist,
        // apply_mutations_to_payload must deduplicate them and never nest <cw_pinned_context>.
        payload["messages"][1]["content"] = json!(
            "do npm test, see if it runs\n<cw_pinned_context>\n- [User: do npm test]: do npm test\n</cw_pinned_context>"
        );
        let dup_pins = vec![
            MutationRule {
                id: "pin_1".into(),
                session_id: "s_oc".into(),
                op: MutationOp::PinContext {
                    label: "Rule".into(),
                    content: "Never break KV cache\n<cw_pinned_context>\n- [Nested]: bad\n</cw_pinned_context>".into(),
                    source_node_id: None,
                },
                status: MutationStatus::Committed,
                estimated_saved_tokens: 0,
                estimated_kv_break_tokens: 0,
                created_at: chrono::Utc::now(),
            },
            MutationRule {
                id: "pin_2".into(),
                session_id: "s_oc".into(),
                op: MutationOp::PinContext {
                    label: "Rule".into(),
                    content: "Never break KV cache".into(),
                    source_node_id: None,
                },
                status: MutationStatus::Committed,
                estimated_saved_tokens: 0,
                estimated_kv_break_tokens: 0,
                created_at: chrono::Utc::now(),
            },
        ];
        assert!(dup_pins[0].op.is_semantically_equivalent(&dup_pins[1].op));
        apply_mutations_to_payload(&mut payload, ProviderKind::OpenAI, &dup_pins, 15);
        let after_str = serde_json::to_string(&payload).unwrap();
        assert_eq!(
            after_str.matches("<cw_pinned_context>").count(),
            2,
            "Preserve user marker text and add one deduplicated reminder"
        );
        assert_eq!(
            after_str.matches("Never break KV cache").count(),
            1,
            "Duplicate PinContext rules must be deduplicated"
        );
    }
}
