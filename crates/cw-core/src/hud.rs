//! Ambient HUD & Positional Attention ECG (Horizon 4 / CW-080).
//!
//! Provides ultra-low-latency (<1ms) statusline telemetry for terminal prompts (Starship, tmux, zellij)
//! and IDE status bars, alongside empirical "Lost-in-the-Middle" positional attention ECG curves.

use crate::ir::{ContextTurn, NodeKind};
use serde::{Deserialize, Serialize};

/// Attention salience tier for a node in the prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttentionZone {
    /// High attention primacy zone: beginning of prompt (tokens 0% to 20%).
    Primacy,
    /// Attenuated attention trough ("Lost-in-the-Middle"): tokens 20% to 80%.
    Trough,
    /// High attention recency zone: tail of prompt (tokens 80% to 100%).
    Recency,
}

/// Node-level attention ECG telemetry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeAttentionEcg {
    pub node_id: String,
    pub label: String,
    pub token_start: u32,
    pub token_end: u32,
    pub token_count: u32,
    pub attention_score_pct: f32,
    pub zone: AttentionZone,
    pub at_risk: bool,
}

/// Positional Attention ECG curve across the entire context window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextAttentionEcg {
    pub total_tokens: u32,
    pub primacy_tokens: u32,
    pub trough_tokens: u32,
    pub recency_tokens: u32,
    pub avg_attention_pct: f32,
    pub critical_nodes_in_trough: usize,
    pub node_breakdown: Vec<NodeAttentionEcg>,
}

/// Instantaneous Ambient Context HUD statusline summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextHudSummary {
    pub session_id: String,
    pub harness: String,
    pub model: String,
    pub total_tokens: u32,
    pub context_limit: u32,
    pub used_capacity_pct: f32,
    pub kv_cache_hit_ratio_pct: f32,
    pub active_overlay_savings_tokens: u32,
    pub cuttable_bloat_tokens: u32,
    pub health_score: u8,
    pub health_indicator: String, // "🟢" | "🟡" | "🔴"
    pub formatted_oneliner: String,
    pub compact_oneliner: String,
}

/// Calculates the empirical U-shaped "Lost-in-the-Middle" attention score (0.0 to 100.0)
/// based on the relative position `(start + end) / (2 * total_tokens)` in the prompt.
pub fn calculate_attention_salience(
    mid_point_tokens: u32,
    total_tokens: u32,
) -> (f32, AttentionZone) {
    if total_tokens == 0 {
        return (100.0, AttentionZone::Primacy);
    }

    let rel_pos = (mid_point_tokens as f32 / total_tokens as f32).clamp(0.0, 1.0);

    // Bathtub U-curve formula: High at 0 (95%), dips to ~40% at center 0.5, rises to 98% at tail 1.0
    // Salience(x) = 40.0 + 240.0 * (x - 0.5)^2
    let salience = (40.0 + 240.0 * (rel_pos - 0.5) * (rel_pos - 0.5)).clamp(35.0, 100.0);

    let zone = if rel_pos < 0.20 {
        AttentionZone::Primacy
    } else if rel_pos > 0.80 {
        AttentionZone::Recency
    } else {
        AttentionZone::Trough
    };

    (salience, zone)
}

/// Evaluates the complete Positional Attention ECG curve for a context turn.
pub fn evaluate_attention_ecg(turn: &ContextTurn) -> ContextAttentionEcg {
    let total_tokens = turn.token_usage.input_tokens.max(1);
    let mut node_breakdown = Vec::new();

    let mut primacy_tokens = 0u32;
    let mut trough_tokens = 0u32;
    let mut recency_tokens = 0u32;
    let mut salience_sum = 0.0f32;
    let mut critical_in_trough = 0usize;

    for node in &turn.nodes {
        let mid = (node.token_span.start + node.token_span.end) / 2;
        let (salience, zone) = calculate_attention_salience(mid, total_tokens);
        salience_sum += salience * (node.token_span.count as f32);

        match zone {
            AttentionZone::Primacy => primacy_tokens += node.token_span.count,
            AttentionZone::Trough => trough_tokens += node.token_span.count,
            AttentionZone::Recency => recency_tokens += node.token_span.count,
        }

        // Critical rules (SystemPrompt, InjectedRule, McpToolSchema) are at risk if trapped in the trough!
        let is_critical_kind = matches!(
            node.kind,
            NodeKind::SystemPrompt { .. }
                | NodeKind::InjectedRule { .. }
                | NodeKind::McpToolSchema { .. }
        );
        let at_risk = is_critical_kind && zone == AttentionZone::Trough && salience < 50.0;
        if at_risk {
            critical_in_trough += 1;
        }

        node_breakdown.push(NodeAttentionEcg {
            node_id: node.id.clone(),
            label: node.kind.kind_tag().to_string(),
            token_start: node.token_span.start,
            token_end: node.token_span.end,
            token_count: node.token_span.count,
            attention_score_pct: salience,
            zone,
            at_risk,
        });
    }

    let avg_attention = (salience_sum / total_tokens as f32).clamp(0.0, 100.0);

    ContextAttentionEcg {
        total_tokens,
        primacy_tokens,
        trough_tokens,
        recency_tokens,
        avg_attention_pct: avg_attention,
        critical_nodes_in_trough: critical_in_trough,
        node_breakdown,
    }
}

/// Generates a fast, zero-latency Ambient Context HUD statusline summary for a session.
pub fn generate_context_hud_summary(
    session_id: &str,
    harness: &str,
    model: &str,
    turn: Option<&ContextTurn>,
    cuttable_bloat: u32,
) -> ContextHudSummary {
    let (total_tokens, limit, kv_hit, saved_by_muts, health) = if let Some(t) = turn {
        (
            t.token_usage.input_tokens,
            if t.token_usage.context_limit > 0 {
                t.token_usage.context_limit
            } else {
                131_072
            },
            t.kv_cache_report.hit_ratio * 100.0,
            t.token_usage.saved_by_mutations,
            t.health_score,
        )
    } else {
        (0, 131_072, 0.0, 0, 100)
    };

    let used_cap = ((total_tokens as f32 / limit.max(1) as f32) * 100.0).min(100.0);

    let indicator = if health >= 80 {
        "🟢"
    } else if health >= 60 {
        "🟡"
    } else {
        "🔴"
    };

    let tok_str = if total_tokens >= 1_000_000 {
        format!("{:.1}M", total_tokens as f32 / 1_000_000.0)
    } else if total_tokens >= 1_000 {
        format!("{:.1}k", total_tokens as f32 / 1_000.0)
    } else {
        format!("{}", total_tokens)
    };

    let formatted = format!(
        "{} {} tok | {:.1}% KV | -{} tok diet | {}/100 Health",
        indicator,
        tok_str,
        kv_hit,
        saved_by_muts + cuttable_bloat,
        health
    );

    let compact = format!(
        "{} {} [{:.0}% KV] (-{}t) H:{}",
        indicator,
        tok_str,
        kv_hit,
        saved_by_muts + cuttable_bloat,
        health
    );

    ContextHudSummary {
        session_id: session_id.to_string(),
        harness: harness.to_string(),
        model: model.to_string(),
        total_tokens,
        context_limit: limit,
        used_capacity_pct: used_cap,
        kv_cache_hit_ratio_pct: kv_hit,
        active_overlay_savings_tokens: saved_by_muts,
        cuttable_bloat_tokens: cuttable_bloat,
        health_score: health,
        health_indicator: indicator.to_string(),
        formatted_oneliner: formatted,
        compact_oneliner: compact,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_attention_salience_u_curve() {
        // Position 0.0 (Primacy)
        let (s_head, z_head) = calculate_attention_salience(0, 1000);
        assert!(s_head >= 95.0);
        assert_eq!(z_head, AttentionZone::Primacy);

        // Position 0.5 (Trough)
        let (s_mid, z_mid) = calculate_attention_salience(500, 1000);
        assert!(s_mid <= 45.0);
        assert_eq!(z_mid, AttentionZone::Trough);

        // Position 1.0 (Recency)
        let (s_tail, z_tail) = calculate_attention_salience(1000, 1000);
        assert!(s_tail >= 95.0);
        assert_eq!(z_tail, AttentionZone::Recency);
    }

    #[test]
    fn test_context_hud_summary_formatting() {
        let hud = generate_context_hud_summary("sess_lead", "omp", "qwen3.8", None, 250);
        assert_eq!(hud.health_score, 100);
        assert!(hud.formatted_oneliner.contains("🟢"));
        assert!(hud.formatted_oneliner.contains("100/100 Health"));
    }
}
