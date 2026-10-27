use crate::ir::{
    CategoryBreakdown, ContextNode, FileContextFreshness, FileContextState, KvCacheReport,
    LintFinding, LintRuleId, LintSeverity, MutationOp, NodeKind, ToolSemanticMeta,
};
use std::collections::{HashMap, HashSet};
use std::path::Path;

type FileReadEntry = (usize, u32, u32, String, Option<(u32, u32)>);

pub struct LintAnalysisOutput {
    pub findings: Vec<LintFinding>,
    pub category_breakdown: CategoryBreakdown,
    pub health_score: u8,
    pub file_states: Vec<FileContextState>,
}

/// Runs all 7 Context Health Linter rules (`CW-L001` .. `CW-L007`), computes the
/// `CategoryBreakdown`, calculates the overall `health_score` (0..100), and produces
/// per-file context freshness states for IDE & UI badges.
pub fn analyze_context_health(
    nodes: &[ContextNode],
    kv_report: &KvCacheReport,
    workspace_root: Option<&str>,
) -> LintAnalysisOutput {
    let mut findings = Vec::new();
    let mut breakdown = CategoryBreakdown::default();

    // Track file reads and writes in chronological order
    // path -> Vec<FileReadEntry>
    let mut file_reads: HashMap<String, Vec<FileReadEntry>> = HashMap::new();
    // path -> latest turn_seq where a write/edit occurred
    let mut latest_file_write_turn: HashMap<String, (u32, usize)> = HashMap::new();
    let mut pinned_paths: HashSet<String> = HashSet::new();

    for (idx, node) in nodes.iter().enumerate() {
        match &node.kind {
            NodeKind::ToolCall {
                semantic_meta: Some(ToolSemanticMeta::FileWrite { path, .. }),
                ..
            }
            | NodeKind::ToolResult {
                semantic_meta: Some(ToolSemanticMeta::FileWrite { path, .. }),
                ..
            } => {
                latest_file_write_turn.insert(path.clone(), (node.turn_seq, idx));
            }
            NodeKind::ToolResult {
                semantic_meta:
                    Some(ToolSemanticMeta::FileRead {
                        path,
                        start_line,
                        end_line,
                        content_hash,
                    }),
                ..
            } => {
                let range = Some((start_line.unwrap_or(1), end_line.unwrap_or(800)));
                file_reads.entry(path.clone()).or_default().push((
                    idx,
                    node.turn_seq,
                    node.token_span.count,
                    content_hash.clone(),
                    range,
                ));
            }
            NodeKind::PinnedReminder { label, summary, .. } => {
                pinned_paths.insert(label.clone());
                for token in summary.split_whitespace() {
                    if token.contains('/') || token.ends_with(".rs") || token.ends_with(".ts") {
                        pinned_paths.insert(token.to_string());
                    }
                }
            }
            _ => {}
        }
    }

    // Determine stale and duplicate file read node indices
    let mut stale_or_dup_node_indices: HashSet<usize> = HashSet::new();
    let mut file_states = Vec::new();

    for (path, reads) in &file_reads {
        let write_info = latest_file_write_turn.get(path).copied();
        let disk_modified_reason =
            check_file_modified_on_disk(path, workspace_root, reads.last(), nodes);
        let disk_modified = disk_modified_reason.is_some();

        // CW-L001: Stale File in Context
        let mut stale_nodes = Vec::new();
        let mut stale_tokens = 0u32;

        for (r_idx, &(node_idx, read_turn, tok_count, _, _)) in reads.iter().enumerate() {
            let superseded_by_write = write_info
                .map(|(_, write_node_idx)| write_node_idx > node_idx)
                .unwrap_or(false);
            let is_last_read = r_idx + 1 == reads.len();
            if superseded_by_write || (is_last_read && disk_modified) {
                stale_or_dup_node_indices.insert(node_idx);
                stale_nodes.push(nodes[node_idx].id.clone());
                stale_tokens += tok_count;
                let _ = read_turn;
            }
        }

        if !stale_nodes.is_empty() {
            let detail_msg = match &disk_modified_reason {
                Some(reason) => format!(
                    "File '{path}' was modified on disk ({reason}) after being read into context ({stale_tokens} stale tokens). The model may hallucinate outdated code."
                ),
                None => format!(
                    "File '{path}' was modified after being read into context ({stale_tokens} stale tokens). The model may hallucinate outdated code."
                ),
            };
            findings.push(LintFinding {
                rule_id: LintRuleId::StaleFileInContext,
                severity: LintSeverity::Warning,
                title: if disk_modified {
                    format!("Stale File Read (Modified on Disk): {path}")
                } else {
                    format!("Stale File Read: {path}")
                },
                message: detail_msg,
                affected_node_ids: stale_nodes,
                wasted_tokens: stale_tokens,
                suggested_mutation: Some(MutationOp::ShadowDedupe {
                    file_path: Some(path.clone()),
                }),
            });
        }

        // CW-L002: Duplicate File Read
        if reads.len() > 1 {
            let mut dup_nodes = Vec::new();
            let mut dup_tokens = 0u32;
            for &(node_idx, _, tok_count, _, _) in &reads[..reads.len() - 1] {
                stale_or_dup_node_indices.insert(node_idx);
                dup_nodes.push(nodes[node_idx].id.clone());
                dup_tokens += tok_count;
            }
            findings.push(LintFinding {
                rule_id: LintRuleId::DuplicateFileRead,
                severity: LintSeverity::Warning,
                title: format!("Duplicate File Reads ({}x): {path}", reads.len()),
                message: format!(
                    "File '{path}' appears {} times in context; earlier reads waste {dup_tokens} tokens.",
                    reads.len()
                ),
                affected_node_ids: dup_nodes,
                wasted_tokens: dup_tokens,
                suggested_mutation: Some(MutationOp::ShadowDedupe {
                    file_path: Some(path.clone()),
                }),
            });
        }

        let freshness = if pinned_paths
            .iter()
            .any(|p| path.ends_with(p) || p.contains(path))
        {
            FileContextFreshness::Pinned
        } else if stale_tokens > 0 {
            FileContextFreshness::Stale
        } else if reads.len() > 1 {
            FileContextFreshness::Duplicate
        } else {
            FileContextFreshness::Fresh
        };

        let last_turn = reads.last().map(|r| r.1).unwrap_or(0);
        let total_tok: u32 = reads.iter().map(|r| r.2).sum();
        let ranges: Vec<(u32, u32)> = reads.iter().filter_map(|r| r.4).collect();
        let n_ids: Vec<String> = reads.iter().map(|r| nodes[r.0].id.clone()).collect();

        file_states.push(FileContextState {
            path: path.clone(),
            freshness,
            last_read_turn: last_turn,
            read_count: reads.len() as u32,
            total_tokens_in_context: total_tok,
            line_ranges: ranges,
            node_ids: n_ids,
        });
    }

    // Populate CategoryBreakdown & check CW-L003 (Massive Tool Output Bloat)
    let mut mcp_tool_count = 0u32;
    let mut called_tool_names: HashSet<String> = HashSet::new();
    let mut mcp_servers_seen: HashSet<String> = HashSet::new();

    for (idx, node) in nodes.iter().enumerate() {
        let tok = node.token_span.count;
        match &node.kind {
            NodeKind::SystemPrompt { .. } => breakdown.system_and_identity += tok,
            NodeKind::InjectedRule { .. } => breakdown.rules_and_instructions += tok,
            NodeKind::SkillCatalog { .. } => breakdown.skills_catalog += tok,
            NodeKind::McpToolSchema {
                server_name,
                tool_name,
                ..
            } => {
                breakdown.mcp_and_native_tools += tok;
                mcp_tool_count += 1;
                if let Some(srv) = server_name {
                    mcp_servers_seen.insert(srv.clone());
                }
                let _ = tool_name;
            }
            NodeKind::UserMessage { .. } => breakdown.user_prompts += tok,
            NodeKind::AssistantThinking { .. } => breakdown.assistant_thinking += tok,
            NodeKind::AssistantText { .. } => breakdown.assistant_responses_and_calls += tok,
            NodeKind::ToolCall { tool_name, .. } => {
                breakdown.assistant_responses_and_calls += tok;
                called_tool_names.insert(tool_name.clone());
            }
            NodeKind::ToolResult {
                call_id,
                tool_name,
                semantic_meta,
                ..
            } => {
                match semantic_meta {
                    Some(ToolSemanticMeta::FileRead { .. }) => {
                        if stale_or_dup_node_indices.contains(&idx) {
                            breakdown.file_reads_stale_or_duplicate += tok;
                        } else {
                            breakdown.file_reads_fresh += tok;
                        }
                    }
                    _ => breakdown.command_outputs += tok,
                }

                // CW-L003: Massive Tool Output Bloat (> 3,000 tokens)
                if tok > 3000 {
                    findings.push(LintFinding {
                        rule_id: LintRuleId::MassiveToolOutputBloat,
                        severity: if tok > 8000 {
                            LintSeverity::Critical
                        } else {
                            LintSeverity::Warning
                        },
                        title: format!("Massive Tool Output ({tok} tok): {tool_name}"),
                        message: format!(
                            "Tool result '{}' at turn {} consumes {tok} tokens.",
                            node.kind.short_label(),
                            node.turn_seq
                        ),
                        affected_node_ids: vec![node.id.clone()],
                        wasted_tokens: tok.saturating_sub(200),
                        suggested_mutation: Some(MutationOp::PruneOrStub {
                            target_fingerprint: Some(node.fingerprint.0.clone()),
                            target_call_id: if call_id.is_empty() {
                                None
                            } else {
                                Some(call_id.clone())
                            },
                            target_json_pointer: Some(node.raw_json_pointer.clone()),
                            stub_text: format!(
                                "[cw:pruned original_tokens={tok} tool=\"{tool_name}\" summary=\"Large output compacted\"]"
                            ),
                        }),
                    });
                }
            }
            NodeKind::PinnedReminder { .. } | NodeKind::FoldedSubtree { .. } => {
                breakdown.pinned_and_folded += tok;
            }
        }
    }

    // CW-L004: Failed Debugging Loop (>= 3 consecutive error tool results)
    let mut consecutive_errors: Vec<&ContextNode> = Vec::new();
    for node in nodes {
        if let NodeKind::ToolResult {
            is_error,
            semantic_meta,
            ..
        } = &node.kind
        {
            let failed_exec = matches!(
                semantic_meta,
                Some(ToolSemanticMeta::CommandExec {
                    exit_code: Some(code),
                    ..
                }) if *code != 0
            );
            if *is_error || failed_exec {
                consecutive_errors.push(node);
            } else {
                if consecutive_errors.len() >= 3 {
                    emit_failed_loop_finding(&consecutive_errors, &mut findings);
                }
                consecutive_errors.clear();
            }
        }
    }
    if consecutive_errors.len() >= 3 {
        emit_failed_loop_finding(&consecutive_errors, &mut findings);
    }

    // CW-L005: MCP Tool Schema Bloat
    if breakdown.mcp_and_native_tools > 8000 || mcp_tool_count > 25 {
        let unused_servers: Vec<String> = mcp_servers_seen
            .into_iter()
            .filter(|srv| !called_tool_names.iter().any(|called| called.contains(srv)))
            .collect();
        findings.push(LintFinding {
            rule_id: LintRuleId::McpToolSchemaBloat,
            severity: LintSeverity::Warning,
            title: format!(
                "Tool Schema Bloat ({mcp_tool_count} tools • {} tok)",
                breakdown.mcp_and_native_tools
            ),
            message: format!(
                "{mcp_tool_count} tool schemas consume {} tokens on every turn. Disabling unused MCP servers improves tool selection accuracy.",
                breakdown.mcp_and_native_tools
            ),
            affected_node_ids: nodes
                .iter()
                .filter(|n| matches!(n.kind, NodeKind::McpToolSchema { .. }))
                .map(|n| n.id.clone())
                .collect(),
            wasted_tokens: breakdown.mcp_and_native_tools / 2,
            suggested_mutation: Some(MutationOp::GateToolsAndSkills {
                disabled_tools: vec![],
                disabled_mcp_servers: unused_servers,
                disabled_skills: vec![],
            }),
        });
    }

    // CW-L006: Unintended KV Cache Break
    if kv_report.invalidated_tokens > 0
        && let Some(cause) = &kv_report.invalidation_cause
    {
        let (node_id, detail) = match cause {
            crate::ir::CacheInvalidationCause::DynamicSystemInjection {
                node_id, detail, ..
            } => (Some(node_id.clone()), detail.clone()),
            crate::ir::CacheInvalidationCause::ToolSchemaMutated {
                node_id, detail, ..
            } => (Some(node_id.clone()), detail.clone()),
            crate::ir::CacheInvalidationCause::HistoryEditedOrCompacted {
                node_id, detail, ..
            } => (Some(node_id.clone()), detail.clone()),
            crate::ir::CacheInvalidationCause::CacheTtlExpired { previous_tokens } => (
                None,
                format!("KV cache TTL expired for {previous_tokens} previous tokens"),
            ),
        };
        findings.push(LintFinding {
            rule_id: LintRuleId::UnintendedKvCacheBreak,
            severity: LintSeverity::Critical,
            title: format!(
                "KV Cache Prefix Invalidated ({} tok evicted)",
                kv_report.invalidated_tokens
            ),
            message: detail,
            affected_node_ids: node_id.into_iter().collect(),
            wasted_tokens: kv_report.invalidated_tokens,
            suggested_mutation: Some(MutationOp::NormalizePrefix {
                sort_tools_deterministically: true,
                relocate_dynamic_system_metadata: true,
            }),
        });
    }

    // CW-L007: Lost-in-the-Middle Attention Risk
    let total_tokens: u32 = nodes.iter().map(|n| n.token_span.count).sum();
    if total_tokens > 8000 {
        for node in nodes {
            if matches!(node.kind, NodeKind::InjectedRule { .. }) && node.attention_salience < 0.40
            {
                let snippet: String = node
                    .kind
                    .text_for_tokenization()
                    .chars()
                    .take(120)
                    .collect();
                findings.push(LintFinding {
                    rule_id: LintRuleId::LostInTheMiddleRisk,
                    severity: LintSeverity::Info,
                    title: format!(
                        "Low Attention Salience ({:.0}%): {}",
                        node.attention_salience * 100.0,
                        node.kind.short_label()
                    ),
                    message: format!(
                        "Rule '{}' sits in the middle attention trough (salience {:.2}). Pin it to the tail to boost adherence.",
                        node.kind.short_label(),
                        node.attention_salience
                    ),
                    affected_node_ids: vec![node.id.clone()],
                    wasted_tokens: 0,
                    suggested_mutation: Some(MutationOp::PinContext {
                        label: node.kind.short_label(),
                        content: snippet,
                        source_node_id: Some(node.id.clone()),
                    }),
                });
            }
        }
    }

    // Compute overall Context Health Score (0..100)
    let mut penalty: u32 = 0;
    for f in &findings {
        penalty += match f.severity {
            LintSeverity::Critical => 18,
            LintSeverity::Warning => 9,
            LintSeverity::Info => 3,
        };
    }
    let health_score = 100u32.saturating_sub(penalty.min(95)) as u8;

    LintAnalysisOutput {
        findings,
        category_breakdown: breakdown,
        health_score,
        file_states,
    }
}

fn emit_failed_loop_finding(consecutive_errors: &[&ContextNode], findings: &mut Vec<LintFinding>) {
    let start_turn = consecutive_errors.first().map(|n| n.turn_seq).unwrap_or(1);
    let end_turn = consecutive_errors
        .last()
        .map(|n| n.turn_seq)
        .unwrap_or(start_turn);
    let wasted: u32 = consecutive_errors.iter().map(|n| n.token_span.count).sum();
    let ids: Vec<String> = consecutive_errors.iter().map(|n| n.id.clone()).collect();

    findings.push(LintFinding {
        rule_id: LintRuleId::FailedDebuggingLoop,
        severity: LintSeverity::Warning,
        title: format!(
            "Failed Debugging Loop ({} errors across turns {start_turn}..{end_turn})",
            consecutive_errors.len()
        ),
        message: format!(
            "{} consecutive failed tool executions consume {wasted} tokens and increase error imitation risk. Fold this subtree into a single outcome summary.",
            consecutive_errors.len()
        ),
        affected_node_ids: ids,
        wasted_tokens: wasted,
        suggested_mutation: Some(MutationOp::FoldSubtree {
            start_turn,
            end_turn,
            summary_text: format!(
                "Collapsed {} failed debugging steps across turns {start_turn}..{end_turn}.",
                consecutive_errors.len()
            ),
        }),
    });
}

fn check_file_modified_on_disk(
    path: &str,
    workspace_root: Option<&str>,
    last_read: Option<&FileReadEntry>,
    nodes: &[ContextNode],
) -> Option<String> {
    let &(node_idx, _, _, ref read_hash, read_range) = last_read?;
    let resolved = if Path::new(path).is_absolute() {
        std::path::PathBuf::from(path)
    } else if let Some(root) = workspace_root {
        Path::new(root).join(path)
    } else {
        return None;
    };

    let Ok(disk_content) = std::fs::read_to_string(&resolved) else {
        return None;
    };

    // 1. Check mtime against node timestamp if available
    if let Some(node) = nodes.get(node_idx)
        && let Some(ref ts_str) = node.timestamp
        && let Ok(read_dt) = chrono::DateTime::parse_from_rfc3339(ts_str)
        && let Ok(meta) = std::fs::metadata(&resolved)
        && let Ok(mtime) = meta.modified()
    {
        let mtime_dt: chrono::DateTime<chrono::Utc> = mtime.into();
        if mtime_dt
            .signed_duration_since(read_dt.with_timezone(&chrono::Utc))
            .num_seconds()
            > 2
        {
            // Confirm content actually differs before flagging
            let raw_node_text = node.kind.text_for_tokenization();
            if !disk_contains_read_snippet(&disk_content, &raw_node_text, read_range) {
                return Some("disk mtime & content changed externally".to_string());
            }
        }
    }

    // 2. Direct content hash / line-slice comparison
    if !read_hash.is_empty() {
        if let Some((start, end)) = read_range
            && (start > 1 || end < 800)
        {
            if let Some(node) = nodes.get(node_idx) {
                let raw_node_text = node.kind.text_for_tokenization();
                if !disk_contains_read_snippet(&disk_content, &raw_node_text, Some((start, end))) {
                    return Some(format!("lines {start}..{end} modified on disk"));
                }
            }
            return None;
        }

        let disk_hash = blake3::hash(disk_content.trim().as_bytes()).to_hex()[..16].to_string();
        if &disk_hash != read_hash {
            if let Some(node) = nodes.get(node_idx) {
                let raw_node_text = node.kind.text_for_tokenization();
                if disk_contains_read_snippet(&disk_content, &raw_node_text, read_range) {
                    return None;
                }
            }
            return Some("BLAKE3 hash differs from on-disk file".to_string());
        }
    }

    None
}

fn disk_contains_read_snippet(
    disk_content: &str,
    node_text: &str,
    read_range: Option<(u32, u32)>,
) -> bool {
    if node_text.starts_with("[cw:") {
        return true;
    }
    let disk_lines: Vec<&str> = disk_content.lines().collect();
    if let Some((start, end)) = read_range {
        let s_idx = (start.saturating_sub(1) as usize).min(disk_lines.len());
        let e_idx = (end as usize).min(disk_lines.len());
        if s_idx < e_idx {
            let slice_str = disk_lines[s_idx..e_idx].join("\n");
            let slice_hash = blake3::hash(slice_str.trim().as_bytes()).to_hex()[..16].to_string();
            let node_hash = blake3::hash(node_text.trim().as_bytes()).to_hex()[..16].to_string();
            if slice_hash == node_hash {
                return true;
            }
        }
    }

    // Strip optional line-number prefixes (e.g. `12: code` or `   12→code`) and check sample lines
    let mut checked = 0usize;
    let mut matched = 0usize;
    for line in node_text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("Created At:")
            || trimmed.starts_with("Completed At:")
            || trimmed.starts_with("Showing lines ")
            || trimmed.starts_with("Partial read ")
        {
            continue;
        }
        let code_part = if let Some((num_prefix, rest)) = trimmed.split_once(": ")
            && num_prefix.chars().all(|c| c.is_ascii_digit())
        {
            rest.trim()
        } else {
            trimmed
        };
        if code_part.len() < 8 {
            continue;
        }
        checked += 1;
        if disk_content.contains(code_part) {
            matched += 1;
        }
        if checked >= 12 {
            break;
        }
    }
    if checked == 0 {
        return disk_content.trim() == node_text.trim();
    }
    matched * 4 >= checked * 3
}
