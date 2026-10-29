use crate::ir::{
    CacheInvalidationCause, ContextNode, ContextTurn, KvCacheReport, KvNodeStatus, NodeKind,
    ProviderKind,
};
use crate::tokenizer::prefix_serialization_order;
use serde::{Deserialize, Serialize};

/// Analyzes KV Cache boundaries between `prev_nodes` ($T_{n-1}$) and `curr_nodes` ($T_n$),
/// updating `kv_status` on every node in `curr_nodes` and returning a detailed `KvCacheReport`.
pub fn analyze_kv_cache(
    prev_nodes: Option<&[ContextNode]>,
    curr_nodes: &mut [ContextNode],
    provider: ProviderKind,
    actual_cached_tokens: Option<u32>,
) -> KvCacheReport {
    let curr_order = prefix_serialization_order(curr_nodes, provider);
    let total_curr_tokens: u32 = curr_nodes.iter().map(|n| n.token_span.count).sum();

    let Some(prev) = prev_nodes else {
        // First turn of the session: everything is newly written to cache
        let hit = actual_cached_tokens.unwrap_or(0).min(total_curr_tokens);
        for &idx in &curr_order {
            curr_nodes[idx].kv_status = if curr_nodes[idx].token_span.end <= hit && hit > 0 {
                KvNodeStatus::CachedHit
            } else {
                KvNodeStatus::NewlyCached
            };
        }
        return KvCacheReport {
            hit_tokens: hit,
            newly_cached_tokens: total_curr_tokens.saturating_sub(hit),
            invalidated_tokens: 0,
            uncached_tail_tokens: 0,
            hit_ratio: if total_curr_tokens > 0 {
                hit as f32 / total_curr_tokens as f32
            } else {
                0.0
            },
            divergence_token_index: None,
            invalidation_cause: None,
        };
    };

    let prev_order = prefix_serialization_order(prev, provider);
    let total_prev_tokens: u32 = prev.iter().map(|n| n.token_span.count).sum();

    // 1. Find Longest Common Prefix (LCP) across ordered nodes
    let mut matched_prefix_tokens: u32 = 0;
    let mut divergence_node_idx: Option<usize> = None;

    let min_len = prev_order.len().min(curr_order.len());
    for step in 0..min_len {
        let p_node = &prev[prev_order[step]];
        let c_idx = curr_order[step];
        let c_node = &curr_nodes[c_idx];

        if p_node.role == c_node.role
            && p_node.kind.kind_tag() == c_node.kind.kind_tag()
            && p_node.kind.text_for_tokenization() == c_node.kind.text_for_tokenization()
        {
            matched_prefix_tokens = c_node.token_span.end;
        } else {
            // Compute sub-node character/token prefix overlap if text partially matches
            let p_text = p_node.kind.text_for_tokenization();
            let c_text = c_node.kind.text_for_tokenization();
            let common_bytes = p_text
                .bytes()
                .zip(c_text.bytes())
                .take_while(|(a, b)| a == b)
                .count();
            let ratio = if !c_text.is_empty() {
                common_bytes as f32 / c_text.len() as f32
            } else {
                0.0
            };
            let partial_tokens = (c_node.token_span.count as f32 * ratio) as u32;
            matched_prefix_tokens = c_node.token_span.start + partial_tokens;
            divergence_node_idx = Some(c_idx);
            break;
        }
    }

    // 2. Reconcile with provider telemetry if available
    let effective_hit_tokens = match actual_cached_tokens {
        Some(actual) if actual == 0 && matched_prefix_tokens >= 1024 => 0, // Cache TTL expired
        Some(actual) => actual.min(total_curr_tokens),
        None => 0,
    };

    let ttl_expired = actual_cached_tokens == Some(0) && matched_prefix_tokens >= 1024;
    let prefix_broke = matched_prefix_tokens < total_prev_tokens && divergence_node_idx.is_some();

    let mut hit_tokens: u32 = 0;
    let mut invalidated_tokens: u32 = 0;
    let mut newly_cached_tokens: u32 = 0;

    for &idx in &curr_order {
        let span = curr_nodes[idx].token_span;
        if span.end <= effective_hit_tokens && effective_hit_tokens > 0 {
            curr_nodes[idx].kv_status = KvNodeStatus::CachedHit;
            hit_tokens += span.count;
        } else if (prefix_broke || ttl_expired) && span.start < total_prev_tokens {
            curr_nodes[idx].kv_status = KvNodeStatus::CacheInvalidated;
            invalidated_tokens += span.count;
        } else {
            curr_nodes[idx].kv_status = KvNodeStatus::NewlyCached;
            newly_cached_tokens += span.count;
        }
    }

    let invalidation_cause = if ttl_expired && !prefix_broke {
        Some(CacheInvalidationCause::CacheTtlExpired {
            previous_tokens: total_prev_tokens,
        })
    } else if let Some(div_idx) = divergence_node_idx {
        let div_node = &curr_nodes[div_idx];
        let cause = match &div_node.kind {
            NodeKind::SystemPrompt { .. }
            | NodeKind::InjectedRule { .. }
            | NodeKind::SkillCatalog { .. } => CacheInvalidationCause::DynamicSystemInjection {
                node_id: div_node.id.clone(),
                divergence_token: matched_prefix_tokens,
                detail: format!(
                    "System/rule block '{}' mutated at token {} (invalidating {} subsequent tokens)",
                    div_node.kind.short_label(),
                    matched_prefix_tokens,
                    total_prev_tokens.saturating_sub(matched_prefix_tokens)
                ),
            },
            NodeKind::McpToolSchema { tool_name, .. } => {
                CacheInvalidationCause::ToolSchemaMutated {
                    node_id: div_node.id.clone(),
                    divergence_token: matched_prefix_tokens,
                    detail: format!(
                        "Tool schema '{}' changed or reordered at token {}",
                        tool_name, matched_prefix_tokens
                    ),
                }
            }
            _ => CacheInvalidationCause::HistoryEditedOrCompacted {
                node_id: div_node.id.clone(),
                divergence_token: matched_prefix_tokens,
                detail: format!(
                    "Historical node '{}' at turn {} diverged at token {}",
                    div_node.kind.short_label(),
                    div_node.turn_seq,
                    matched_prefix_tokens
                ),
            },
        };
        Some(cause)
    } else {
        None
    };

    KvCacheReport {
        hit_tokens,
        newly_cached_tokens,
        invalidated_tokens,
        uncached_tail_tokens: 0,
        hit_ratio: if total_curr_tokens > 0 {
            hit_tokens as f32 / total_curr_tokens as f32
        } else {
            0.0
        },
        divergence_token_index: if prefix_broke {
            Some(matched_prefix_tokens)
        } else {
            None
        },
        invalidation_cause,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RadixGraphNode {
    pub id: String,
    pub label: String,
    pub group: String,
    pub turn_seq: u32,
    pub tokens: u32,
    pub kv_status: KvNodeStatus,
    pub lint_badges: Vec<String>,
    pub node_id: Option<String>,
    pub raw_json_pointer: Option<String>,
    pub timestamp: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RadixGraphEdge {
    pub id: String,
    pub source: String,
    pub target: String,
    pub animated: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RadixGraph {
    pub nodes: Vec<RadixGraphNode>,
    pub edges: Vec<RadixGraphEdge>,
}

/// Builds an interactive Radix Prefix Tree / DAG for XYFlow visualization from the active turn
/// and any forked turns in the session.
pub fn build_radix_graph(latest_turn: &ContextTurn, all_turns: &[ContextTurn]) -> RadixGraph {
    let mut graph_nodes = Vec::new();
    let mut graph_edges = Vec::new();

    let root_id = "radix_root".to_string();
    graph_nodes.push(RadixGraphNode {
        id: root_id.clone(),
        label: format!(
            "Prefix Root ({} tok • {:.0}% KV Hit)",
            latest_turn.token_usage.input_tokens,
            latest_turn.kv_cache_report.hit_ratio * 100.0
        ),
        group: "root".to_string(),
        turn_seq: 0,
        tokens: latest_turn.token_usage.input_tokens,
        kv_status: KvNodeStatus::CachedHit,
        lint_badges: vec![],
        node_id: None,
        raw_json_pointer: None,
        timestamp: Some(latest_turn.created_at.to_rfc3339()),
    });

    // Aggregate MCP tool schemas into one collapsible group node so 50 tools don't clutter the canvas
    let mut mcp_tokens = 0u32;
    let mut mcp_count = 0u32;
    let mut mcp_kv = KvNodeStatus::CachedHit;
    for n in &latest_turn.nodes {
        if matches!(n.kind, NodeKind::McpToolSchema { .. }) {
            mcp_tokens += n.token_span.count;
            mcp_count += 1;
            if n.kv_status == KvNodeStatus::CacheInvalidated {
                mcp_kv = KvNodeStatus::CacheInvalidated;
            }
        }
    }

    let mut prev_spine_id = root_id.clone();
    if mcp_count > 0 {
        let mcp_id = "group_mcp_tools".to_string();
        graph_nodes.push(RadixGraphNode {
            id: mcp_id.clone(),
            label: format!("MCP & Native Tools ({mcp_count} tools • {mcp_tokens} tok)"),
            group: "tools".to_string(),
            turn_seq: 0,
            tokens: mcp_tokens,
            kv_status: mcp_kv,
            lint_badges: if mcp_tokens > 8000 {
                vec!["CW-L005: Tool Bloat".to_string()]
            } else {
                vec![]
            },
            node_id: None,
            raw_json_pointer: Some("/tools".to_string()),
            timestamp: None,
        });
        graph_edges.push(RadixGraphEdge {
            id: format!("e_{prev_spine_id}_{mcp_id}"),
            source: prev_spine_id,
            target: mcp_id.clone(),
            animated: false,
        });
        prev_spine_id = mcp_id;
    }

    // Add system, rule, skill, and conversation nodes
    for node in &latest_turn.nodes {
        if matches!(node.kind, NodeKind::McpToolSchema { .. }) {
            continue;
        }
        let mut badges = Vec::new();
        for lint in &latest_turn.lint_findings {
            if lint.affected_node_ids.contains(&node.id) {
                badges.push(lint.title.clone());
            }
        }
        if node.attention_salience < 0.35 {
            badges.push("Low Attention Zone".to_string());
        }

        let g_id = node.id.clone();
        graph_nodes.push(RadixGraphNode {
            id: g_id.clone(),
            label: format!(
                "{} ({} tok)",
                node.kind.short_label(),
                node.token_span.count
            ),
            group: node.kind.kind_tag().to_string(),
            turn_seq: node.turn_seq,
            tokens: node.token_span.count,
            kv_status: node.kv_status,
            lint_badges: badges,
            node_id: Some(node.id.clone()),
            raw_json_pointer: Some(node.raw_json_pointer.clone()),
            timestamp: node.timestamp.clone(),
        });
        graph_edges.push(RadixGraphEdge {
            id: format!("e_{prev_spine_id}_{g_id}"),
            source: prev_spine_id,
            target: g_id.clone(),
            animated: node.kv_status == KvNodeStatus::NewlyCached,
        });
        prev_spine_id = g_id;
    }

    // Add any forked replay branches from `all_turns`
    for turn in all_turns {
        if turn.id != latest_turn.id && turn.parent_turn_id.is_some() {
            let fork_id = format!("fork_{}", turn.id);
            graph_nodes.push(RadixGraphNode {
                id: fork_id.clone(),
                label: format!(
                    "Fork Turn #{} ({} tok • Health {})",
                    turn.sequence_number, turn.token_usage.input_tokens, turn.health_score
                ),
                group: "fork".to_string(),
                turn_seq: turn.sequence_number,
                tokens: turn.token_usage.input_tokens,
                kv_status: KvNodeStatus::NewlyCached,
                lint_badges: vec![],
                node_id: None,
                raw_json_pointer: None,
                timestamp: Some(turn.created_at.to_rfc3339()),
            });
            graph_edges.push(RadixGraphEdge {
                id: format!("e_root_{fork_id}"),
                source: root_id.clone(),
                target: fork_id,
                animated: true,
            });
        }
    }

    RadixGraph {
        nodes: graph_nodes,
        edges: graph_edges,
    }
}
