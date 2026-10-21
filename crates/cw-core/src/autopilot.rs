use crate::ir::{
    ContextNode, MutationOp, MutationRule, MutationStatus, NodeKind, ProviderKind,
};
use crate::local_llm::detect_local_engine;
use crate::mutation::apply_mutations_to_payload;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoPilotConfig {
    pub enabled: bool,
    /// Rule 1: Automatically truncate oversized tool outputs on the latest turn (>4,000 tok)
    /// to head 40 lines + tail 60 lines and spill full log to `/tmp/cw_logs/<call_id>.log` (0 KV break).
    pub zero_break_tail_hygiene: bool,
    pub tail_max_tool_tokens: u32,
    /// Rule 2: Automatically pin `<RULE[...]>` / `AGENTS.md` to the tail when context > 60k tokens
    /// and salience drops below 42% (0 KV break).
    pub auto_cache_safe_pin: bool,
    pub pin_min_context_tokens: u32,
    /// Rule 3: Automatically trigger Shadow-Dedupe + Old Tool Log Diet when context exceeds 140k tokens.
    pub auto_diet_at_breakpoint: bool,
    pub breakpoint_tokens: u32,
    /// Rule 4: Local Model Sliding-Window Guard — proactively compact context before exceeding 78% of local `num_ctx` (e.g. 25,000 / 32,768 tokens).
    pub local_model_window_guard: bool,
    pub local_window_threshold_tokens: u32,
}

impl Default for AutoPilotConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            zero_break_tail_hygiene: true,
            tail_max_tool_tokens: 4_000,
            auto_cache_safe_pin: true,
            pin_min_context_tokens: 60_000,
            auto_diet_at_breakpoint: true,
            breakpoint_tokens: 140_000,
            local_model_window_guard: true,
            local_window_threshold_tokens: 25_000,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AutoPilotReport {
    pub triggered_policies: Vec<String>,
    pub generated_mutations: Vec<MutationRule>,
    pub spilled_log_files: Vec<String>,
}

/// Evaluates hands-free Auto-Pilot policies on an incoming request payload or turn.
/// Mutates `payload` in-place and returns any newly generated persistent `MutationRule`s.
pub fn evaluate_autopilot_policies(
    config: &AutoPilotConfig,
    session_id: &str,
    model: &str,
    provider: ProviderKind,
    turn_seq: u32,
    nodes: &[ContextNode],
    payload: &mut Value,
) -> AutoPilotReport {
    if !config.enabled {
        return AutoPilotReport::default();
    }

    let mut report = AutoPilotReport::default();
    let total_tokens: u32 = nodes.iter().map(|n| n.token_span.count).sum();
    let max_turn = nodes.iter().map(|n| n.turn_seq).max().unwrap_or(turn_seq);
    let (is_local, _) = detect_local_engine(model, provider);

    // Policy 1: Zero-Break Tail Hygiene (Spill oversized latest-turn tool outputs to `/tmp/cw_logs/`)
    if config.zero_break_tail_hygiene {
        for node in nodes {
            if node.turn_seq == max_turn
                && node.token_span.count > config.tail_max_tool_tokens
                && let NodeKind::ToolResult {
                    call_id,
                    tool_name,
                    content,
                    ..
                } = &node.kind
            {
                if content.contains("[cw:auto_pilot") || content.contains("[cw:pruned") {
                    continue;
                }
                let lines: Vec<&str> = content.lines().collect();
                if lines.len() > 110 {
                    let safe_call_id: String = call_id
                        .chars()
                        .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
                        .take(36)
                        .collect();
                    let log_dir = PathBuf::from("/tmp/cw_logs");
                    let _ = fs::create_dir_all(&log_dir);
                    let log_path = log_dir.join(format!(
                        "{}_{}.log",
                        session_id,
                        if safe_call_id.is_empty() {
                            &node.id
                        } else {
                            &safe_call_id
                        }
                    ));
                    let _ = fs::write(&log_path, content);
                    let log_path_str = log_path.to_string_lossy().to_string();

                    let head = lines[..40].join("\n");
                    let tail = lines[lines.len() - 60..].join("\n");
                    let omitted_lines = lines.len().saturating_sub(100);
                    let stub = format!(
                        "{head}\n\n... [cw:auto_pilot Zero-Break Tail Hygiene truncated {omitted_lines} middle lines ({} tok) from `{tool_name}`. Full raw output saved to `{log_path_str}` — use grep/read_file on that path if needed] ...\n\n{tail}",
                        node.token_span.count
                    );

                    let rule = MutationRule {
                        id: format!("ap_tail_{}", &Uuid::new_v4().to_string()[..8]),
                        session_id: session_id.to_string(),
                        op: MutationOp::PruneOrStub {
                            target_fingerprint: Some(node.fingerprint.0.clone()),
                            target_call_id: if call_id.is_empty() {
                                None
                            } else {
                                Some(call_id.clone())
                            },
                            target_json_pointer: Some(node.raw_json_pointer.clone()),
                            stub_text: stub,
                        },
                        status: MutationStatus::Committed,
                        estimated_saved_tokens: (node.token_span.count as i32).saturating_sub(450),
                        estimated_kv_break_tokens: 0,
                        created_at: Utc::now(),
                    };
                    report.spilled_log_files.push(log_path_str);
                    report.generated_mutations.push(rule);
                }
            }
        }
        if !report.spilled_log_files.is_empty() {
            report
                .triggered_policies
                .push("zero_break_tail_hygiene".to_string());
        }
    }

    // Policy 2: Auto Cache-Safe Pin (when total tokens > threshold and rule salience < 42%)
    if config.auto_cache_safe_pin && total_tokens >= config.pin_min_context_tokens {
        let mut pinned_any = false;
        for node in nodes {
            if matches!(node.kind, NodeKind::InjectedRule { .. }) && node.attention_salience < 0.42
            {
                let snippet: String = node
                    .kind
                    .text_for_tokenization()
                    .lines()
                    .take(8)
                    .collect::<Vec<_>>()
                    .join(" ");
                let snippet_trimmed: String = snippet.chars().take(240).collect();
                let rule = MutationRule {
                    id: format!("ap_pin_{}", &Uuid::new_v4().to_string()[..8]),
                    session_id: session_id.to_string(),
                    op: MutationOp::PinContext {
                        label: format!("Auto-Pinned {}", node.kind.short_label()),
                        content: snippet_trimmed,
                        source_node_id: Some(node.id.clone()),
                    },
                    status: MutationStatus::Committed,
                    estimated_saved_tokens: -35,
                    estimated_kv_break_tokens: 0,
                    created_at: Utc::now(),
                };
                report.generated_mutations.push(rule);
                pinned_any = true;
            }
        }
        if pinned_any {
            report
                .triggered_policies
                .push("auto_cache_safe_pin".to_string());
        }
    }

    // Policy 3 & 4: Auto Diet at Breakpoint OR Local Model Sliding-Window Guard
    let trigger_cloud_breakpoint =
        config.auto_diet_at_breakpoint && total_tokens >= config.breakpoint_tokens;
    let trigger_local_guard = config.local_model_window_guard
        && is_local
        && total_tokens >= config.local_window_threshold_tokens;

    if trigger_cloud_breakpoint || trigger_local_guard {
        report.generated_mutations.push(MutationRule {
            id: format!("ap_dedupe_{}", &Uuid::new_v4().to_string()[..8]),
            session_id: session_id.to_string(),
            op: MutationOp::ShadowDedupe { file_path: None },
            status: MutationStatus::Committed,
            estimated_saved_tokens: 0,
            estimated_kv_break_tokens: 0,
            created_at: Utc::now(),
        });
        if trigger_local_guard {
            report
                .triggered_policies
                .push("local_model_window_guard".to_string());
        } else {
            report
                .triggered_policies
                .push("auto_diet_at_breakpoint".to_string());
        }
    }

    if !report.generated_mutations.is_empty() {
        apply_mutations_to_payload(payload, provider, &report.generated_mutations, turn_seq);
    }

    report
}
