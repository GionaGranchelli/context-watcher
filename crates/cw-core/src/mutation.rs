use crate::ir::{
    ContextNode, FirewallAction, FirewallRule, FirewallTrigger, KvCacheReport, MutationOp,
    MutationRule, MutationStatus, NodeKind, ProviderKind, ToolSemanticMeta,
};
use crate::normalizer::parse_request_to_nodes;
use crate::tokenizer::estimate_tokens;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::LazyLock;

static DYNAMIC_TIME_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)<ADDITIONAL_METADATA>\s*The current local time is:[^<]*</ADDITIONAL_METADATA>")
        .unwrap()
});

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MutationApplicationReport {
    pub applied_rule_ids: Vec<String>,
    pub saved_tokens: u32,
}

/// Applies all `Committed` mutations from `rules` deterministically onto a raw provider JSON payload.
/// Preserves protocol invariants (never breaks `tool_use` / `tool_result` pairing or signatures).
pub fn apply_mutations_to_payload(
    payload: &mut Value,
    provider: ProviderKind,
    rules: &[MutationRule],
    turn_seq: u32,
) -> MutationApplicationReport {
    let active_rules: Vec<&MutationRule> = rules
        .iter()
        .filter(|r| r.status == MutationStatus::Committed)
        .collect();
    if active_rules.is_empty() {
        return MutationApplicationReport::default();
    }

    let before_nodes = parse_request_to_nodes(payload, provider, turn_seq).unwrap_or_default();
    let before_tokens: u32 = before_nodes.iter().map(|n| n.token_span.count).sum();

    let mut applied_ids = Vec::new();
    let mut pinned_blocks: Vec<(String, String)> = Vec::new();

    for rule in &active_rules {
        let mut applied = false;
        match &rule.op {
            MutationOp::PruneOrStub {
                target_fingerprint,
                target_call_id,
                target_json_pointer,
                stub_text,
            } => {
                // Match node by fingerprint, call_id, or direct pointer
                for node in &before_nodes {
                    let fp_match = target_fingerprint
                        .as_ref()
                        .map(|fp| fp == &node.fingerprint.0)
                        .unwrap_or(false);
                    let call_match = match (&node.kind, target_call_id.as_ref()) {
                        (NodeKind::ToolResult { call_id, .. }, Some(tc)) if !call_id.is_empty() => {
                            call_id == tc
                        }
                        _ => false,
                    };
                    let ptr_match = target_json_pointer
                        .as_ref()
                        .map(|p| p == &node.raw_json_pointer)
                        .unwrap_or(false);

                    if (fp_match || call_match || ptr_match)
                        && replace_node_text_at_pointer(payload, &node.raw_json_pointer, stub_text)
                    {
                        applied = true;
                    }
                }
            }
            MutationOp::FoldSubtree {
                start_turn,
                end_turn,
                summary_text,
            } => {
                let mut first_folded = true;
                for node in &before_nodes {
                    let is_foldable_kind =
                        matches!(
                            node.kind,
                            NodeKind::ToolResult { .. } | NodeKind::AssistantThinking { .. }
                        ) || (matches!(node.kind, NodeKind::AssistantText { .. })
                            && node.turn_seq < *end_turn);
                    if node.turn_seq >= *start_turn
                        && node.turn_seq <= *end_turn
                        && is_foldable_kind
                    {
                        let replacement = if first_folded {
                            first_folded = false;
                            format!(
                                "[cw:folded_subtree turns={}..{}] {}",
                                start_turn, end_turn, summary_text
                            )
                        } else {
                            "[cw:folded]".to_string()
                        };
                        if replace_node_text_at_pointer(
                            payload,
                            &node.raw_json_pointer,
                            &replacement,
                        ) {
                            applied = true;
                        }
                    }
                }
            }
            MutationOp::ShadowDedupe { file_path } => {
                // Find all FileRead tool results and deduplicate earlier reads of the same file
                // or reads superseded by a later FileWrite
                let mut reads_by_path: HashMap<String, Vec<&ContextNode>> = HashMap::new();
                let mut latest_write_by_path: HashMap<String, u32> = HashMap::new();

                for node in &before_nodes {
                    match &node.kind {
                        NodeKind::ToolCall {
                            semantic_meta: Some(ToolSemanticMeta::FileWrite { path, .. }),
                            ..
                        }
                        | NodeKind::ToolResult {
                            semantic_meta: Some(ToolSemanticMeta::FileWrite { path, .. }),
                            ..
                        } => {
                            latest_write_by_path.insert(path.clone(), node.turn_seq);
                        }
                        NodeKind::ToolResult {
                            semantic_meta: Some(ToolSemanticMeta::FileRead { path, .. }),
                            ..
                        } if file_path.as_ref().map(|fp| fp == path).unwrap_or(true) => {
                            reads_by_path.entry(path.clone()).or_default().push(node);
                        }
                        _ => {}
                    }
                }

                for (path, reads) in reads_by_path {
                    let latest_write_turn = latest_write_by_path.get(&path).copied().unwrap_or(0);
                    let latest_read_turn = reads.last().map(|n| n.turn_seq).unwrap_or(0);
                    let supersede_turn = latest_write_turn.max(latest_read_turn);

                    for (idx, read_node) in reads.iter().enumerate() {
                        let is_older_dup = idx + 1 < reads.len();
                        let is_stale_from_write = read_node.turn_seq < latest_write_turn;
                        if is_older_dup || is_stale_from_write {
                            let stub = format!(
                                "[cw:shadow_deduped path=\"{path}\" superseded_by_turn={supersede_turn} saved_tokens={}]",
                                read_node.token_span.count
                            );
                            if replace_node_text_at_pointer(
                                payload,
                                &read_node.raw_json_pointer,
                                &stub,
                            ) {
                                applied = true;
                            }
                        }
                    }
                }
            }
            MutationOp::GateToolsAndSkills {
                disabled_tools,
                disabled_mcp_servers,
                disabled_skills: _,
            } => {
                if filter_tools_in_payload(payload, disabled_tools, disabled_mcp_servers) {
                    applied = true;
                }
            }
            MutationOp::PinContext { label, content, .. } => {
                pinned_blocks.push((label.clone(), content.clone()));
                applied = true;
            }
            MutationOp::NormalizePrefix {
                sort_tools_deterministically,
                relocate_dynamic_system_metadata,
            } => {
                if *sort_tools_deterministically && sort_tools_in_payload(payload) {
                    applied = true;
                }
                if *relocate_dynamic_system_metadata && strip_dynamic_time_in_system(payload) {
                    applied = true;
                }
            }
        }
        if applied {
            applied_ids.push(rule.id.clone());
        }
    }

    if !pinned_blocks.is_empty() {
        inject_pinned_context_at_tail(payload, &pinned_blocks);
    }

    let after_nodes = parse_request_to_nodes(payload, provider, turn_seq).unwrap_or_default();
    let after_tokens: u32 = after_nodes.iter().map(|n| n.token_span.count).sum();

    MutationApplicationReport {
        applied_rule_ids: applied_ids,
        saved_tokens: before_tokens.saturating_sub(after_tokens),
    }
}

/// Estimates how many tokens a `MutationOp` will save and how many cached KV prefix tokens
/// it would invalidate if committed immediately.
pub fn estimate_mutation_impact(op: &MutationOp, nodes: &[ContextNode]) -> (i32, u32) {
    let total_cached: u32 = nodes
        .iter()
        .filter(|n| n.kv_status == crate::ir::KvNodeStatus::CachedHit)
        .map(|n| n.token_span.count)
        .sum();

    match op {
        MutationOp::PruneOrStub {
            target_fingerprint,
            target_call_id,
            target_json_pointer,
            stub_text,
        } => {
            let stub_tok = estimate_tokens(stub_text) as i32;
            for node in nodes {
                let matches = target_fingerprint
                    .as_ref()
                    .map(|f| f == &node.fingerprint.0)
                    .unwrap_or(false)
                    || target_json_pointer
                        .as_ref()
                        .map(|p| p == &node.raw_json_pointer)
                        .unwrap_or(false)
                    || match (&node.kind, target_call_id.as_ref()) {
                        (NodeKind::ToolResult { call_id, .. }, Some(tc)) => call_id == tc,
                        _ => false,
                    };
                if matches {
                    let saved = (node.token_span.count as i32) - stub_tok;
                    let kv_break = total_cached.saturating_sub(node.token_span.start);
                    return (saved, kv_break);
                }
            }
            (0, 0)
        }
        MutationOp::FoldSubtree {
            start_turn,
            end_turn,
            summary_text,
        } => {
            let mut target_tokens = 0u32;
            let mut earliest_start = total_cached;
            for node in nodes {
                if node.turn_seq >= *start_turn
                    && node.turn_seq <= *end_turn
                    && matches!(
                        node.kind,
                        NodeKind::ToolResult { .. } | NodeKind::AssistantThinking { .. }
                    )
                {
                    target_tokens += node.token_span.count;
                    earliest_start = earliest_start.min(node.token_span.start);
                }
            }
            let summary_tok = estimate_tokens(summary_text) as i32;
            let saved = (target_tokens as i32) - summary_tok;
            let kv_break = total_cached.saturating_sub(earliest_start);
            (saved, kv_break)
        }
        MutationOp::ShadowDedupe { file_path } => {
            let mut reads_by_path: HashMap<String, Vec<&ContextNode>> = HashMap::new();
            let mut latest_write_by_path: HashMap<String, u32> = HashMap::new();
            for node in nodes {
                match &node.kind {
                    NodeKind::ToolCall {
                        semantic_meta: Some(ToolSemanticMeta::FileWrite { path, .. }),
                        ..
                    }
                    | NodeKind::ToolResult {
                        semantic_meta: Some(ToolSemanticMeta::FileWrite { path, .. }),
                        ..
                    } => {
                        latest_write_by_path.insert(path.clone(), node.turn_seq);
                    }
                    NodeKind::ToolResult {
                        semantic_meta: Some(ToolSemanticMeta::FileRead { path, .. }),
                        ..
                    } if file_path.as_ref().map(|fp| fp == path).unwrap_or(true) => {
                        reads_by_path.entry(path.clone()).or_default().push(node);
                    }
                    _ => {}
                }
            }
            let mut saved = 0i32;
            let mut earliest_start = total_cached;
            for (path, reads) in reads_by_path {
                let write_turn = latest_write_by_path.get(&path).copied().unwrap_or(0);
                for (i, r) in reads.iter().enumerate() {
                    if i + 1 < reads.len() || r.turn_seq < write_turn {
                        saved += (r.token_span.count as i32) - 24;
                        earliest_start = earliest_start.min(r.token_span.start);
                    }
                }
            }
            (saved, total_cached.saturating_sub(earliest_start))
        }
        MutationOp::GateToolsAndSkills {
            disabled_tools,
            disabled_mcp_servers,
            ..
        } => {
            let mut saved = 0i32;
            let mut earliest = total_cached;
            for node in nodes {
                if let NodeKind::McpToolSchema {
                    server_name,
                    tool_name,
                    ..
                } = &node.kind
                {
                    let srv_match = server_name
                        .as_ref()
                        .map(|s| disabled_mcp_servers.contains(s))
                        .unwrap_or(false);
                    if srv_match || disabled_tools.contains(tool_name) {
                        saved += node.token_span.count as i32;
                        earliest = earliest.min(node.token_span.start);
                    }
                }
            }
            (saved, total_cached.saturating_sub(earliest))
        }
        MutationOp::PinContext { content, .. } => {
            // PinContext is injected at the tail -> 0 KV cache break!
            (-(estimate_tokens(content) as i32), 0)
        }
        MutationOp::NormalizePrefix { .. } => (0, 0),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FirewallVerdict {
    pub paused_by_rule: Option<FirewallRule>,
    pub auto_applied_actions: Vec<String>,
}

/// Evaluates active `FirewallRule`s on an incoming request payload.
/// Executes automatic actions (`AutoTruncateHeadTail`, `AutoShadowDedupe`) in-place,
/// or returns `paused_by_rule: Some(rule)` if human breakpoint approval is required.
pub fn evaluate_firewall(
    payload: &mut Value,
    provider: ProviderKind,
    nodes: &[ContextNode],
    kv_report: &KvCacheReport,
    rules: &[FirewallRule],
    turn_seq: u32,
) -> FirewallVerdict {
    let mut verdict = FirewallVerdict {
        paused_by_rule: None,
        auto_applied_actions: Vec::new(),
    };

    for rule in rules.iter().filter(|r| r.enabled) {
        let mut matching_pointers = Vec::new();
        let triggered = match &rule.trigger {
            FirewallTrigger::SingleToolResultTokensExceed { max_tokens } => {
                for n in nodes {
                    if matches!(n.kind, NodeKind::ToolResult { .. })
                        && n.token_span.count > *max_tokens
                    {
                        matching_pointers
                            .push((n.raw_json_pointer.clone(), n.kind.text_for_tokenization()));
                    }
                }
                !matching_pointers.is_empty()
            }
            FirewallTrigger::TotalTurnDeltaTokensExceed { max_tokens } => {
                kv_report.newly_cached_tokens > *max_tokens
            }
            FirewallTrigger::UnexpectedKvCacheBreak { min_lost_tokens } => {
                kv_report.invalidated_tokens >= *min_lost_tokens
            }
            FirewallTrigger::ToolResultMatchesRegex { pattern } => {
                if let Ok(re) = Regex::new(pattern) {
                    for n in nodes {
                        if matches!(n.kind, NodeKind::ToolResult { .. }) {
                            let txt = n.kind.text_for_tokenization();
                            if re.is_match(&txt) {
                                matching_pointers.push((n.raw_json_pointer.clone(), txt));
                            }
                        }
                    }
                }
                !matching_pointers.is_empty()
            }
            FirewallTrigger::ManualStepMode => true,
        };

        if !triggered {
            continue;
        }

        match &rule.action {
            FirewallAction::PauseForHumanApproval { .. } => {
                verdict.paused_by_rule = Some(rule.clone());
                return verdict;
            }
            FirewallAction::AutoTruncateHeadTail {
                head_chars,
                tail_chars,
            } => {
                for (ptr, orig_text) in matching_pointers {
                    if orig_text.len() > head_chars + tail_chars {
                        let head: String = orig_text.chars().take(*head_chars).collect();
                        let tail_vec: Vec<char> = orig_text.chars().collect();
                        let tail: String = tail_vec[tail_vec.len().saturating_sub(*tail_chars)..]
                            .iter()
                            .collect();
                        let truncated = format!(
                            "{head}\n\n... [cw:firewall auto-truncated {} chars by rule '{}'] ...\n\n{tail}",
                            orig_text.len().saturating_sub(head_chars + tail_chars),
                            rule.name
                        );
                        replace_node_text_at_pointer(payload, &ptr, &truncated);
                    }
                }
                verdict.auto_applied_actions.push(rule.name.clone());
            }
            FirewallAction::AutoShadowDedupe => {
                let temp_rule = MutationRule {
                    id: "fw_auto_dedupe".to_string(),
                    session_id: String::new(),
                    op: MutationOp::ShadowDedupe { file_path: None },
                    status: MutationStatus::Committed,
                    estimated_saved_tokens: 0,
                    estimated_kv_break_tokens: 0,
                    created_at: chrono::Utc::now(),
                };
                apply_mutations_to_payload(payload, provider, &[temp_rule], turn_seq);
                verdict.auto_applied_actions.push(rule.name.clone());
            }
        }
    }

    verdict
}

fn replace_node_text_at_pointer(payload: &mut Value, pointer: &str, new_text: &str) -> bool {
    let base_ptr = pointer.split('#').next().unwrap_or(pointer);
    let Some(target) = payload.pointer_mut(base_ptr) else {
        return false;
    };

    match target {
        Value::String(s) => {
            *s = new_text.to_string();
            true
        }
        Value::Object(map) => {
            if map.contains_key("thinking") {
                map.insert("thinking".to_string(), Value::String(new_text.to_string()));
                return true;
            }
            if map.contains_key("content") {
                map.insert("content".to_string(), Value::String(new_text.to_string()));
                return true;
            }
            if map.contains_key("output") {
                map.insert("output".to_string(), Value::String(new_text.to_string()));
                return true;
            }
            if map.contains_key("text") {
                map.insert("text".to_string(), Value::String(new_text.to_string()));
                return true;
            }
            if let Some(Value::Object(fr)) = map.get_mut("functionResponse") {
                fr.insert(
                    "response".to_string(),
                    json!({ "output": new_text.to_string() }),
                );
                return true;
            }
            false
        }
        _ => false,
    }
}

fn filter_tools_in_payload(
    payload: &mut Value,
    disabled_tools: &[String],
    disabled_servers: &[String],
) -> bool {
    let Some(tools) = payload.get_mut("tools").and_then(|v| v.as_array_mut()) else {
        return false;
    };
    let before_len = tools.len();

    tools.retain(|tool| {
        // Anthropic / OpenAI tool name check
        if let Some(name) = tool
            .get("name")
            .or_else(|| tool.get("function").and_then(|f| f.get("name")))
            .and_then(|n| n.as_str())
        {
            return !is_tool_disabled(name, disabled_tools, disabled_servers);
        }
        true
    });

    // Also filter Gemini `functionDeclarations` / `function_declarations` if present
    let mut gemini_changed = false;
    for tool_group in tools.iter_mut() {
        for key in ["functionDeclarations", "function_declarations"] {
            if let Some(decls) = tool_group.get_mut(key).and_then(|v| v.as_array_mut()) {
                let prev = decls.len();
                decls.retain(|d| {
                    let name = d.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    !is_tool_disabled(name, disabled_tools, disabled_servers)
                });
                if decls.len() != prev {
                    gemini_changed = true;
                }
            }
        }
    }

    tools.len() != before_len || gemini_changed
}

fn is_tool_disabled(
    full_name: &str,
    disabled_tools: &[String],
    disabled_servers: &[String],
) -> bool {
    if disabled_tools
        .iter()
        .any(|t| t == full_name || full_name.ends_with(t))
    {
        return true;
    }
    for srv in disabled_servers {
        if full_name.starts_with(&format!("mcp_{srv}_"))
            || full_name.starts_with(&format!("{srv}__"))
            || full_name.contains(srv)
        {
            return true;
        }
    }
    false
}

fn sort_tools_in_payload(payload: &mut Value) -> bool {
    let Some(tools) = payload.get_mut("tools").and_then(|v| v.as_array_mut()) else {
        return false;
    };
    tools.sort_by(|a, b| {
        let name_a = a
            .get("name")
            .or_else(|| a.get("function").and_then(|f| f.get("name")))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let name_b = b
            .get("name")
            .or_else(|| b.get("function").and_then(|f| f.get("name")))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        name_a.cmp(name_b)
    });

    // Also sort Gemini function declarations inside each tool group
    for tool_group in tools.iter_mut() {
        for key in ["functionDeclarations", "function_declarations"] {
            if let Some(decls) = tool_group.get_mut(key).and_then(|v| v.as_array_mut()) {
                decls.sort_by(|a, b| {
                    let name_a = a.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    let name_b = b.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    name_a.cmp(name_b)
                });
            }
        }
    }

    true
}

fn strip_dynamic_time_in_system(payload: &mut Value) -> bool {
    let mut changed = false;
    if let Some(Value::String(s)) = payload.get_mut("system")
        && DYNAMIC_TIME_RE.is_match(s)
    {
        *s = DYNAMIC_TIME_RE.replace_all(s, "").to_string();
        changed = true;
    }
    changed
}

fn inject_pinned_context_at_tail(payload: &mut Value, pinned: &[(String, String)]) {
    let mut block = String::from("\n<cw_pinned_context>\n");
    for (label, content) in pinned {
        block.push_str(&format!("- [{label}]: {content}\n"));
    }
    block.push_str("</cw_pinned_context>");

    // Try Anthropic / OpenAI `messages` array tail
    if let Some(messages) = payload.get_mut("messages").and_then(|v| v.as_array_mut())
        && let Some(last_msg) = messages.last_mut()
    {
        match last_msg.get_mut("content") {
            Some(Value::String(s)) => {
                s.push_str(&block);
                return;
            }
            Some(Value::Array(arr)) => {
                arr.push(json!({
                    "type": "text",
                    "text": block
                }));
                return;
            }
            _ => {}
        }
    }

    // Try Gemini `contents` array tail
    if let Some(contents) = payload.get_mut("contents").and_then(|v| v.as_array_mut())
        && let Some(last_content) = contents.last_mut()
        && let Some(parts) = last_content.get_mut("parts").and_then(|v| v.as_array_mut())
    {
        parts.push(json!({ "text": block }));
    }
}
