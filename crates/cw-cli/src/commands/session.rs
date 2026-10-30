use std::path::PathBuf;

use anyhow::Result;
use cw_core::{
    ir::{HarnessKind, KvNodeStatus, MutationOp, MutationRule, MutationStatus},
    local_llm::{analyze_local_model_kv, calculate_cost_and_perf},
    radix::build_radix_graph,
};
use cw_harnesses::{
    import_agy_transcript, import_generic_jsonl_transcript, import_hermes_state_db,
    import_opencode_state_db, sync_mutations_to_transcript_on_disk,
};
use cw_proxy::recompute_latest_turn_in_store;
use cw_store::CwStore;
use uuid::Uuid;

use super::reports::project_label_from_workspace;

/// Resolves a target session ID from explicit flag, environment variable, current workspace directory,
/// or prefix match.
pub fn resolve_session(store: &CwStore, explicit: Option<String>) -> Result<Option<String>> {
    let selected = explicit
        .or_else(|| std::env::var("CW_SESSION_ID").ok())
        .filter(|s| !s.trim().is_empty());
    if let Some(id) = selected {
        if store.get_session(&id)?.is_some() {
            return Ok(Some(id));
        }
        let prefix_matches: Vec<_> = store
            .list_sessions()?
            .into_iter()
            .filter(|s| s.id.starts_with(&id))
            .collect();
        return match prefix_matches.as_slice() {
            [session] => Ok(Some(session.id.clone())),
            [] => anyhow::bail!("Session not found: {id}"),
            _ => anyhow::bail!("Several sessions match prefix '{id}'. Pass the full --session ID."),
        };
    }
    let cwd = std::env::current_dir()?;
    let sessions = store.list_sessions()?;
    if let Some(matching) = sessions.iter().find(|s| {
        s.workspace_root
            .as_deref()
            .is_some_and(|root| std::path::Path::new(root) == cwd)
    }) {
        return Ok(Some(matching.id.clone()));
    }
    Ok(sessions.first().map(|s| s.id.clone()))
}

/// Persists an interactive CLI mutation to the session overlay store.
pub fn save_cli_mutation(
    store: &CwStore,
    session_id: &str,
    op: MutationOp,
    draft: bool,
) -> Result<()> {
    let rule = MutationRule {
        id: format!("mut_{}", &Uuid::new_v4().to_string()[..8]),
        session_id: session_id.to_string(),
        op,
        status: if draft {
            MutationStatus::Draft
        } else {
            MutationStatus::Committed
        },
        estimated_saved_tokens: 0,
        estimated_kv_break_tokens: 0,
        created_at: chrono::Utc::now(),
    };
    store.save_mutation(&rule)?;
    Ok(())
}

/// Displays active session health, token usage, KV cache status, and cost breakdown (`cw status`).
pub fn run_status(
    store: &CwStore,
    session: Option<String>,
    db_path: &std::path::Path,
) -> Result<()> {
    let Some(sess_id) = resolve_session(store, session)? else {
        println!("No active sessions found in {}", db_path.display());
        return Ok(());
    };
    let Some(sess) = store.get_session(&sess_id)? else {
        println!("Session {} not found in database.", sess_id);
        return Ok(());
    };
    let Some(turn) = store.get_latest_turn(&sess_id)? else {
        println!("Session {} has no turns.", sess.id);
        return Ok(());
    };
    let mutations = store.list_mutations(&sess_id)?;
    if let Ok(ap_cfg) = store.get_autopilot_config()
        && ap_cfg.safety_version < 2
    {
        println!(
            "⚠️  Migration Notice: Legacy Auto-Pilot configuration (safety_version < 2) was disabled during the safety upgrade. Run `cw autopilot --enable` to re-enable automatic background optimization."
        );
    }

    println!(
        "Session      : {} ({} • {})",
        sess.id,
        sess.harness.as_str(),
        sess.model
    );
    println!(
        "Turn         : #{} ({} nodes)",
        turn.sequence_number,
        turn.nodes.len()
    );
    println!(
        "Tokens       : {} / {} ({:.1}%) [Saved by overlays: {} tok]",
        turn.token_usage.input_tokens,
        turn.token_usage.context_limit,
        (turn.token_usage.input_tokens as f64 / turn.token_usage.context_limit.max(1) as f64)
            * 100.0,
        turn.token_usage.saved_by_mutations
    );
    println!(
        "KV Cache     : {:.1}% Hit ({} hit | {} new | {} invalidated)",
        turn.kv_cache_report.hit_ratio * 100.0,
        turn.kv_cache_report.hit_tokens,
        turn.kv_cache_report.newly_cached_tokens,
        turn.kv_cache_report.invalidated_tokens
    );
    println!(
        "Health Score : {}/100 ({} lints, {} active overlays)",
        turn.health_score,
        turn.lint_findings.len(),
        mutations.len()
    );
    let cost = calculate_cost_and_perf(&turn, &sess.model, sess.provider);
    if cost.is_local_model {
        let loc = analyze_local_model_kv(&turn, &sess.id, &sess.model, sess.provider);
        println!(
            "Local KV VRAM: {:.0} MB (FP16) | {:.0} MB (Q8_0) | {:.0} MB (Q4_0) | Recommended n_keep/num_keep: {} tok | Slot #{}",
            loc.vram_fp16_mb,
            loc.vram_q8_0_mb,
            loc.vram_q4_0_mb,
            loc.recommended_n_keep_tokens,
            loc.recommended_slot_id
        );
    } else {
        println!(
            "Turn Cost ($): ${:.4} [Saved ${:.4} via KV Cache + ${:.4} via cw Overlays] ({})",
            cost.turn_cost_usd,
            cost.saved_by_kv_cache_usd,
            cost.saved_by_cw_overlays_usd,
            cost.model_family
        );
    }
    Ok(())
}

/// Displays the context Radix tree and KV cache status per block (`cw tree`).
pub fn run_tree(store: &CwStore, session: Option<String>) -> Result<()> {
    let Some(sess_id) = resolve_session(store, session)? else {
        println!("No active sessions found.");
        return Ok(());
    };
    let turns = store.list_turns(&sess_id)?;
    let Some(latest) = turns.last() else {
        println!("No turns recorded.");
        return Ok(());
    };
    let graph = build_radix_graph(latest, &turns);
    println!("Context Radix Tree — Session {sess_id}:");
    for (idx, gnode) in graph.nodes.iter().enumerate() {
        let branch = if idx + 1 == graph.nodes.len() {
            "└──"
        } else {
            "├──"
        };
        let kv = match gnode.kv_status {
            KvNodeStatus::CachedHit => "🟢 HIT ",
            KvNodeStatus::NewlyCached => "🟡 NEW ",
            KvNodeStatus::CacheInvalidated => "🔴 BRK ",
            KvNodeStatus::Uncached => "⚪ TAIL",
            KvNodeStatus::Unknown => "🟣 UNK ",
        };
        let badges = if gnode.lint_badges.is_empty() {
            String::new()
        } else {
            format!(" ⚠️ [{}]", gnode.lint_badges.join(", "))
        };
        println!("  {branch} [{kv}] {}{badges}", gnode.label);
    }
    Ok(())
}

/// Analyzes lint findings and optionally applies suggested fixes (`cw lint`).
pub fn run_lint(store: &CwStore, session: Option<String>, fix: bool) -> Result<()> {
    let Some(sess_id) = resolve_session(store, session)? else {
        println!("No active sessions found.");
        return Ok(());
    };
    let Some(turn) = store.get_latest_turn(&sess_id)? else {
        println!("No turns recorded.");
        return Ok(());
    };
    println!(
        "Context Health Score: {}/100 ({} findings)",
        turn.health_score,
        turn.lint_findings.len()
    );
    for f in &turn.lint_findings {
        println!(
            "  • [{:?}] {:?} — {} (wasted: {} tok)\n    {}",
            f.severity, f.rule_id, f.title, f.wasted_tokens, f.message
        );
        if fix && let Some(op) = &f.suggested_mutation {
            save_cli_mutation(store, &sess_id, op.clone(), false)?;
            println!("    ✓ Applied suggested fix: {:?}", op);
        }
    }
    Ok(())
}

/// Applies or previews optimization plans for a single session or batch of sessions (`cw optimize`).
#[allow(clippy::too_many_arguments)]
pub fn run_optimize(
    store: &CwStore,
    level: Option<String>,
    strategy: Option<String>,
    dry_run: bool,
    explain: bool,
    draft: bool,
    session: Option<String>,
    all: bool,
    project: Option<String>,
    agent: Option<String>,
    sync_back: bool,
) -> Result<()> {
    let raw_strategy = level.or(strategy);
    let strategy = raw_strategy
        .as_deref()
        .map(cw_core::optimizer::resolve_strategy_alias)
        .map(|s| s.to_string());
    let is_batch = all || project.is_some() || agent.is_some();

    if is_batch {
        let strat_id = strategy
            .clone()
            .unwrap_or_else(|| "max_token_diet".to_string());
        let all_sessions = store.list_sessions()?;
        let target_sessions: Vec<_> = all_sessions
            .into_iter()
            .filter(|s| {
                if let Some(ref p_filter) = project {
                    let proj = project_label_from_workspace(s.workspace_root.as_deref());
                    if !proj.eq_ignore_ascii_case(p_filter)
                        && !s
                            .workspace_root
                            .as_deref()
                            .unwrap_or("")
                            .to_lowercase()
                            .contains(&p_filter.to_lowercase())
                    {
                        return false;
                    }
                }
                if let Some(ref a_filter) = agent
                    && !s.harness.as_str().eq_ignore_ascii_case(a_filter)
                {
                    return false;
                }
                true
            })
            .collect();

        if target_sessions.is_empty() {
            println!("No matching sessions found for the specified filter.");
            return Ok(());
        }

        let mut sessions_compacted = 0usize;
        let mut total_ops = 0usize;
        let mut total_saved_tok = 0u64;
        let mut total_synced = 0usize;

        for s in &target_sessions {
            let Some(turn) = store.get_latest_turn(&s.id)? else {
                continue;
            };
            let plans = store.optimization_plans(&turn)?;
            let Some(plan) = plans.iter().find(|p| p.id == strat_id) else {
                anyhow::bail!(
                    "Unknown strategy '{}'. Valid IDs: cache_safe_boost, clean_code_state, surgical_log_diet, deep_trajectory_fold, max_token_diet",
                    strat_id
                );
            };
            if plan.items.is_empty() {
                continue;
            }
            sessions_compacted += 1;
            total_ops += plan.items.len();
            total_saved_tok += plan.saved_tokens as u64;

            if !dry_run {
                for item in &plan.items {
                    let rule = MutationRule {
                        id: format!("mut_{}", &Uuid::new_v4().to_string()[..8]),
                        session_id: s.id.clone(),
                        op: item.mutation_op.clone(),
                        status: if draft {
                            MutationStatus::Draft
                        } else {
                            MutationStatus::Committed
                        },
                        estimated_saved_tokens: item.saved_tokens,
                        estimated_kv_break_tokens: item.kv_break_tokens,
                        created_at: chrono::Utc::now(),
                    };
                    store.save_mutation(&rule)?;
                }
                if !draft {
                    let _ = recompute_latest_turn_in_store(store, &s.id);
                    if sync_back && sync_mutations_to_transcript_on_disk(store, &s.id).is_ok() {
                        total_synced += 1;
                    }
                }
            }
        }

        println!(
            "✅ BatchOptimize ({}) [strategy: '{}'] — Matched {} sessions, compacted {} sessions ({} operations, -{} tokens{}).",
            if dry_run {
                "DRY RUN"
            } else if draft {
                "DRAFT"
            } else {
                "COMMITTED"
            },
            strat_id,
            target_sessions.len(),
            sessions_compacted,
            total_ops,
            total_saved_tok,
            if total_synced > 0 {
                format!(", synced {total_synced} transcripts to disk")
            } else {
                String::new()
            }
        );
        return Ok(());
    }

    let Some(sess_id) = resolve_session(store, session)? else {
        println!("No active sessions found.");
        return Ok(());
    };
    let Some(turn) = store.get_latest_turn(&sess_id)? else {
        println!("No turns recorded for session {sess_id}.");
        return Ok(());
    };
    let plans = store.optimization_plans(&turn)?;

    if strategy.is_none() || dry_run {
        println!(
            "┌──────────────────────────────────────────────────────────────────────────────────────────┐"
        );
        println!("│ ⚡ contextWatcher Optimization & Cut Studio — Session: {sess_id:<33} │");
        println!(
            "│    Current Context: {} tok | Health Score: {}/100 | Active Lints: {}                     │",
            turn.token_usage.input_tokens,
            turn.health_score,
            turn.lint_findings.len()
        );
        println!(
            "└──────────────────────────────────────────────────────────────────────────────────────────┘"
        );

        for p in &plans {
            if let Some(ref filter) = strategy
                && p.id != *filter
            {
                continue;
            }
            println!("\n{}  [id: {}]", p.name, p.id);
            println!("  • Tagline         : {}", p.tagline);
            println!("  • What & How      : {}", p.what_and_how);
            println!("  • Expected Impact : {}", p.expected_quality_impact);
            println!(
                "  • Expected Results: {} -> {} tok (-{} tok / -{:.1}%) | Health: {}/100 -> {}/100",
                p.tokens_before,
                p.tokens_after,
                p.saved_tokens,
                p.saved_pct,
                p.health_score_before,
                p.health_score_after
            );
            println!(
                "  • KV Cache Impact : {} ({} break-even turns) | {} candidate operations",
                p.kv_badge,
                p.break_even_turns,
                p.items.len()
            );

            if explain || strategy.is_some() {
                for item in &p.items {
                    println!(
                        "      - [T{}] {} ({} -> {} tok, -{} tok)",
                        item.turn_seq,
                        item.title,
                        item.tokens_before,
                        item.tokens_after,
                        item.saved_tokens.max(0)
                    );
                    println!("        What we do    : {}", item.what_we_do);
                    println!("        Why it matters: {}", item.why_it_matters);
                }
            }
        }

        if strategy.is_none() {
            println!(
                "\n💡 Tip: Run `cw optimize -s <id>` (single session) or `cw optimize -s <id> --all` / `--project <name>` / `--agent <name>`."
            );
            return Ok(());
        }
    }

    if !dry_run && let Some(strat_id) = strategy {
        if strat_id == "deep_trajectory_fold" {
            anyhow::bail!(
                "Automatic trajectory folding is unavailable without a verified outcome summary; use `cw fold <start_turn> <end_turn> --summary \"...\"` or `cw_fold_turns`"
            );
        }
        let Some(plan) = plans.iter().find(|p| p.id == strat_id) else {
            anyhow::bail!(
                "Unknown strategy '{}'. Valid IDs: cache_safe_boost, clean_code_state, surgical_log_diet, deep_trajectory_fold, max_token_diet",
                strat_id
            );
        };
        for item in &plan.items {
            let rule = MutationRule {
                id: format!("mut_{}", &Uuid::new_v4().to_string()[..8]),
                session_id: sess_id.clone(),
                op: item.mutation_op.clone(),
                status: if draft {
                    MutationStatus::Draft
                } else {
                    MutationStatus::Committed
                },
                estimated_saved_tokens: item.saved_tokens.max(0),
                estimated_kv_break_tokens: item.kv_break_tokens,
                created_at: chrono::Utc::now(),
            };
            store.save_mutation(&rule)?;
        }
        if !draft {
            let updated_turn = recompute_latest_turn_in_store(store, &sess_id);
            if let Some(mut t) = updated_turn {
                if t.token_usage.saved_by_mutations == 0 && plan.saved_tokens > 0 {
                    t.token_usage.saved_by_mutations = plan.saved_tokens;
                }
                if let Ok(Some(sess)) = store.get_session(&sess_id) {
                    let pulse = cw_core::autopilot::generate_ambient_savings_pulse(
                        &sess_id,
                        &t,
                        &sess.model,
                        sess.provider,
                        t.token_usage.saved_by_mutations as u64,
                    );
                    let _ = store.record_ambient_pulse(&pulse);
                    println!("\n{}", pulse.formatted_message);
                }
            }
            if sync_back {
                let report = sync_mutations_to_transcript_on_disk(store, &sess_id)?;
                println!("Exported compacted copy: {}", report.export_path);
            }
        }
        println!(
            "\n✅ {} {} optimization operations from '{}' on session {} (Est. saved: {} tok, Health: {} -> {}/100).",
            if draft { "Staged" } else { "Committed" },
            plan.items.len(),
            plan.name,
            sess_id,
            plan.saved_tokens,
            plan.health_score_before,
            plan.health_score_after
        );
    }

    Ok(())
}

/// Folds a range of turns into a single high-level outcome summary (`cw fold`).
pub fn run_fold(
    store: &CwStore,
    start_turn: u32,
    end_turn: u32,
    summary: String,
    session: Option<String>,
    draft: bool,
) -> Result<()> {
    let Some(sess_id) = resolve_session(store, session)? else {
        anyhow::bail!("No active session found");
    };
    save_cli_mutation(
        store,
        &sess_id,
        MutationOp::FoldSubtree {
            start_turn,
            end_turn,
            summary_text: summary,
        },
        draft,
    )?;
    println!("Folded turns {start_turn}..{end_turn} on session {sess_id}.");
    Ok(())
}

/// Prunes or stubs a target node, tool output, or JSON pointer (`cw prune`).
pub fn run_prune(
    store: &CwStore,
    target: String,
    summary: String,
    session: Option<String>,
) -> Result<()> {
    let Some(sess_id) = resolve_session(store, session)? else {
        anyhow::bail!("No active session found");
    };
    let is_ptr = target.starts_with('/');
    save_cli_mutation(
        store,
        &sess_id,
        MutationOp::PruneOrStub {
            target_fingerprint: if !is_ptr { Some(target.clone()) } else { None },
            target_call_id: if !is_ptr { Some(target.clone()) } else { None },
            target_json_pointer: if is_ptr { Some(target) } else { None },
            stub_text: format!("[cw:pruned summary=\"{summary}\"]"),
        },
        false,
    )?;
    println!("Applied PruneOrStub mutation on session {sess_id}.");
    Ok(())
}

/// Deduplicates repetitive file read results (`cw dedupe`).
pub fn run_dedupe(store: &CwStore, file: Option<String>, session: Option<String>) -> Result<()> {
    let Some(sess_id) = resolve_session(store, session)? else {
        anyhow::bail!("No active session found");
    };
    save_cli_mutation(
        store,
        &sess_id,
        MutationOp::ShadowDedupe { file_path: file },
        false,
    )?;
    println!("Applied ShadowDedupe mutation on session {sess_id}.");
    Ok(())
}

/// Temporarily disables inactive tools or MCP servers to reclaim context tokens (`cw gate`).
pub fn run_gate(
    store: &CwStore,
    disable_server: Vec<String>,
    disable_tool: Vec<String>,
    session: Option<String>,
) -> Result<()> {
    let Some(sess_id) = resolve_session(store, session)? else {
        anyhow::bail!("No active session found");
    };
    save_cli_mutation(
        store,
        &sess_id,
        MutationOp::GateToolsAndSkills {
            disabled_tools: disable_tool,
            disabled_mcp_servers: disable_server,
            disabled_skills: vec![],
        },
        false,
    )?;
    println!("Updated Tool & MCP Server Gate on session {sess_id}.");
    Ok(())
}

/// Pins critical system instructions or user rules to the high-attention recency tail (`cw pin`).
pub fn run_pin(
    store: &CwStore,
    label: String,
    content: String,
    session: Option<String>,
) -> Result<()> {
    let Some(sess_id) = resolve_session(store, session)? else {
        anyhow::bail!("No active session found");
    };
    save_cli_mutation(
        store,
        &sess_id,
        MutationOp::PinContext {
            label: label.clone(),
            content,
            source_node_id: None,
        },
        false,
    )?;
    println!("Pinned '{label}' to high-attention tail on session {sess_id}.");
    Ok(())
}

/// Imports an external transcript or session database into ContextWatcher (`cw import`).
pub fn run_import(
    store: &CwStore,
    path: PathBuf,
    harness: String,
    workspace: Option<String>,
) -> Result<()> {
    let p_str = path.to_string_lossy();
    let h = if harness == "antigravity" {
        if p_str.contains(".omp") {
            HarnessKind::Omp
        } else if p_str.contains(".pi") {
            HarnessKind::Pi
        } else if p_str.contains(".codex") {
            HarnessKind::Codex
        } else if p_str.contains(".claude") {
            HarnessKind::ClaudeCode
        } else if p_str.contains(".hermes") {
            HarnessKind::Hermes
        } else if p_str.contains("opencode") {
            HarnessKind::OpenCode
        } else {
            HarnessKind::Antigravity
        }
    } else {
        HarnessKind::from_str_lossy(&harness)
    };
    if (h == HarnessKind::OpenCode || p_str.contains("opencode"))
        && (path.extension().and_then(|e| e.to_str()) == Some("db")
            || path.extension().and_then(|e| e.to_str()) == Some("sqlite"))
    {
        let ids = import_opencode_state_db(&path, store, 20)?;
        println!(
            "Imported {} OpenCode sessions from {}: {}",
            ids.len(),
            path.display(),
            ids.join(", ")
        );
    } else if h == HarnessKind::Hermes
        || path.extension().and_then(|e| e.to_str()) == Some("db")
        || path.extension().and_then(|e| e.to_str()) == Some("sqlite")
    {
        let ids = import_hermes_state_db(&path, store, 20)?;
        println!(
            "Imported {} Hermes Agent sessions from {}: {}",
            ids.len(),
            path.display(),
            ids.join(", ")
        );
    } else {
        let sess_id = if h == HarnessKind::Antigravity {
            import_agy_transcript(&path, store, workspace)?
        } else {
            import_generic_jsonl_transcript(&path, h, store, workspace)?
        };
        println!("Imported transcript into session: {sess_id}");
    }
    Ok(())
}

/// Exports a compacted copy of a session's transcript to disk without modifying the original (`cw sync-back`).
pub fn run_sync_back(store: &CwStore, session: Option<String>) -> Result<()> {
    let Some(sess_id) = resolve_session(store, session)? else {
        anyhow::bail!("No active session found");
    };
    let report = sync_mutations_to_transcript_on_disk(store, &sess_id)?;
    println!("Exported a compacted copy; source transcript unchanged.");
    println!("  • Session         : {}", report.session_id);
    println!("  • Transcript Path : {}", report.transcript_path);
    println!("  • Export Path     : {}", report.export_path);
    println!("  • Steps Compacted : {}", report.steps_compacted);
    println!(
        "  • Disk Size       : {} bytes -> {} bytes (-{} bytes)",
        report.bytes_before,
        report.bytes_after,
        report.bytes_before.saturating_sub(report.bytes_after)
    );
    Ok(())
}

/// Analyzes local LLM KV cache memory consumption, VRAM footprint, and slot recommendations (`cw local`).
pub fn run_local(store: &CwStore, session: Option<String>, model: Option<String>) -> Result<()> {
    let Some(sess_id) = resolve_session(store, session)? else {
        println!("No active sessions found.");
        return Ok(());
    };
    let Some(sess) = store.get_session(&sess_id)? else {
        println!("Session {} not found in database.", sess_id);
        return Ok(());
    };
    let Some(turn) = store.get_latest_turn(&sess_id)? else {
        println!("No turns recorded for session {sess_id}.");
        return Ok(());
    };
    let model_str = model.unwrap_or_else(|| sess.model.clone());
    let loc = analyze_local_model_kv(&turn, &sess_id, &model_str, sess.provider);

    println!(
        "┌──────────────────────────────────────────────────────────────────────────────────────────┐"
    );
    println!("│ 🖥️  contextWatcher Local LLM & KV-Cache Slot Studio — Session: {sess_id:<25} │");
    println!(
        "└──────────────────────────────────────────────────────────────────────────────────────────┘"
    );
    println!(
        "  • Engine & Model           : {} ({})",
        loc.model_name, loc.engine_kind
    );
    println!(
        "  • Context Window Usage     : {} / {} tok ({:.1}%) [Recommended num_ctx: {}]",
        turn.token_usage.input_tokens, loc.num_ctx_limit, loc.used_ctx_pct, loc.recommended_num_ctx
    );
    println!(
        "  • Pin System Prefix (VRAM) : n_keep / num_keep = {} tokens (System Prompt + Rules + Tools)",
        loc.recommended_n_keep_tokens
    );
    println!(
        "  • Deterministic KV Slot    : Slot #{} (prevents subagent KV slot thrashing)",
        loc.recommended_slot_id
    );
    println!(
        "  • PagedAttention Alignment : {}-tok blocks ({} aligned tok, {} tail waste tok)",
        loc.paged_attention_block_size,
        loc.prefix_block_aligned_tokens,
        loc.prefix_block_waste_tokens
    );
    println!(
        "  • KV Cache VRAM Footprint  : {:.1} MB (FP16) | {:.1} MB (Q8_0) | {:.1} MB (Q4_0)",
        loc.vram_fp16_mb, loc.vram_q8_0_mb, loc.vram_q4_0_mb
    );
    println!(
        "  • Local GPU Prefill Time   : ~{} ms (Saved ~{} ms via KV Cache hits @ {} tok/s)",
        loc.prefill_latency_ms, loc.prefill_saved_by_kv_ms, loc.estimated_prefill_tps
    );
    println!("\n🔧 RECOMMENDATIONS & ENGINE PARAMETERS:");
    for rec in &loc.recommendations {
        println!("  • [{}] {}", rec.severity.to_uppercase(), rec.title);
        println!("    {}", rec.explanation);
        println!("    Fix / Config: {}", rec.cli_or_api_fix);
    }
    Ok(())
}

/// Inspects or configures the contextWatcher background autopilot policies (`cw autopilot`).
pub fn run_autopilot(
    store: &CwStore,
    enable: bool,
    disable: bool,
    min_health: Option<u8>,
    strategy: Option<String>,
    sync_to_disk: Option<bool>,
) -> Result<()> {
    let mut cfg = store.get_autopilot_config()?;
    let migrated_from_legacy = cfg.safety_version < 2;
    let mut changed = false;
    if enable {
        cfg.enabled = true;
        changed = true;
    } else if disable {
        cfg.enabled = false;
        changed = true;
    }
    if let Some(h) = min_health {
        cfg.min_health_score = h;
        changed = true;
    }
    if let Some(s) = strategy {
        cfg.heal_strategy = s;
        changed = true;
    }
    if let Some(sd) = sync_to_disk {
        cfg.auto_sync_to_disk = sd;
        changed = true;
    }
    if changed {
        store.set_autopilot_config(&cfg)?;
    } else if migrated_from_legacy {
        println!(
            "⚠️  Migration Notice: Legacy Auto-Pilot configuration (safety_version < 2) was disabled during the safety upgrade. Run `cw autopilot --enable` to re-enable automatic background optimization."
        );
    }
    println!(
        "Auto-Pilot Status: {}",
        if cfg.enabled {
            "🟢 ENABLED"
        } else {
            "🔴 DISABLED"
        }
    );
    println!(
        "  1. Legacy tail hygiene     : {} (threshold {} tok; automatic spills disabled)",
        cfg.zero_break_tail_hygiene, cfg.tail_max_tool_tokens
    );
    println!(
        "  2. Auto Cache-Safe Pin     : {} (pin rules <42% salience when context >{} tok)",
        cfg.auto_cache_safe_pin, cfg.pin_min_context_tokens
    );
    println!(
        "  3. Auto Diet at Breakpoint : {} (dedupe when context >{} tok)",
        cfg.auto_diet_at_breakpoint, cfg.breakpoint_tokens
    );
    println!(
        "  4. Local Window Guard      : {} (protect local models >{} tok before num_ctx shift)",
        cfg.local_model_window_guard, cfg.local_window_threshold_tokens
    );
    println!(
        "  5. Low-Health Self-Healer  : {} (auto-heal with '{}' when health < {}/100, sync_disk={})",
        cfg.auto_heal_low_health, cfg.heal_strategy, cfg.min_health_score, cfg.auto_sync_to_disk
    );
    Ok(())
}

/// Displays ambient savings pulse events for a session or globally (`cw pulse`).
pub fn run_pulse(store: &CwStore, session: Option<String>, limit: usize) -> Result<()> {
    if let Some(sess_target) = session {
        let Some(sess_id) = resolve_session(store, Some(sess_target))? else {
            println!("No matching session found.");
            return Ok(());
        };
        if let Some(pulse) = store.get_latest_pulse_for_session(&sess_id)? {
            println!("\nLatest Ambient Savings Pulse for {sess_id}:");
            println!("  {}", pulse.formatted_message);
            println!(
                "  Tokens Saved: {} ({:.1}%) | Speedup: {:.1}x | Est. Cost Saved: ${:.4} | Turn #{}",
                pulse.tokens_saved_this_pulse,
                pulse.saved_percentage,
                pulse.speedup_factor,
                pulse.estimated_cost_saved_usd,
                pulse.turn_seq
            );
        } else {
            println!("No ambient savings pulses recorded yet for session {sess_id}.");
        }
    } else {
        let pulses = store.list_recent_ambient_pulses(limit)?;
        if pulses.is_empty() {
            println!("No ambient savings pulses recorded yet across sessions.");
            println!(
                "💡 Tip: As sessions run or when you optimize, cw automatically records savings pulses."
            );
        } else {
            println!(
                "\n⚡ contextWatcher Ambient Savings Pulses (Recent {} events):\n",
                pulses.len()
            );
            for p in pulses {
                println!(
                    "  • [{}] {}",
                    p.timestamp.format("%Y-%m-%d %H:%M:%S"),
                    p.formatted_message
                );
            }
        }
    }
    Ok(())
}

/// Fast pre-turn or shell hook context check (`cw hook`).
pub fn run_hook_check(
    store: &CwStore,
    quiet: bool,
    explicit_session: Option<String>,
) -> Result<()> {
    let Some(sid) = resolve_session(store, explicit_session)? else {
        return Ok(());
    };
    let Some(turn) = store.get_latest_turn(&sid)? else {
        return Ok(());
    };

    let cfg = store.get_autopilot_config()?;
    if cfg.enabled && turn.token_usage.input_tokens >= cfg.breakpoint_tokens {
        let op = MutationOp::ShadowDedupe { file_path: None };
        let _ = save_cli_mutation(store, &sid, op, false);
        if !quiet {
            println!(
                "[cw] Auto-Pilot triggered on session '{}' ({} tok >= {} breakpoint): deduplicated stale reads.",
                sid, turn.token_usage.input_tokens, cfg.breakpoint_tokens
            );
        }
    } else if !quiet && (turn.health_score < 65 || turn.token_usage.input_tokens > 100_000) {
        println!(
            "[cw] Context alert for '{}': {} tok, Health {}/100 ({} lints). Run `cw optimize` or call `cw_optimize_strategy`.",
            sid,
            turn.token_usage.input_tokens,
            turn.health_score,
            turn.lint_findings.len()
        );
    }
    Ok(())
}

/// Explicitly purges contextWatcher history and owned exports; never modifies the source transcript (`cw purge`).
pub fn run_purge(store: &CwStore, session: &str, keep_turns: usize) -> Result<()> {
    println!(
        "Removed {} cw turn records. Source transcripts were not changed.",
        store.purge_history(session, keep_turns)?
    );
    Ok(())
}

/// Computes and prints an estimated mutation preview payload without forwarding or altering history (`cw preview`).
pub fn run_preview(store: &CwStore, session: &str) -> Result<()> {
    let latest = store
        .get_latest_turn(session)?
        .ok_or_else(|| anyhow::anyhow!("No recorded turn"))?;
    let meta = store
        .get_session(session)?
        .ok_or_else(|| anyhow::anyhow!("Session not found"))?;
    let mut payload = store
        .get_turn_raw_payload(&latest.id)?
        .ok_or_else(|| anyhow::anyhow!("Original unavailable"))?;
    let provider = latest.normalization_provider.unwrap_or(meta.provider);
    cw_core::mutation::apply_mutations_to_payload(
        &mut payload,
        provider,
        &store.list_mutations(session)?,
        latest.sequence_number,
    );
    let preview = recompute_latest_turn_in_store(store, session)
        .ok_or_else(|| anyhow::anyhow!("Preview unavailable"))?;
    println!(
        "{}",
        serde_json::to_string(
            &serde_json::json!({"turn":preview,"payload":payload,"basis":"estimated mutation preview; not an upstream measurement"})
        )?
    );
    Ok(())
}
