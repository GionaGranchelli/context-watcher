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

static PINNED_CONTEXT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)\n?<cw_pinned_context>\n?.*?</cw_pinned_context>").unwrap());

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
    if active_rules.is_empty()
        || has_signed_content(payload)
        || !crate::normalizer::supports_mutation_payload(payload, provider)
    {
        return MutationApplicationReport::default();
    }

    let before_nodes = parse_request_to_nodes(payload, provider, turn_seq).unwrap_or_default();
    let before_tokens: u32 = before_nodes.iter().map(|n| n.token_span.count).sum();
    let last_assistant_idx = before_nodes
        .iter()
        .rposition(|n| {
            matches!(
                n.kind,
                NodeKind::AssistantText { .. }
                    | NodeKind::AssistantThinking { .. }
                    | NodeKind::ToolCall { .. }
            )
        })
        .unwrap_or(0);

    let mut applied_ids = Vec::new();
    let mut pinned_blocks: Vec<(String, String)> = Vec::new();
    let mut pin_rule_ids = Vec::new();

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
                for (node_idx, node) in before_nodes.iter().enumerate() {
                    if is_protected_tool_result(
                        node,
                        node_idx,
                        before_nodes.len(),
                        last_assistant_idx,
                    ) {
                        continue;
                    }
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
                    let identity_matches = if target_fingerprint.is_some() {
                        fp_match && (target_call_id.is_none() || call_match)
                    } else {
                        call_match
                    };
                    let location_matches = target_json_pointer
                        .as_ref()
                        .is_none_or(|p| p == &node.raw_json_pointer);
                    if identity_matches
                        && location_matches
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
                for (node_idx, node) in before_nodes.iter().enumerate() {
                    if is_protected_tool_result(
                        node,
                        node_idx,
                        before_nodes.len(),
                        last_assistant_idx,
                    ) {
                        continue;
                    }
                    let is_foldable_kind = matches!(
                        node.kind,
                        NodeKind::ToolResult { .. } | NodeKind::AssistantThinking { .. }
                    );
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
                for (idx, read) in before_nodes.iter().enumerate() {
                    let NodeKind::ToolResult {
                        semantic_meta: Some(ToolSemanticMeta::FileRead { path, .. }),
                        ..
                    } = &read.kind
                    else {
                        continue;
                    };
                    if file_path.as_ref().is_some_and(|wanted| wanted != path) {
                        continue;
                    }
                    if let Some(newer) = before_nodes[idx + 1..]
                        .iter()
                        .find(|newer| read_is_covered_by(read, newer))
                    {
                        let stub = format!(
                            "[cw:shadow_deduped path={path:?} superseded_by_turn={}]",
                            newer.turn_seq
                        );
                        applied |=
                            replace_node_text_at_pointer(payload, &read.raw_json_pointer, &stub);
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
                let clean_content = PINNED_CONTEXT_RE
                    .replace_all(content, "")
                    .replace("<cw_pinned_context>", "")
                    .replace("</cw_pinned_context>", "")
                    .trim()
                    .to_string();
                let clean_label = label.trim().to_string();
                if !clean_content.is_empty() {
                    let already_exists = pinned_blocks.iter().any(|(l, c)| {
                        (l.eq_ignore_ascii_case(&clean_label)
                            && c.eq_ignore_ascii_case(&clean_content))
                            || c == &clean_content
                    });
                    if !already_exists {
                        pinned_blocks.push((clean_label, clean_content));
                    }
                    pin_rule_ids.push(rule.id.clone());
                    applied = true;
                }
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

    if !pinned_blocks.is_empty() && !inject_pinned_context_at_tail(payload, &pinned_blocks) {
        applied_ids.retain(|id| !pin_rule_ids.contains(id));
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
pub fn estimate_mutation_impact(
    op: &MutationOp,
    nodes: &[ContextNode],
    report: Option<&crate::ir::KvCacheReport>,
) -> (i32, u32) {
    let total_tokens: u32 = nodes
        .iter()
        .map(|n| n.token_span.end)
        .max()
        .unwrap_or(0)
        .max(nodes.iter().map(|n| n.token_span.count).sum());

    let is_append = op.is_append_only();
    let calc_break = |start: u32, count: u32| -> u32 {
        crate::radix::calculate_kv_break_tokens(start, count, is_append, total_tokens, report)
    };

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
                    let kv_break = calc_break(node.token_span.start, node.token_span.count);
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
            let mut earliest_start = total_tokens;
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
            let kv_break = calc_break(earliest_start, target_tokens);
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
            let mut earliest_start = total_tokens;
            for (path, reads) in reads_by_path {
                let write_turn = latest_write_by_path.get(&path).copied().unwrap_or(0);
                for (i, r) in reads.iter().enumerate() {
                    if i + 1 < reads.len() || r.turn_seq < write_turn {
                        saved += (r.token_span.count as i32) - 24;
                        earliest_start = earliest_start.min(r.token_span.start);
                    }
                }
            }
            (saved, calc_break(earliest_start, 1))
        }
        MutationOp::GateToolsAndSkills {
            disabled_tools,
            disabled_mcp_servers,
            ..
        } => {
            let mut saved = 0i32;
            let mut earliest = total_tokens;
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
            (saved, calc_break(earliest, 1))
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
                        && !is_protected_tool_result(
                            n,
                            nodes
                                .iter()
                                .position(|candidate| candidate.id == n.id)
                                .unwrap_or(0),
                            nodes.len(),
                            nodes
                                .iter()
                                .rposition(|candidate| {
                                    matches!(
                                        candidate.kind,
                                        NodeKind::ToolCall { .. }
                                            | NodeKind::AssistantText { .. }
                                            | NodeKind::AssistantThinking { .. }
                                    )
                                })
                                .unwrap_or(0),
                        )
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

        if !matches!(rule.action, FirewallAction::PauseForHumanApproval { .. })
            && (has_signed_content(payload)
                || !crate::normalizer::supports_mutation_payload(payload, provider))
        {
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
                    if nodes.iter().enumerate().any(|(index, node)| {
                        node.raw_json_pointer == ptr
                            && is_protected_tool_result(
                                node,
                                index,
                                nodes.len(),
                                nodes
                                    .iter()
                                    .rposition(|n| {
                                        matches!(
                                            n.kind,
                                            NodeKind::ToolCall { .. }
                                                | NodeKind::AssistantText { .. }
                                                | NodeKind::AssistantThinking { .. }
                                        )
                                    })
                                    .unwrap_or(0),
                            )
                    }) {
                        continue;
                    }
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
    if pointer.contains('#') {
        return false;
    }
    let base_ptr = pointer;
    let Some(target) = payload.pointer_mut(base_ptr) else {
        return false;
    };

    match target {
        Value::String(s) => {
            *s = new_text.to_string();
            true
        }
        Value::Array(blocks)
            if !blocks.is_empty()
                && blocks.iter().all(|block| {
                    matches!(
                        block.get("type").and_then(Value::as_str),
                        Some("text" | "input_text" | "output_text")
                    ) && block.get("text").is_some_and(Value::is_string)
                }) =>
        {
            for (index, block) in blocks.iter_mut().enumerate() {
                block["text"] = Value::String(if index == 0 {
                    new_text.to_owned()
                } else {
                    String::new()
                });
            }
            true
        }
        Value::Object(map) => {
            if map.contains_key("thinking") {
                map.insert("thinking".to_string(), Value::String(new_text.to_string()));
                return true;
            }
            for key in ["content", "output"] {
                if map
                    .get_mut(key)
                    .is_some_and(|value| replace_text_value(value, new_text))
                {
                    return true;
                }
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

fn replace_text_value(value: &mut Value, text: &str) -> bool {
    match value {
        Value::String(current) => {
            *current = text.to_owned();
            true
        }
        Value::Array(blocks)
            if !blocks.is_empty()
                && blocks.iter().all(|b| {
                    matches!(
                        b.get("type").and_then(Value::as_str),
                        Some("text" | "input_text" | "output_text")
                    ) && b.get("text").is_some_and(Value::is_string)
                }) =>
        {
            for (i, b) in blocks.iter_mut().enumerate() {
                b["text"] = Value::String(if i == 0 {
                    text.to_owned()
                } else {
                    String::new()
                });
            }
            true
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

fn is_protected_tool_result(
    node: &ContextNode,
    node_idx: usize,
    total_nodes: usize,
    last_assistant_idx: usize,
) -> bool {
    let NodeKind::ToolResult {
        tool_name, content, ..
    } = &node.kind
    else {
        return false;
    };
    if matches!(
        &node.kind,
        NodeKind::ToolResult {
            semantic_meta: Some(ToolSemanticMeta::FileRead { .. }),
            ..
        }
    ) {
        return true;
    }
    // Never prune an unread tool result at the tail of the prompt before the assistant responds
    if (last_assistant_idx > 0 && node_idx > last_assistant_idx) || node_idx + 1 == total_nodes {
        return true;
    }
    // Never prune contextWatcher's own MCP inspection outputs or skill payloads
    let lower_tool = tool_name.to_ascii_lowercase();
    lower_tool.starts_with("cw_")
        || lower_tool.contains("contextwatcher")
        || lower_tool.contains("context_watcher")
        || lower_tool.contains("skill")
        || content.contains("\"health_score\"")
        || content.contains("\"kv_cache_report\"")
        || content.contains("<SKILL>")
        || content.contains("SKILL.md")
}

fn inject_pinned_context_at_tail(payload: &mut Value, pinned: &[(String, String)]) -> bool {
    let mut block = String::from("\n<cw_pinned_context>\n");
    for (label, content) in pinned {
        block.push_str(&format!("- [{label}]: {content}\n"));
    }
    block.push_str("</cw_pinned_context>");

    if contains_text(payload, &block) {
        return true;
    }
    // Try Anthropic / OpenAI `messages` array tail
    if let Some(messages) = payload.get_mut("messages").and_then(|v| v.as_array_mut())
        && let Some(last_msg) = messages.last_mut()
    {
        match last_msg.get_mut("content") {
            Some(Value::String(s)) => {
                s.push_str(&block);
                return true;
            }
            Some(Value::Array(arr)) => {
                arr.push(json!({
                    "type": "text",
                    "text": block
                }));
                return true;
            }
            _ => {}
        }
    }

    // Responses API input accepts text blocks inside a message item.
    if let Some(input) = payload.get_mut("input") {
        match input {
            Value::String(text) => {
                text.push_str(&block);
                return true;
            }
            Value::Array(items) => {
                if let Some(message) = items
                    .last_mut()
                    .filter(|item| item.get("type").and_then(Value::as_str) == Some("message"))
                {
                    match message.get_mut("content") {
                        Some(Value::String(text)) => {
                            text.push_str(&block);
                            return true;
                        }
                        Some(Value::Array(content)) => {
                            content.push(json!({"type": "input_text", "text": block}));
                            return true;
                        }
                        _ => {}
                    }
                }
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
        return true;
    }
    false
}

/// Signed requests are opaque until a provider-specific edit capability is qualified.
pub fn has_signed_content(value: &Value) -> bool {
    match value {
        Value::Object(map) => {
            map.contains_key("signature")
                || map.contains_key("thinkingSignature")
                || map.contains_key("thoughtSignature")
                || map.contains_key("encrypted_content")
                || map.values().any(has_signed_content)
        }
        Value::Array(items) => items.iter().any(has_signed_content),
        _ => false,
    }
}

pub fn read_is_covered_by(older: &ContextNode, newer: &ContextNode) -> bool {
    match (&older.kind, &newer.kind) {
        (
            NodeKind::ToolResult {
                is_error: false,
                content: old,
                semantic_meta:
                    Some(ToolSemanticMeta::FileRead {
                        path: p,
                        start_line: a,
                        end_line: b,
                        ..
                    }),
                ..
            },
            NodeKind::ToolResult {
                is_error: false,
                content: new,
                semantic_meta:
                    Some(ToolSemanticMeta::FileRead {
                        path: q,
                        start_line: c,
                        end_line: d,
                        ..
                    }),
                ..
            },
        ) if p == q => match (a, b, c, d) {
            (Some(a), Some(b), Some(c), Some(d)) if c <= a && d >= b => {
                let count = b.saturating_sub(*a).saturating_add(1) as usize;
                old.lines().count() == count
                    && new
                        .lines()
                        .skip(a.saturating_sub(*c) as usize)
                        .take(count)
                        .eq(old.lines())
            }
            (Some(_), Some(_), Some(_), Some(_)) => false,
            _ => old == new && a == c && b == d,
        },
        _ => false,
    }
}

fn contains_text(value: &Value, wanted: &str) -> bool {
    match value {
        Value::String(text) => text.contains(wanted),
        Value::Array(items) => items.iter().any(|item| contains_text(item, wanted)),
        Value::Object(map) => map.values().any(|item| contains_text(item, wanted)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn pin_rule() -> MutationRule {
        MutationRule {
            id: "pin-1".into(),
            session_id: "session-1".into(),
            op: MutationOp::PinContext {
                label: "test".into(),
                content: "retain this".into(),
                source_node_id: None,
            },
            status: MutationStatus::Committed,
            estimated_saved_tokens: 0,
            estimated_kv_break_tokens: 0,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn pin_is_injected_into_responses_input_messages() {
        let mut payload = json!({
            "model": "test",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "question"}]
            }]
        });

        let report =
            apply_mutations_to_payload(&mut payload, ProviderKind::OpenAI, &[pin_rule()], 1);

        assert_eq!(report.applied_rule_ids, ["pin-1"]);
        let blocks = payload["input"][0]["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[1]["type"], "input_text");
        assert!(blocks[1]["text"].as_str().unwrap().contains("retain this"));
    }

    #[test]
    fn pin_is_not_reported_applied_without_a_supported_tail() {
        let mut payload = json!({
            "model": "test",
            "input": [{"type": "function_call", "call_id": "call-1", "name": "tool"}]
        });
        let original = payload.clone();

        let report =
            apply_mutations_to_payload(&mut payload, ProviderKind::OpenAI, &[pin_rule()], 1);

        assert!(report.applied_rule_ids.is_empty());
        assert_eq!(payload, original);
    }
}
