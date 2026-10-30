use anyhow::Result;
use cw_core::{
    hud::{evaluate_attention_ecg, generate_context_hud_summary},
    ir::{ContextTurn, Session},
    local_llm::calculate_cost_and_perf,
    swarm::analyze_swarm_prefix_sharing,
};
use cw_store::CwStore;

use super::session::{project_label_from_workspace, resolve_session};

/// Generates and prints the token, KV-cache, and cost ($) savings scoreboard across all sessions (`cw gain`).
pub fn run_gain_report(store: &CwStore, show_history: bool) -> Result<()> {
    let sessions = store.list_sessions()?;

    if sessions.is_empty() {
        println!(
            "┌──────────────────────────────────────────────────────────────────────────────────┐"
        );
        println!(
            "│ contextWatcher (`cw gain`) — Token, KV Cache & Cost Savings Analytics           │"
        );
        println!(
            "├──────────────────────────────────────────────────────────────────────────────────┤"
        );
        println!(
            "│ No active coding sessions recorded on this machine yet.                          │"
        );
        println!(
            "│                                                                                  │"
        );
        println!(
            "│ 💡 contextWatcher is active and watching in the background!                      │"
        );
        println!(
            "│ As soon as you run any coding session with Antigravity, Claude Code, Codex,     │"
        );
        println!(
            "│ Pi / OMP, or Hermes, cw will automatically record your tokens and display your   │"
        );
        println!(
            "│ KV-cache hit ratio & dollar savings here.                                        │"
        );
        println!(
            "└──────────────────────────────────────────────────────────────────────────────────┘"
        );
        return Ok(());
    }

    #[derive(Default)]
    struct Bucket {
        sessions: usize,
        input_tokens: u64,
        saved_by_overlays: u64,
        cuttable_tokens: u64,
        kv_hit_tokens: u64,
        saved_usd: f64,
        cuttable_usd: f64,
        health_sum: u64,
    }

    let mut total = Bucket::default();
    let mut by_agent: std::collections::BTreeMap<String, Bucket> =
        std::collections::BTreeMap::new();
    let mut by_project: std::collections::BTreeMap<String, Bucket> =
        std::collections::BTreeMap::new();
    let mut recent_mutations = Vec::new();

    for s in &sessions {
        let Some(turn) = store.get_latest_turn(&s.id)? else {
            continue;
        };
        let plans = store.optimization_plans(&turn)?;
        let max_cut = plans
            .iter()
            .find(|p| p.id == "max_token_diet")
            .map(|p| p.saved_tokens)
            .unwrap_or(0);
        let cost = calculate_cost_and_perf(&turn, &s.model, s.provider);
        let cut_usd = (max_cut as f64 / 1_000_000.0) * 3.0; // Conservative $3/1M base input estimate

        let agent_key = s.harness.as_str().to_string();
        let proj_key = project_label_from_workspace(s.workspace_root.as_deref());

        for b in [
            &mut total,
            by_agent.entry(agent_key).or_default(),
            by_project.entry(proj_key).or_default(),
        ] {
            b.sessions += 1;
            b.input_tokens += turn.token_usage.input_tokens as u64;
            b.saved_by_overlays += turn.token_usage.saved_by_mutations as u64;
            b.cuttable_tokens += max_cut as u64;
            b.kv_hit_tokens += turn.kv_cache_report.hit_tokens as u64;
            b.saved_usd += cost.saved_by_kv_cache_usd + cost.saved_by_cw_overlays_usd;
            b.cuttable_usd += cut_usd;
            b.health_sum += turn.health_score as u64;
        }

        if show_history && let Ok(muts) = store.list_mutations(&s.id) {
            for m in muts {
                recent_mutations.push((s.id.clone(), s.harness.as_str().to_string(), m));
            }
        }
    }

    let avg_health = if total.sessions > 0 {
        total.health_sum / total.sessions as u64
    } else {
        100
    };
    let kv_ratio = if total.input_tokens > 0 {
        ((total.kv_hit_tokens as f64 / total.input_tokens as f64) * 100.0).clamp(0.0, 99.9)
    } else {
        0.0
    };

    println!(
        "┌──────────────────────────────────────────────────────────────────────────────────┐"
    );
    println!("│ contextWatcher (`cw gain`) — Token, KV Cache & Cost Savings Analytics           │");
    println!(
        "├──────────────────────────────────────────────────────────────────────────────────┤"
    );
    println!(
        "│ Sessions Analyzed   : {:<10} │ Active Context Volume : {:>12} tok     │",
        total.sessions, total.input_tokens
    );
    println!(
        "│ Active Overlay Cut  : {:<10} │ Ready-to-Cut Bloat    : {:>12} tok     │",
        format!("-{} tok", total.saved_by_overlays),
        format!("-{}", total.cuttable_tokens)
    );
    println!(
        "│ KV Cache Hit Ratio  : {:<9.1}% │ Avg Context Health    : {:>12}/100     │",
        kv_ratio, avg_health
    );
    println!(
        "│ Active Saved ($)    : ${:<9.2} │ Cuttable Potential ($) : ${:>11.2}/turn   │",
        total.saved_usd, total.cuttable_usd
    );
    println!(
        "└──────────────────────────────────────────────────────────────────────────────────┘"
    );

    println!("\nBY AGENT HARNESS:");
    println!(
        "  {:<16} {:>8} {:>14} {:>14} {:>14} {:>10}",
        "AGENT", "SESSIONS", "CONTEXT TOK", "ACTIVE SAVED", "CUTTABLE BLOAT", "AVG HEALTH"
    );
    println!("  {}", "─".repeat(78));
    for (agent, b) in &by_agent {
        let h = if b.sessions > 0 {
            b.health_sum / b.sessions as u64
        } else {
            100
        };
        println!(
            "  {:<16} {:>8} {:>14} {:>14} {:>14} {:>9}/100",
            agent,
            b.sessions,
            b.input_tokens,
            format!("-{}", b.saved_by_overlays),
            format!("-{}", b.cuttable_tokens),
            h
        );
    }

    let mut proj_vec: Vec<_> = by_project.into_iter().collect();
    proj_vec.sort_by_key(|b| std::cmp::Reverse(b.1.input_tokens));

    println!("\nTOP PROJECTS BY CONTEXT VOLUME:");
    println!(
        "  {:<22} {:>8} {:>14} {:>14} {:>10}",
        "PROJECT", "SESSIONS", "CONTEXT TOK", "CUTTABLE BLOAT", "AVG HEALTH"
    );
    println!("  {}", "─".repeat(72));
    for (proj, b) in proj_vec.into_iter().take(12) {
        let h = if b.sessions > 0 {
            b.health_sum / b.sessions as u64
        } else {
            100
        };
        let p_short: String = proj.chars().take(21).collect();
        println!(
            "  {:<22} {:>8} {:>14} {:>14} {:>9}/100",
            p_short,
            b.sessions,
            b.input_tokens,
            format!("-{}", b.cuttable_tokens),
            h
        );
    }

    if show_history && !recent_mutations.is_empty() {
        println!(
            "\nACTIVE MUTATION OVERLAY LEDGER ({} rules):",
            recent_mutations.len()
        );
        for (sid, h, m) in recent_mutations.into_iter().take(20) {
            println!(
                "  • [{}] {:<12} {:<24} {:?} ({:?})",
                m.id, h, sid, m.status, m.op
            );
        }
    }

    println!("\nTip: Run `cw optimize -s max_token_diet` or `cw sync-back` to apply compactions.");
    Ok(())
}

/// Generates and prints the multi-agent swarm prefix sharing analysis (`cw swarm`).
pub fn run_swarm_report(
    store: &CwStore,
    project_filter: Option<String>,
    model_filter: Option<String>,
    min_members: usize,
    output_json: bool,
) -> Result<()> {
    let sessions = store.list_sessions()?;
    if sessions.is_empty() {
        if output_json {
            println!("[]");
        } else {
            println!(
                "No sessions recorded yet. Start agent sessions through `cw proxy` or run `cw serve`."
            );
        }
        return Ok(());
    }

    let mut sessions_with_turns = Vec::new();
    for s in &sessions {
        if let Some(ref proj) = project_filter {
            let ws = project_label_from_workspace(s.workspace_root.as_deref());
            if !ws.to_lowercase().contains(&proj.to_lowercase()) {
                continue;
            }
        }
        if let Some(ref mod_filter) = model_filter
            && !s.model.to_lowercase().contains(&mod_filter.to_lowercase())
        {
            continue;
        }

        if let Some(turn) = store.get_latest_turn(&s.id)? {
            sessions_with_turns.push((s, turn));
        }
    }

    let items: Vec<(&Session, &ContextTurn)> =
        sessions_with_turns.iter().map(|(s, t)| (*s, t)).collect();

    let mut swarms = analyze_swarm_prefix_sharing(&items);
    if min_members > 1 {
        swarms.retain(|s| s.member_count >= min_members);
    }

    if output_json {
        println!("{}", serde_json::to_string_pretty(&swarms)?);
        return Ok(());
    }

    if swarms.is_empty() {
        println!("\n🔍 No multi-agent swarm clusters found matching filters.");
        println!(
            "💡 Tip: Multi-agent swarms occur when 2 or more subagents operate on the same workspace project using the same model."
        );
        return Ok(());
    }

    println!("\n🐝 contextWatcher Multi-Agent Swarm Prefix Multiplexing");
    println!(
        "════════════════════════════════════════════════════════════════════════════════════"
    );

    let mut total_saved_tokens: u64 = 0;
    let mut total_vram_saved_mb: f32 = 0.0;
    let mut total_members: usize = 0;

    for swarm in &swarms {
        total_saved_tokens += swarm.tokens_saved;
        total_vram_saved_mb += swarm.vram_fp16_mb_saved;
        total_members += swarm.member_count;

        println!("Cluster: {} | Model: {}", swarm.project_name, swarm.model);
        println!(
            "  Shared Prefix Trunk: {} tokens (System: {} tok • MCP/Tools: {} tok)",
            swarm.shared_prefix_tokens, swarm.shared_system_tokens, swarm.shared_tool_tokens
        );
        println!(
            "  Active Swarm Members: {} agents | Memory Reduction: {:.1}%",
            swarm.member_count, swarm.memory_reduction_pct
        );
        println!(
            "  Prefix Tokens Multiplexed: {} unshared -> {} shared (Saved: {} tokens)",
            swarm.total_unshared_tokens, swarm.total_multiplexed_tokens, swarm.tokens_saved
        );
        println!(
            "  Est. Local GPU VRAM Saved: {:.1} MB (FP16) / {:.1} MB (Q8)",
            swarm.vram_fp16_mb_saved, swarm.vram_q8_mb_saved
        );
        println!("  Member Breakdown:");
        for m in &swarm.members {
            println!(
                "    • [{}] {:<16} {:>6} tok ({:.1}% shared trunk, +{} unique tail tokens)",
                m.session_id, m.harness, m.total_tokens, m.shared_ratio, m.unique_tokens
            );
        }
        println!(
            "────────────────────────────────────────────────────────────────────────────────"
        );
    }

    println!(
        "Summary: {} Swarm Cluster(s) | {} Active Agents | {} Shared Tokens Saved | ~{:.1} MB VRAM Preserved",
        swarms.len(),
        total_members,
        total_saved_tokens,
        total_vram_saved_mb
    );
    println!(
        "💡 With Swarm Radix routing, local inference engines (llama.cpp/vLLM) only keep 1 copy of the shared prefix trunk in VRAM!\n"
    );

    Ok(())
}

/// Generates and prints the Ambient Context HUD and Attention ECG report (`cw hud`).
pub fn run_hud_report(
    store: &CwStore,
    session_target: Option<String>,
    compact: bool,
    show_ecg: bool,
    output_json: bool,
) -> Result<()> {
    let session_id = resolve_session(store, session_target)?;
    let Some(sid) = session_id else {
        if output_json {
            println!("null");
        } else {
            println!("No active session found.");
        }
        return Ok(());
    };

    let session_obj = store.get_session(&sid)?;
    let latest_turn = store.get_latest_turn(&sid)?;

    let plans = latest_turn
        .as_ref()
        .map(|turn| store.optimization_plans(turn))
        .transpose()?
        .unwrap_or_default();
    let cuttable = plans
        .iter()
        .find(|p| p.id == "max_token_diet")
        .map(|p| p.saved_tokens)
        .unwrap_or(0);

    let harness_str = session_obj
        .as_ref()
        .map(|s| s.harness.as_str())
        .unwrap_or("agent");
    let model_str = session_obj
        .as_ref()
        .map(|s| s.model.as_str())
        .unwrap_or("model");

    let summary =
        generate_context_hud_summary(&sid, harness_str, model_str, latest_turn.as_ref(), cuttable);

    let ecg = latest_turn.as_ref().map(evaluate_attention_ecg);

    if output_json {
        let out = serde_json::json!({
            "summary": summary,
            "ecg": ecg,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    if show_ecg {
        println!("\n💓 contextWatcher Positional Attention ECG (Lost-in-the-Middle Salience)");
        println!(
            "════════════════════════════════════════════════════════════════════════════════════"
        );
        println!(
            "Session       : {} ({} • {})",
            summary.session_id, summary.harness, summary.model
        );
        println!("Statusline    : {}", summary.formatted_oneliner);
        if let Some(e) = ecg {
            println!(
                "Attention ECG : Primacy: {}t | Trough (Dead Zone): {}t | Recency: {}t",
                e.primacy_tokens, e.trough_tokens, e.recency_tokens
            );
            println!(
                "Average Score : {:.1}% Attention Across Context",
                e.avg_attention_pct
            );
            println!(
                "Risk Nodes    : {} critical instruction(s) sinking in Lost-in-the-Middle trough",
                e.critical_nodes_in_trough
            );
            println!(
                "────────────────────────────────────────────────────────────────────────────────"
            );
            println!(
                "  {:<6} {:<24} {:>10} {:>12}  {:<14}  {:<8}",
                "INDEX", "NODE KIND", "SPAN", "SALIENT %", "ZONE", "STATUS"
            );
            println!("  {}", "─".repeat(80));
            for (idx, n) in e.node_breakdown.iter().enumerate() {
                let status_str = if n.at_risk {
                    "⚠️ AT RISK"
                } else {
                    "✅ Stable"
                };
                let zone_str = match n.zone {
                    cw_core::hud::AttentionZone::Primacy => "Primacy (Top)",
                    cw_core::hud::AttentionZone::Trough => "Trough (Mid)",
                    cw_core::hud::AttentionZone::Recency => "Recency (Tail)",
                };
                let span_str = format!("{}..{}", n.token_start, n.token_end);
                println!(
                    "  #{:<5} {:<24} {:>10} {:>11.1}%  {:<14}  {:<8}",
                    idx, n.label, span_str, n.attention_score_pct, zone_str, status_str
                );
            }
        }
        println!(
            "\n💡 Tip: Context Pinning (`cw pin`) injects critical instructions at the high-attention Recency tail!\n"
        );
        return Ok(());
    }

    if compact {
        println!("{}", summary.compact_oneliner);
    } else {
        println!("{}", summary.formatted_oneliner);
    }

    Ok(())
}
