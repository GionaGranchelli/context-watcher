//! Multi-Agent Swarm Prefix Grouping & Radix Tree Router (Horizon 2 / CW-060).
//!
//! Analyzes multiple concurrent or related agent sessions working on the same workspace/project,
//! detects their Longest Common Prefix (LCP) of ordered root nodes (system prompt, rules, tool schemas),
//! and computes the shared KV Cache memory multiplexing benefits (1x prefix in GPU VRAM across M agents).

use crate::ir::{ContextNode, ContextTurn, NodeKind, Session};
use crate::radix::{RadixGraph, RadixGraphEdge, RadixGraphNode};
use crate::tokenizer::prefix_serialization_order;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Details of a single agent session belonging to a swarm prefix group.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwarmSessionMember {
    pub session_id: String,
    pub harness: String,
    pub model: String,
    pub total_tokens: u32,
    pub shared_prefix_tokens: u32,
    pub unique_tokens: u32,
    pub shared_ratio: f32,
}

/// Represents a clustered group of agent sessions that share a common prefix root in VRAM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwarmPrefixGroup {
    pub group_id: String,
    pub project_name: String,
    pub model: String,
    pub shared_prefix_tokens: u32,
    pub shared_system_tokens: u32,
    pub shared_tool_tokens: u32,
    pub member_count: usize,
    pub members: Vec<SwarmSessionMember>,
    pub total_unshared_tokens: u64,
    pub total_multiplexed_tokens: u64,
    pub tokens_saved: u64,
    pub vram_fp16_mb_saved: f32,
    pub vram_q8_mb_saved: f32,
    pub memory_reduction_pct: f32,
}

/// Computes the Longest Common Prefix (LCP) of nodes between two node slices.
fn find_common_prefix_tokens(
    a_nodes: &[ContextNode],
    a_order: &[usize],
    b_nodes: &[ContextNode],
    b_order: &[usize],
) -> u32 {
    let min_len = a_order.len().min(b_order.len());
    let mut matched_tokens = 0u32;

    for i in 0..min_len {
        let na = &a_nodes[a_order[i]];
        let nb = &b_nodes[b_order[i]];

        if na.kind.kind_tag() == nb.kind.kind_tag()
            && na.kind.text_for_tokenization() == nb.kind.text_for_tokenization()
        {
            matched_tokens = na.token_span.end.min(nb.token_span.end);
        } else {
            break;
        }
    }

    matched_tokens
}

/// Analyzes a collection of sessions and their latest turns, clustering them into SwarmPrefixGroups.
pub fn analyze_swarm_prefix_sharing(
    sessions_with_turns: &[(&Session, &ContextTurn)],
) -> Vec<SwarmPrefixGroup> {
    if sessions_with_turns.is_empty() {
        return Vec::new();
    }

    // 1. Group sessions by (project_workspace, model)
    let mut clusters: HashMap<(String, String), Vec<(&Session, &ContextTurn)>> = HashMap::new();
    for &(sess, turn) in sessions_with_turns {
        let ws = sess
            .workspace_root
            .as_deref()
            .map(|w| {
                w.trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or(w)
                    .to_string()
            })
            .unwrap_or_else(|| "default_workspace".to_string());
        clusters
            .entry((ws, sess.model.clone()))
            .or_default()
            .push((sess, turn));
    }

    let mut out_groups = Vec::new();

    for ((project, model), items) in clusters {
        if items.is_empty() {
            continue;
        }

        // Compute common prefix across all items in cluster
        let first_turn = items[0].1;
        let first_provider = items[0].0.provider;
        let first_order = prefix_serialization_order(&first_turn.nodes, first_provider);

        let mut shared_tokens = items[0].1.token_usage.input_tokens;
        for &(_s, turn) in items.iter().skip(1) {
            let order = prefix_serialization_order(&turn.nodes, first_provider);
            let common =
                find_common_prefix_tokens(&first_turn.nodes, &first_order, &turn.nodes, &order);
            shared_tokens = shared_tokens.min(common);
        }

        // Categorize shared system vs tool schema tokens
        let mut shared_sys = 0u32;
        let mut shared_tools = 0u32;
        for &idx in &first_order {
            let n = &first_turn.nodes[idx];
            if n.token_span.end <= shared_tokens {
                match n.kind {
                    NodeKind::SystemPrompt { .. } | NodeKind::InjectedRule { .. } => {
                        shared_sys += n.token_span.count;
                    }
                    NodeKind::McpToolSchema { .. } => {
                        shared_tools += n.token_span.count;
                    }
                    _ => {}
                }
            }
        }

        let mut members = Vec::new();
        let mut total_unshared: u64 = 0;
        let mut sum_unique: u64 = 0;

        for &(s, turn) in &items {
            let total = turn.token_usage.input_tokens;
            let unique = total.saturating_sub(shared_tokens);
            let ratio = if total > 0 {
                (shared_tokens as f32 / total as f32) * 100.0
            } else {
                0.0
            };

            total_unshared += total as u64;
            sum_unique += unique as u64;

            members.push(SwarmSessionMember {
                session_id: s.id.clone(),
                harness: s.harness.as_str().to_string(),
                model: s.model.clone(),
                total_tokens: total,
                shared_prefix_tokens: shared_tokens,
                unique_tokens: unique,
                shared_ratio: ratio,
            });
        }

        let total_multiplexed = (shared_tokens as u64) + sum_unique;
        let tokens_saved = total_unshared.saturating_sub(total_multiplexed);

        // Approximate local GPU VRAM estimation: (tokens * 2 bytes * 32 layers * 4096 hidden) / 1024^2 MB
        // INT8: 1 byte per token per parameter dimension
        let vram_fp16 = ((tokens_saved as f64) * 2.0 * 32.0 * 4096.0) / (1024.0 * 1024.0);
        let vram_q8 = ((tokens_saved as f64) * 1.0 * 32.0 * 4096.0) / (1024.0 * 1024.0);
        let red_pct = if total_unshared > 0 {
            (tokens_saved as f32 / total_unshared as f32) * 100.0
        } else {
            0.0
        };

        out_groups.push(SwarmPrefixGroup {
            group_id: format!(
                "swarm_{}_{}",
                project.to_lowercase(),
                &model.replace('/', "_")
            ),
            project_name: project,
            model,
            shared_prefix_tokens: shared_tokens,
            shared_system_tokens: shared_sys,
            shared_tool_tokens: shared_tools,
            member_count: items.len(),
            members,
            total_unshared_tokens: total_unshared,
            total_multiplexed_tokens: total_multiplexed,
            tokens_saved,
            vram_fp16_mb_saved: vram_fp16 as f32,
            vram_q8_mb_saved: vram_q8 as f32,
            memory_reduction_pct: red_pct,
        });
    }

    out_groups
}

/// Builds a unified cross-agent Radix DAG visual graph representing swarm prefix multiplexing.
pub fn build_swarm_radix_graph(swarm: &SwarmPrefixGroup) -> RadixGraph {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();

    let root_id = format!("swarm_root_{}", swarm.group_id);
    nodes.push(RadixGraphNode {
        id: root_id.clone(),
        label: format!(
            "Shared Swarm Trunk ({} tok • {} Agents)",
            swarm.shared_prefix_tokens, swarm.member_count
        ),
        group: "swarm_root".to_string(),
        turn_seq: 0,
        tokens: swarm.shared_prefix_tokens,
        kv_status: crate::ir::KvNodeStatus::CachedHit,
        lint_badges: vec!["Shared Swarm Trunk".to_string()],
        node_id: None,
        raw_json_pointer: None,
        timestamp: None,
    });

    for member in &swarm.members {
        let leaf_id = format!("agent_leaf_{}", member.session_id);
        nodes.push(RadixGraphNode {
            id: leaf_id.clone(),
            label: format!(
                "{} [{}] (+{} unique tok • {:.0}% shared)",
                member.harness, member.session_id, member.unique_tokens, member.shared_ratio
            ),
            group: "swarm_agent".to_string(),
            turn_seq: 1,
            tokens: member.unique_tokens,
            kv_status: crate::ir::KvNodeStatus::NewlyCached,
            lint_badges: vec![],
            node_id: Some(member.session_id.clone()),
            raw_json_pointer: None,
            timestamp: None,
        });

        edges.push(RadixGraphEdge {
            id: format!("e_{}_{}", root_id, leaf_id),
            source: root_id.clone(),
            target: leaf_id,
            animated: true,
        });
    }

    RadixGraph { nodes, edges }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyze_turn_from_payload;
    use crate::ir::{HarnessKind, ProviderKind, Session};
    use chrono::Utc;
    use serde_json::json;

    #[test]
    fn test_swarm_prefix_grouping_and_savings_calculation() {
        let s1 = Session {
            id: "agent_orchestrator".into(),
            parent_session_id: None,
            harness: HarnessKind::Antigravity,
            workspace_root: Some("/home/dev/my_project".into()),
            model: "qwen2.5-coder:32b".into(),
            provider: ProviderKind::OpenAI,
            title: Some("Lead".into()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let s2 = Session {
            id: "agent_coder".into(),
            parent_session_id: Some("agent_orchestrator".into()),
            harness: HarnessKind::Pi,
            workspace_root: Some("/home/dev/my_project".into()),
            model: "qwen2.5-coder:32b".into(),
            provider: ProviderKind::OpenAI,
            title: Some("Coder".into()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let s3 = Session {
            id: "agent_tester".into(),
            parent_session_id: Some("agent_orchestrator".into()),
            harness: HarnessKind::ClaudeCode,
            workspace_root: Some("/home/dev/my_project".into()),
            model: "qwen2.5-coder:32b".into(),
            provider: ProviderKind::OpenAI,
            title: Some("Tester".into()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let p1 = json!({
            "model": "qwen2.5-coder:32b",
            "messages": [
                { "role": "system", "content": "You are a professional coding assistant working on my_project.\n<RULE[AGENTS.md]>\nAlways write production tests.\n</RULE[AGENTS.md]>" },
                { "role": "user", "content": "Orchestrate feature implementation" }
            ]
        });

        let p2 = json!({
            "model": "qwen2.5-coder:32b",
            "messages": [
                { "role": "system", "content": "You are a professional coding assistant working on my_project.\n<RULE[AGENTS.md]>\nAlways write production tests.\n</RULE[AGENTS.md]>" },
                { "role": "user", "content": "Write the engine code in kv_engine.py" }
            ]
        });

        let p3 = json!({
            "model": "qwen2.5-coder:32b",
            "messages": [
                { "role": "system", "content": "You are a professional coding assistant working on my_project.\n<RULE[AGENTS.md]>\nAlways write production tests.\n</RULE[AGENTS.md]>" },
                { "role": "user", "content": "Run python unit tests on test_kv_engine.py" }
            ]
        });

        let (turn1, _) = analyze_turn_from_payload(
            "t1".into(),
            "agent_orchestrator".into(),
            None,
            1,
            &p1,
            ProviderKind::OpenAI,
            None,
            None,
            None,
        )
        .unwrap();
        let (turn2, _) = analyze_turn_from_payload(
            "t2".into(),
            "agent_coder".into(),
            None,
            1,
            &p2,
            ProviderKind::OpenAI,
            None,
            None,
            None,
        )
        .unwrap();
        let (turn3, _) = analyze_turn_from_payload(
            "t3".into(),
            "agent_tester".into(),
            None,
            1,
            &p3,
            ProviderKind::OpenAI,
            None,
            None,
            None,
        )
        .unwrap();

        let pairs = vec![(&s1, &turn1), (&s2, &turn2), (&s3, &turn3)];

        let groups = analyze_swarm_prefix_sharing(&pairs);
        assert_eq!(groups.len(), 1);
        let g = &groups[0];
        assert_eq!(g.project_name, "my_project");
        assert_eq!(g.member_count, 3);
        assert!(g.shared_prefix_tokens > 0);
        assert!(g.tokens_saved > 0);
        assert!(g.vram_fp16_mb_saved > 0.0);
        assert!(g.memory_reduction_pct > 0.0);

        let graph = build_swarm_radix_graph(g);
        assert_eq!(graph.nodes.len(), 4); // 1 trunk + 3 leaves
        assert_eq!(graph.edges.len(), 3);
    }
}
