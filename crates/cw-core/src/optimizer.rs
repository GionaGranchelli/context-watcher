use crate::ir::{
    ContextTurn, LintRuleId, MutationOp, MutationRule, MutationStatus, NodeKind, ProviderKind,
    ToolSemanticMeta,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizationCandidateItem {
    pub item_id: String,
    pub node_id: Option<String>,
    pub turn_seq: u32,
    pub timestamp: Option<String>,
    pub title: String,
    pub action_kind: String, // "cut_output" | "shadow_dedupe" | "prune_thinking" | "fold_turns" | "pin_rule" | "gate_tool"
    pub what_we_do: String,
    pub why_it_matters: String,
    pub content_preview: String,
    pub tokens_before: u32,
    pub tokens_after: u32,
    pub saved_tokens: i32,
    pub kv_break_tokens: u32,
    pub mutation_op: MutationOp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizationStrategyPlan {
    pub id: String,
    pub name: String,
    pub tagline: String,
    pub kv_badge: String,
    pub what_and_how: String,
    pub expected_quality_impact: String,
    pub tokens_before: u32,
    pub tokens_after: u32,
    pub saved_tokens: u32,
    pub saved_pct: f32,
    pub kv_cache_break_tokens: u32,
    pub break_even_turns: u32,
    pub health_score_before: u8,
    pub health_score_after: u8,
    pub items: Vec<OptimizationCandidateItem>,
}

/// Generates all 5 interactive Optimization Strategy Plans for a `ContextTurn`,
/// with heuristic per-node estimates. Use the payload variant for whole-plan totals.
pub fn build_optimization_plans(turn: &ContextTurn) -> Vec<OptimizationStrategyPlan> {
    if !turn.analysis_complete
        || turn.nodes.iter().any(|node| {
            matches!(
                &node.kind,
                NodeKind::AssistantThinking {
                    signature: Some(_),
                    ..
                }
            )
        })
    {
        return Vec::new();
    }
    let total_tokens = turn.token_usage.input_tokens.max(1);
    let max_turn = turn
        .nodes
        .iter()
        .map(|n| n.turn_seq)
        .max()
        .unwrap_or(1)
        .max(1);

    // Compute kv_break_tokens using centralized prefix-invalidation calculator
    let compute_kv_break = |start: u32, count: u32, is_append: bool| -> u32 {
        crate::radix::calculate_kv_break_tokens(
            start,
            count,
            is_append,
            total_tokens,
            Some(&turn.kv_cache_report),
        )
    };

    // Identify stale/duplicate file read nodes from linter findings
    let mut stale_or_dup_node_ids: HashSet<String> = HashSet::new();
    for f in &turn.lint_findings {
        if matches!(
            f.rule_id,
            LintRuleId::StaleFileInContext | LintRuleId::DuplicateFileRead
        ) {
            for id in &f.affected_node_ids {
                stale_or_dup_node_ids.insert(id.clone());
            }
        }
    }

    // Collect candidate buckets
    let mut pin_items = Vec::new();
    let mut tail_bloat_items = Vec::new();
    let mut stale_file_items = Vec::new();
    let mut log_and_thought_items = Vec::new();
    let fold_items: Vec<OptimizationCandidateItem> = Vec::new();
    let mut gate_tool_items = Vec::new();

    // Track which MCP servers were actually called
    let mut called_tools: HashSet<String> = HashSet::new();
    for n in &turn.nodes {
        if let NodeKind::ToolCall { tool_name, .. } = &n.kind {
            called_tools.insert(tool_name.clone());
        }
    }

    let last_assistant_idx = turn
        .nodes
        .iter()
        .rposition(|n| matches!(n.kind, NodeKind::AssistantText { .. }))
        .unwrap_or(0);

    for (node_idx, n) in turn.nodes.iter().enumerate() {
        let tok = n.token_span.count;
        let text = n.kind.text_for_tokenization();
        if text.starts_with("[cw:pruned")
            || text.starts_with("[cw:shadow_deduped")
            || text.starts_with("[cw:folded")
        {
            continue;
        }
        let preview: String = text.chars().take(180).collect();

        // 1. Low-attention rules -> Pin to tail (0% KV break).
        // Never auto-pin UserMessage turns (prevents pinning casual user requests every turn).
        if matches!(n.kind, NodeKind::InjectedRule { .. })
            && total_tokens >= 30_000
            && n.attention_salience < 0.42
        {
            let snippet: String = text.chars().take(160).collect();
            pin_items.push(OptimizationCandidateItem {
                item_id: format!("pin_{}", n.id),
                node_id: Some(n.id.clone()),
                turn_seq: n.turn_seq,
                timestamp: n.timestamp.clone(),
                title: format!(
                    "Pin '{}' to High-Attention Tail (Currently {:.0}% Salience)",
                    n.kind.short_label(),
                    n.attention_salience * 100.0
                ),
                action_kind: "pin_rule".to_string(),
                what_we_do: "Appends a concise <cw_pinned_context> reminder at the very end of the prompt without editing the early prefix.".to_string(),
                why_it_matters: "Moves critical instructions out of the 'Lost in the Middle' attention trough (<45% salience) into the ~98% recency zone with ZERO KV cache invalidation.".to_string(),
                content_preview: preview.clone(),
                tokens_before: 0,
                tokens_after: 28,
                saved_tokens: -28,
                kv_break_tokens: 0,
                mutation_op: MutationOp::PinContext {
                    label: n.kind.short_label(),
                    content: snippet,
                    source_node_id: Some(n.id.clone()),
                },
            });
        }

        // 2. Stale or duplicate file reads
        if stale_or_dup_node_ids.contains(&n.id)
            && turn
                .nodes
                .iter()
                .skip_while(|other| other.id != n.id)
                .skip(1)
                .any(|newer| crate::mutation::read_is_covered_by(n, newer))
        {
            let path = match &n.kind {
                NodeKind::ToolResult {
                    semantic_meta: Some(ToolSemanticMeta::FileRead { path, .. }),
                    ..
                } => Some(path.clone()),
                _ => None,
            };
            let after_tok = 14u32.min(tok);
            let saved = (tok as i32) - (after_tok as i32);
            let kv_break = compute_kv_break(n.token_span.start, tok, false);
            stale_file_items.push(OptimizationCandidateItem {
                item_id: format!("dedupe_{}", n.id),
                node_id: Some(n.id.clone()),
                turn_seq: n.turn_seq,
                timestamp: n.timestamp.clone(),
                title: format!("Shadow-Dedupe Stale/Duplicate {}", n.kind.short_label()),
                action_kind: "shadow_dedupe".to_string(),
                what_we_do: format!(
                    "Replaces this outdated/duplicate file read at Turn #{} ({} tok) with a 1-line pointer to the newest version while preserving tool_use_id pairing.",
                    n.turn_seq, tok
                ),
                why_it_matters: "Eliminates contradictory versions of the same file from the context window so the LLM stops hallucinating old code or line numbers.".to_string(),
                content_preview: preview.clone(),
                tokens_before: tok,
                tokens_after: after_tok,
                saved_tokens: saved,
                kv_break_tokens: kv_break,
                mutation_op: MutationOp::ShadowDedupe { file_path: path },
            });
        }

        // 3. Tool outputs (Command logs, directory listings, old tool results)
        // Never stub unread tool results at the tail of the prompt, current-turn outputs,
        // contextWatcher's own MCP inspection outputs, or skill payloads.
        if let NodeKind::ToolResult {
            call_id,
            tool_name,
            content,
            semantic_meta,
            ..
        } = &n.kind
        {
            let is_fresh_file = matches!(semantic_meta, Some(ToolSemanticMeta::FileRead { .. }))
                && !stale_or_dup_node_ids.contains(&n.id);
            let lower_tool = tool_name.to_ascii_lowercase();
            let is_cw_or_skill = lower_tool.starts_with("cw_")
                || lower_tool.contains("contextwatcher")
                || lower_tool.contains("context_watcher")
                || lower_tool.contains("skill")
                || content.contains("\"health_score\"")
                || content.contains("\"kv_cache_report\"")
                || content.contains("<SKILL>")
                || content.contains("SKILL.md");
            let is_unread_tail = (last_assistant_idx > 0 && node_idx > last_assistant_idx)
                || node_idx + 1 == turn.nodes.len()
                || n.turn_seq >= max_turn;

            if !is_fresh_file && !is_cw_or_skill && !is_unread_tail && tok > 100 {
                let after_tok = 16u32.min(tok);
                let saved = (tok as i32) - (after_tok as i32);
                let kv_break = compute_kv_break(n.token_span.start, tok, false);
                let item = OptimizationCandidateItem {
                    item_id: format!("cut_tool_{}", n.id),
                    node_id: Some(n.id.clone()),
                    turn_seq: n.turn_seq,
                    timestamp: n.timestamp.clone(),
                    title: format!(
                        "Compact Tool Output: {} (Turn #{})",
                        n.kind.short_label(),
                        n.turn_seq
                    ),
                    action_kind: "cut_output".to_string(),
                    what_we_do: format!(
                        "Replaces raw '{}' output ({} tok) at {} with a compact 1-line outcome stub while keeping call_id '{}' intact.",
                        tool_name, tok, n.raw_json_pointer, call_id
                    ),
                    why_it_matters: format!(
                        "The model already processed and acted on this Turn #{} output. Keeping verbose old command logs dilutes attention on current tasks.",
                        n.turn_seq
                    ),
                    content_preview: preview.clone(),
                    tokens_before: tok,
                    tokens_after: after_tok,
                    saved_tokens: saved,
                    kv_break_tokens: kv_break,
                    mutation_op: MutationOp::PruneOrStub {
                        target_fingerprint: Some(n.fingerprint.0.clone()),
                        target_call_id: if call_id.is_empty() {
                            None
                        } else {
                            Some(call_id.clone())
                        },
                        target_json_pointer: if call_id.is_empty() {
                            Some(n.raw_json_pointer.clone())
                        } else {
                            None
                        },
                        stub_text: format!(
                            "[cw:pruned tool=\"{}\" turn={} original_tokens={}]",
                            tool_name, n.turn_seq, tok
                        ),
                    },
                };
                if n.turn_seq == max_turn.saturating_sub(1) && tok > 100 {
                    tail_bloat_items.push(item.clone());
                }
                log_and_thought_items.push(item);
            }
        }

        // 4. Old Assistant Thinking blocks
        if matches!(
            n.kind,
            NodeKind::AssistantThinking {
                signature: None,
                ..
            }
        ) && n.turn_seq < max_turn
        {
            let after_tok = 8u32.min(tok);
            let saved = (tok as i32) - (after_tok as i32);
            let kv_break = compute_kv_break(n.token_span.start, tok, false);
            let think_item = OptimizationCandidateItem {
                item_id: format!("cut_think_{}", n.id),
                node_id: Some(n.id.clone()),
                turn_seq: n.turn_seq,
                timestamp: n.timestamp.clone(),
                title: format!("Prune Completed Thinking Block (Turn #{})", n.turn_seq),
                action_kind: "prune_thinking".to_string(),
                what_we_do: format!(
                    "Replaces internal reasoning text at Turn #{} ({} tok) with a minimal placeholder while preserving any cryptographic signature.",
                    n.turn_seq, tok
                ),
                why_it_matters: "Internal chain-of-thought from earlier turns already produced its tool call or answer; trimming old monologue frees context budget with zero loss of factual history.".to_string(),
                content_preview: preview.clone(),
                tokens_before: tok,
                tokens_after: after_tok,
                saved_tokens: saved,
                kv_break_tokens: kv_break,
                mutation_op: MutationOp::PruneOrStub {
                    target_fingerprint: Some(n.fingerprint.0.clone()),
                    target_call_id: None,
                    target_json_pointer: Some(n.raw_json_pointer.clone()),
                    stub_text: format!("[cw:pruned_thinking turn={}]", n.turn_seq),
                },
            };
            if n.turn_seq >= max_turn.saturating_sub(1) && tok > 50 {
                tail_bloat_items.push(think_item.clone());
            }
            log_and_thought_items.push(think_item);
        }

        // 5. Unused MCP Tool Schemas
        if let NodeKind::McpToolSchema {
            server_name,
            tool_name,
            ..
        } = &n.kind
            && !called_tools.iter().any(|c| c.contains(tool_name))
        {
            let kv_break = compute_kv_break(n.token_span.start, tok, false);
            gate_tool_items.push(OptimizationCandidateItem {
                item_id: format!("gate_{}", n.id),
                node_id: Some(n.id.clone()),
                turn_seq: 0,
                timestamp: None,
                title: format!(
                    "Gate Off Unused Tool Schema: {}{}",
                    server_name
                        .as_ref()
                        .map(|s| format!("{s}/"))
                        .unwrap_or_default(),
                    tool_name
                ),
                action_kind: "gate_tool".to_string(),
                what_we_do: format!(
                    "Strips the JSON schema definition for '{}' ({} tok) from the outgoing 'tools' array.",
                    tool_name, tok
                ),
                why_it_matters: "This tool has been called 0 times in this session. Removing unused tool schemas saves tokens on every turn and increases LLM tool-selection accuracy.".to_string(),
                content_preview: preview,
                tokens_before: tok,
                tokens_after: 0,
                saved_tokens: tok as i32,
                kv_break_tokens: kv_break,
                mutation_op: MutationOp::GateToolsAndSkills {
                    disabled_tools: vec![tool_name.clone()],
                    disabled_mcp_servers: vec![],
                    disabled_skills: vec![],
                },
            });
        }
    }

    // Automatic folds cannot preserve factual outcomes without a reviewed summary.
    let fold_end = max_turn.saturating_sub(1);

    // Assemble the 5 Strategy Plans
    // Strategy 1 (cache_safe_boost / min): Strictly prefix-preserving candidates only.
    // Retains 100% of historical tokens; pins critical rules from the middle attention trough (<42%) to the high-attention tail.
    // Never modifies historical tokens or the tokenized prefix, ensuring 0% KV-cache break on both Turn N and Turn N+1.
    let mut strategy1_seen = HashSet::new();
    let mut strategy1_items = Vec::new();
    for item in &pin_items {
        let key = item.node_id.clone().unwrap_or_else(|| item.item_id.clone());
        if strategy1_seen.insert(key) {
            strategy1_items.push(item.clone());
        }
    }

    let mut strategy2_items = stale_file_items.clone();
    if strategy2_items.is_empty() {
        // If no explicitly flagged stale reads, include any earlier file reads
        for item in &log_and_thought_items {
            if item.title.contains("Read:") {
                strategy2_items.push(item.clone());
            }
        }
    }

    let strategy3_items = log_and_thought_items.clone();
    let strategy4_items = fold_items.clone();

    // Strategy 5: Max Token Diet (Deep Trajectory Fold + Stale files + Unused MCP Tools + Pinning)
    let mut seen_ids = HashSet::new();
    let mut strategy5_items = Vec::new();
    if !fold_items.is_empty() {
        for item in &fold_items {
            strategy5_items.push(item.clone());
        }
        for item in stale_file_items
            .into_iter()
            .chain(
                log_and_thought_items
                    .into_iter()
                    .filter(|i| i.turn_seq > fold_end),
            )
            .chain(pin_items)
        {
            let key = item.node_id.clone().unwrap_or_else(|| item.item_id.clone());
            if seen_ids.insert(key) {
                strategy5_items.push(item);
            }
        }
    } else {
        for item in stale_file_items
            .into_iter()
            .chain(log_and_thought_items)
            .chain(pin_items)
        {
            let key = item.node_id.clone().unwrap_or_else(|| item.item_id.clone());
            if seen_ids.insert(key) {
                strategy5_items.push(item);
            }
        }
    }

    vec![
        finalize_plan(
            "cache_safe_boost",
            "🛡️ 1. Preserve Prefix (Minimum)",
            "Zero-KV-break candidates only (uncached duplicate reads, uncached tail bloat, and rule reminders)",
            "🟢 0% KV Cache Break",
            "Selects only mutations with zero cached-prefix invalidation (kv_break_tokens == 0) while leaving any cached prefix untouched.",
            "Preserves cached prefix tokens; use clean_code_state ('clean') to deduplicate covered reads inside an already-cached prefix.",
            total_tokens,
            turn.health_score,
            strategy1_items,
            6,
        ),
        finalize_plan(
            "clean_code_state",
            "🔁 2. Clean Code State (Stale & Duplicate File Eviction)",
            "Removes earlier reads only when a later successful read covers identical contents",
            "🟡 Surgical Prefix Dedupe",
            "Compares successful file reads and their line ranges. Disjoint ranges, changed text, failed reads and unique contents remain intact.",
            "May reduce repeated text. Continuation quality requires a separate task test.",
            total_tokens,
            turn.health_score,
            strategy2_items,
            15,
        ),
        finalize_plan(
            "surgical_log_diet",
            "🧹 3. Surgical Tool Log & Thinking Diet",
            "Cuts old command outputs, stack traces, and completed thinking blocks",
            "🟡 High Savings • Preserves Full Conversation",
            "Keeps all user requests, assistant answers, and fresh file reads intact, but compacts verbose command/tool outputs (cargo test, ls, grep, stack traces) and raw thinking blocks from completed turns into concise 1-line stubs.",
            "Estimated savings depend on the selected nodes. Signed blocks and current unread results are preserved.",
            total_tokens,
            turn.health_score,
            strategy3_items,
            20,
        ),
        finalize_plan(
            "deep_trajectory_fold",
            "📁 4. Deep Trajectory Fold (Sub-Task Compaction)",
            "Automatic trajectory folding is unavailable without a verified outcome summary",
            "🟠 Deep Compaction (Best Between Sub-Tasks)",
            "No automatic folds are proposed. Manual folds require a user-supplied summary and retain originals in the store.",
            "Continuation quality is not inferred from token reduction.",
            total_tokens,
            turn.health_score,
            strategy4_items,
            18,
        ),
        finalize_plan(
            "max_token_diet",
            "🔥 5. Max Token Diet (Full All-In-One Optimization)",
            "Combines covered-read dedupe, eligible old outputs and optional reminders",
            "Prefix may change; review before applying",
            "Combines the available conservative candidates. Tool schemas and automatic folds are preserved.",
            "Savings and cache effects are estimates; no health or quality increase is assumed.",
            total_tokens,
            turn.health_score,
            strategy5_items,
            35,
        ),
    ]
}

/// Apply each complete plan to a copy so overlapping edits are counted only once.
pub fn build_optimization_plans_for_payload(
    turn: &ContextTurn,
    payload: &serde_json::Value,
    provider: ProviderKind,
) -> Vec<OptimizationStrategyPlan> {
    if !crate::normalizer::supports_mutation_payload(payload, provider)
        || crate::mutation::has_signed_content(payload)
    {
        return Vec::new();
    }
    let mut plans = build_optimization_plans(turn);
    let count = |value: &serde_json::Value| {
        crate::normalizer::parse_request_to_nodes(value, provider, turn.sequence_number)
            .map(|nodes| nodes.iter().map(|n| n.token_span.count).sum::<u32>())
            .unwrap_or(0)
    };
    let before = count(payload);
    for plan in &mut plans {
        let rules: Vec<_> = plan
            .items
            .iter()
            .map(|item| MutationRule {
                id: item.item_id.clone(),
                session_id: turn.session_id.clone(),
                op: item.mutation_op.clone(),
                status: MutationStatus::Committed,
                estimated_saved_tokens: 0,
                estimated_kv_break_tokens: 0,
                created_at: turn.created_at,
            })
            .collect();
        let mut preview = payload.clone();
        crate::mutation::apply_mutations_to_payload(
            &mut preview,
            provider,
            &rules,
            turn.sequence_number,
        );
        plan.tokens_before = before;
        plan.tokens_after = count(&preview);
        plan.saved_tokens = before.saturating_sub(plan.tokens_after);
        plan.saved_pct = if before > 0 {
            plan.saved_tokens as f32 / before as f32 * 100.0
        } else {
            0.0
        };
    }
    plans
}

#[allow(clippy::too_many_arguments)]
fn finalize_plan(
    id: &str,
    name: &str,
    tagline: &str,
    kv_badge: &str,
    what_and_how: &str,
    expected_quality_impact: &str,
    tokens_before: u32,
    health_before: u8,
    items: Vec<OptimizationCandidateItem>,
    health_bonus: u8,
) -> OptimizationStrategyPlan {
    let net_saved_i32: i32 = items.iter().map(|i| i.saved_tokens).sum();
    let saved_tokens = net_saved_i32.max(0) as u32;
    let tokens_after = (tokens_before as i32 - net_saved_i32).max(1) as u32;
    let saved_pct = if tokens_before > 0 {
        (saved_tokens as f32 / tokens_before as f32) * 100.0
    } else {
        0.0
    };
    let kv_break = items.iter().map(|i| i.kv_break_tokens).max().unwrap_or(0);
    let break_even_turns = if kv_break == 0 || saved_tokens == 0 {
        0
    } else {
        kv_break.div_ceil(saved_tokens).max(1)
    };
    let _ = health_bonus;
    let health_after = health_before;

    OptimizationStrategyPlan {
        id: id.to_string(),
        name: name.to_string(),
        tagline: tagline.to_string(),
        kv_badge: kv_badge.to_string(),
        what_and_how: what_and_how.to_string(),
        expected_quality_impact: expected_quality_impact.to_string(),
        tokens_before,
        tokens_after,
        saved_tokens,
        saved_pct,
        kv_cache_break_tokens: kv_break,
        break_even_turns,
        health_score_before: health_before,
        health_score_after: health_after,
        items,
    }
}

/// Maps short human/slash-command intensity levels (`min`, `mid`, `max`) to canonical strategy IDs.
pub fn resolve_strategy_alias(input: &str) -> &str {
    match input.trim().to_ascii_lowercase().as_str() {
        "min" | "lite" | "safe" => "cache_safe_boost",
        "mid" | "medium" | "balanced" | "log" => "surgical_log_diet",
        "max" | "ultra" | "full" | "deep" => "max_token_diet",
        "boost" | "pin" => "cache_safe_boost",
        "clean" | "dedupe" => "clean_code_state",
        "fold" => "deep_trajectory_fold",
        _ => input.trim(),
    }
}
