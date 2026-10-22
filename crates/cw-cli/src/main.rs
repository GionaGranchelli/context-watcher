use anyhow::Result;
use axum::{
    body::Body,
    http::{StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use clap::{Parser, Subcommand};
use cw_core::{
    ir::{HarnessKind, KvNodeStatus, MutationOp, MutationRule, MutationStatus},
    local_llm::{analyze_local_model_kv, calculate_cost_and_perf},
    optimizer::build_optimization_plans,
    radix::build_radix_graph,
};
use cw_harnesses::{
    import_agy_transcript, import_generic_jsonl_transcript, import_hermes_state_db,
    spawn_transcript_watcher, sync_mutations_to_transcript_on_disk,
};
use cw_mcp::run_mcp_stdio_server;
use cw_proxy::{DaemonState, build_api_router, build_proxy_router};
use cw_store::CwStore;
use cw_tui::run_tui;
use rust_embed::Embed;
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Embed)]
#[folder = "../../web/build/"]
struct WebAssets;

#[derive(Parser)]
#[command(
    name = "cw",
    version,
    about = "contextWatcher — Intercepting Proxy, KV Cache X-Ray, Linter & Surgical Debugger for LLM Harnesses"
)]
struct Cli {
    /// Path to SQLite database file
    #[arg(long, env = "CW_DB_PATH")]
    db: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the contextWatcher daemon (Control/Web UI server + HTTP/SSE Proxy + Transcript Watcher)
    Serve {
        #[arg(long, default_value_t = 4400)]
        port: u16,
        #[arg(long, default_value_t = 4401)]
        proxy_port: u16,
        #[arg(long)]
        workspace: Option<String>,
    },
    /// Wrap and launch an agent harness (agy, codex, pi, opencode, aider, claude) with proxy env vars injected
    Wrap {
        #[arg(long, default_value_t = 4401)]
        proxy_port: u16,
        #[arg(required = true, trailing_var_arg = true)]
        cmd: Vec<String>,
    },
    /// Launch the interactive Ratatui Terminal UI dashboard
    Tui {
        #[arg(long)]
        session: Option<String>,
    },
    /// Show context window token summary, KV cache hit ratio, and health score
    Status {
        #[arg(long)]
        session: Option<String>,
    },
    /// Print the ASCII Context Radix Prefix Tree for a session
    Tree {
        #[arg(long)]
        session: Option<String>,
    },
    /// Run and display Context Health Linter findings (stale files, duplicates, bloat, cache breaks)
    Lint {
        #[arg(long)]
        session: Option<String>,
        /// Automatically apply suggested mutations for all findings
        #[arg(long)]
        fix: bool,
    },
    /// Interactive Optimization & Cut Studio: preview or apply the 5 context optimization strategies
    Optimize {
        /// Strategy ID: cache_safe_boost | clean_code_state | surgical_log_diet | deep_trajectory_fold | max_token_diet
        #[arg(long, short = 's')]
        strategy: Option<String>,
        /// Only show the Before -> After simulation without applying mutations
        #[arg(long)]
        dry_run: bool,
        /// Show detailed per-node explanations (What we do & Why it matters)
        #[arg(long)]
        explain: bool,
        /// Stage optimizations into the Cache-Safe Draft Queue instead of committing immediately
        #[arg(long)]
        draft: bool,
        #[arg(long)]
        session: Option<String>,
    },
    /// Fold a range of turns into a single concise outcome summary
    Fold {
        start_turn: u32,
        end_turn: u32,
        #[arg(long, default_value = "Collapsed intermediate debugging turns")]
        summary: String,
        #[arg(long)]
        session: Option<String>,
        #[arg(long)]
        draft: bool,
    },
    /// Prune or stub a specific node by fingerprint, call_id, or JSON pointer
    Prune {
        target: String,
        #[arg(long, default_value = "Compacted by user via CLI")]
        summary: String,
        #[arg(long)]
        session: Option<String>,
    },
    /// Shadow-deduplicate stale or duplicate file reads in context
    Dedupe {
        #[arg(long)]
        file: Option<String>,
        #[arg(long)]
        session: Option<String>,
    },
    /// Gate (disable) MCP tool servers or specific tool schemas
    Gate {
        #[arg(long)]
        disable_server: Vec<String>,
        #[arg(long)]
        disable_tool: Vec<String>,
        #[arg(long)]
        session: Option<String>,
    },
    /// Pin a critical constraint or code note to the high-attention tail of the prompt (0% KV break)
    Pin {
        label: String,
        content: String,
        #[arg(long)]
        session: Option<String>,
    },
    /// Import a local harness session transcript (.jsonl) into contextWatcher
    Import {
        path: PathBuf,
        #[arg(long, default_value = "antigravity")]
        harness: String,
        #[arg(long)]
        workspace: Option<String>,
    },
    /// Run the built-in JSON-RPC 2.0 MCP server over stdio
    Mcp {
        #[arg(long, default_value = "http://127.0.0.1:4400")]
        daemon_url: String,
    },
    /// Safely write committed compactions back to the harness's on-disk .jsonl transcript (.bak backup created automatically)
    SyncBack {
        #[arg(long)]
        session: Option<String>,
    },
    /// Local LLM & KV-Cache Slot Studio (Ollama, llama.cpp, vLLM, SGLang, LM Studio): inspect VRAM, n_keep/num_keep pinning, and slot affinity
    Local {
        #[arg(long)]
        session: Option<String>,
        #[arg(long)]
        model: Option<String>,
    },
    /// View or configure hands-free Auto-Pilot background optimization policies
    Autopilot {
        #[arg(long)]
        enable: bool,
        #[arg(long)]
        disable: bool,
    },
    /// Install contextWatcher Skill & MCP config for Antigravity, Claude Code, Codex, OpenCode & Pi
    InstallIntegrations,
}

fn default_db_path() -> PathBuf {
    if let Ok(p) = std::env::var("CW_DB_PATH") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join(".local/share/context-watcher/cw.db")
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let db_path = cli.db.unwrap_or_else(default_db_path);

    match cli.command {
        Commands::Mcp { daemon_url } => {
            run_mcp_stdio_server(&daemon_url).await?;
        }
        Commands::Serve {
            port,
            proxy_port,
            workspace,
        } => {
            tracing_subscriber::fmt()
                .with_env_filter(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| "info".into()),
                )
                .init();

            let store = CwStore::open(&db_path)?;
            let ws_root = workspace.or_else(|| {
                std::env::current_dir()
                    .ok()
                    .and_then(|p| p.to_str().map(|s| s.to_string()))
            });
            spawn_transcript_watcher(store.clone(), ws_root);

            let state = DaemonState::new(store);
            let proxy_app = build_proxy_router(state.clone());
            let ui_and_api_app = build_api_router(state).fallback(serve_embedded_web_ui);

            let proxy_addr = format!("127.0.0.1:{proxy_port}");
            let ui_addr = format!("127.0.0.1:{port}");

            println!("┌──────────────────────────────────────────────────────────────────┐");
            println!("│ contextWatcher Daemon Active                                     │");
            println!("│ • Web UI & Control API : http://{ui_addr:<31} │");
            println!("│ • Intercepting Proxy   : http://{proxy_addr:<31} │");
            println!("│   - Anthropic Base URL : http://{proxy_addr}/anthropic          │");
            println!("│   - OpenAI Base URL    : http://{proxy_addr}/openai/v1          │");
            println!("│   - Gemini Base URL    : http://{proxy_addr}/gemini             │");
            println!("│   - Ollama Local Proxy : http://{proxy_addr}/ollama             │");
            println!("│   - llama.cpp Proxy    : http://{proxy_addr}/llamacpp           │");
            println!("│   - vLLM Local Proxy   : http://{proxy_addr}/vllm/v1            │");
            println!("└──────────────────────────────────────────────────────────────────┘");

            let proxy_listener = tokio::net::TcpListener::bind(&proxy_addr).await?;
            let ui_listener = tokio::net::TcpListener::bind(&ui_addr).await?;

            tokio::try_join!(
                axum::serve(proxy_listener, proxy_app),
                axum::serve(ui_listener, ui_and_api_app),
            )?;
        }
        Commands::Wrap { proxy_port, cmd } => {
            let bin = &cmd[0];
            let args = &cmd[1..];
            let session_id = format!("wrap_{}_{}", bin, &Uuid::new_v4().to_string()[..6]);
            let status = std::process::Command::new(bin)
                .args(args)
                .env(
                    "ANTHROPIC_BASE_URL",
                    format!("http://127.0.0.1:{proxy_port}/anthropic"),
                )
                .env(
                    "OPENAI_BASE_URL",
                    format!("http://127.0.0.1:{proxy_port}/openai/v1"),
                )
                .env(
                    "GEMINI_API_BASE_URL",
                    format!("http://127.0.0.1:{proxy_port}/gemini"),
                )
                .env("CW_SESSION_ID", &session_id)
                .status()?;
            std::process::exit(status.code().unwrap_or(1));
        }
        Commands::Tui { session } => {
            let store = CwStore::open(&db_path)?;
            run_tui(store, session)?;
        }
        Commands::Status { session } => {
            let store = CwStore::open(&db_path)?;
            let Some(sess_id) = resolve_session(&store, session)? else {
                println!("No active sessions found in {}", db_path.display());
                return Ok(());
            };
            let sess = store.get_session(&sess_id)?.unwrap();
            let Some(turn) = store.get_latest_turn(&sess_id)? else {
                println!("Session {} has no turns.", sess.id);
                return Ok(());
            };
            let mutations = store.list_mutations(&sess_id)?;

            println!("Session      : {} ({} • {})", sess.id, sess.harness.as_str(), sess.model);
            println!("Turn         : #{} ({} nodes)", turn.sequence_number, turn.nodes.len());
            println!(
                "Tokens       : {} / {} ({:.1}%) [Saved by overlays: {} tok]",
                turn.token_usage.input_tokens,
                turn.token_usage.context_limit,
                (turn.token_usage.input_tokens as f64 / turn.token_usage.context_limit.max(1) as f64) * 100.0,
                turn.token_usage.saved_by_mutations
            );
            println!(
                "KV Cache     : {:.1}% Hit ({} hit | {} new | {} invalidated)",
                turn.kv_cache_report.hit_ratio * 100.0,
                turn.kv_cache_report.hit_tokens,
                turn.kv_cache_report.newly_cached_tokens,
                turn.kv_cache_report.invalidated_tokens
            );
            println!("Health Score : {}/100 ({} lints, {} active overlays)", turn.health_score, turn.lint_findings.len(), mutations.len());
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
        }
        Commands::Tree { session } => {
            let store = CwStore::open(&db_path)?;
            let Some(sess_id) = resolve_session(&store, session)? else {
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
                let branch = if idx + 1 == graph.nodes.len() { "└──" } else { "├──" };
                let kv = match gnode.kv_status {
                    KvNodeStatus::CachedHit => "🟢 HIT ",
                    KvNodeStatus::NewlyCached => "🟡 NEW ",
                    KvNodeStatus::CacheInvalidated => "🔴 BRK ",
                    KvNodeStatus::Uncached => "⚪ TAIL",
                };
                let badges = if gnode.lint_badges.is_empty() {
                    String::new()
                } else {
                    format!(" ⚠️ [{}]", gnode.lint_badges.join(", "))
                };
                println!("  {branch} [{kv}] {}{badges}", gnode.label);
            }
        }
        Commands::Lint { session, fix } => {
            let store = CwStore::open(&db_path)?;
            let Some(sess_id) = resolve_session(&store, session)? else {
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
                if fix
                    && let Some(op) = &f.suggested_mutation {
                        save_cli_mutation(&store, &sess_id, op.clone(), false)?;
                        println!("    ✓ Applied suggested fix: {:?}", op);
                    }
            }
        }
        Commands::Optimize {
            strategy,
            dry_run,
            explain,
            draft,
            session,
        } => {
            let store = CwStore::open(&db_path)?;
            let Some(sess_id) = resolve_session(&store, session)? else {
                println!("No active sessions found.");
                return Ok(());
            };
            let Some(turn) = store.get_latest_turn(&sess_id)? else {
                println!("No turns recorded for session {sess_id}.");
                return Ok(());
            };
            let plans = build_optimization_plans(&turn);

            if strategy.is_none() || dry_run {
                println!("┌──────────────────────────────────────────────────────────────────────────────────────────┐");
                println!("│ ⚡ contextWatcher Optimization & Cut Studio — Session: {sess_id:<33} │");
                println!("│    Current Context: {} tok | Health Score: {}/100 | Active Lints: {}                     │",
                    turn.token_usage.input_tokens, turn.health_score, turn.lint_findings.len());
                println!("└──────────────────────────────────────────────────────────────────────────────────────────┘");

                for p in &plans {
                    if let Some(ref filter) = strategy
                        && p.id != *filter {
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
                    println!("\n💡 Tip: Run `cw optimize --strategy <id>` to apply a strategy, or add `--explain` to inspect every node.");
                    return Ok(());
                }
            }

            if !dry_run
                && let Some(strat_id) = strategy {
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
        }
        Commands::Fold {
            start_turn,
            end_turn,
            summary,
            session,
            draft,
        } => {
            let store = CwStore::open(&db_path)?;
            let Some(sess_id) = resolve_session(&store, session)? else {
                anyhow::bail!("No active session found");
            };
            save_cli_mutation(
                &store,
                &sess_id,
                MutationOp::FoldSubtree {
                    start_turn,
                    end_turn,
                    summary_text: summary,
                },
                draft,
            )?;
            println!("Folded turns {start_turn}..{end_turn} on session {sess_id}.");
        }
        Commands::Prune {
            target,
            summary,
            session,
        } => {
            let store = CwStore::open(&db_path)?;
            let Some(sess_id) = resolve_session(&store, session)? else {
                anyhow::bail!("No active session found");
            };
            let is_ptr = target.starts_with('/');
            save_cli_mutation(
                &store,
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
        }
        Commands::Dedupe { file, session } => {
            let store = CwStore::open(&db_path)?;
            let Some(sess_id) = resolve_session(&store, session)? else {
                anyhow::bail!("No active session found");
            };
            save_cli_mutation(&store, &sess_id, MutationOp::ShadowDedupe { file_path: file }, false)?;
            println!("Applied ShadowDedupe mutation on session {sess_id}.");
        }
        Commands::Gate {
            disable_server,
            disable_tool,
            session,
        } => {
            let store = CwStore::open(&db_path)?;
            let Some(sess_id) = resolve_session(&store, session)? else {
                anyhow::bail!("No active session found");
            };
            save_cli_mutation(
                &store,
                &sess_id,
                MutationOp::GateToolsAndSkills {
                    disabled_tools: disable_tool,
                    disabled_mcp_servers: disable_server,
                    disabled_skills: vec![],
                },
                false,
            )?;
            println!("Updated Tool & MCP Server Gate on session {sess_id}.");
        }
        Commands::Pin {
            label,
            content,
            session,
        } => {
            let store = CwStore::open(&db_path)?;
            let Some(sess_id) = resolve_session(&store, session)? else {
                anyhow::bail!("No active session found");
            };
            save_cli_mutation(
                &store,
                &sess_id,
                MutationOp::PinContext {
                    label: label.clone(),
                    content,
                    source_node_id: None,
                },
                false,
            )?;
            println!("Pinned '{label}' to high-attention tail on session {sess_id}.");
        }
        Commands::Import {
            path,
            harness,
            workspace,
        } => {
            let store = CwStore::open(&db_path)?;
            let h = HarnessKind::from_str_lossy(&harness);
            if h == HarnessKind::Hermes
                || path.extension().and_then(|e| e.to_str()) == Some("db")
                || path.extension().and_then(|e| e.to_str()) == Some("sqlite")
            {
                let ids = import_hermes_state_db(&path, &store, 20)?;
                println!(
                    "Imported {} Hermes Agent sessions from {}: {}",
                    ids.len(),
                    path.display(),
                    ids.join(", ")
                );
            } else {
                let sess_id = if h == HarnessKind::Antigravity {
                    import_agy_transcript(&path, &store, workspace)?
                } else {
                    import_generic_jsonl_transcript(&path, h, &store, workspace)?
                };
                println!("Imported transcript into session: {sess_id}");
            }
        }
        Commands::SyncBack { session } => {
            let store = CwStore::open(&db_path)?;
            let Some(sess_id) = resolve_session(&store, session)? else {
                anyhow::bail!("No active session found");
            };
            let report = sync_mutations_to_transcript_on_disk(&store, &sess_id)?;
            println!("✅ Synced compacted context back to harness transcript on disk!");
            println!("  • Session         : {}", report.session_id);
            println!("  • Transcript Path : {}", report.transcript_path);
            println!("  • Safe Backup     : {}", report.backup_path);
            println!("  • Steps Compacted : {}", report.steps_compacted);
            println!(
                "  • Disk Size       : {} bytes -> {} bytes (-{} bytes)",
                report.bytes_before,
                report.bytes_after,
                report.bytes_before.saturating_sub(report.bytes_after)
            );
        }
        Commands::Local { session, model } => {
            let store = CwStore::open(&db_path)?;
            let Some(sess_id) = resolve_session(&store, session)? else {
                println!("No active sessions found.");
                return Ok(());
            };
            let sess = store.get_session(&sess_id)?.unwrap();
            let Some(turn) = store.get_latest_turn(&sess_id)? else {
                println!("No turns recorded for session {sess_id}.");
                return Ok(());
            };
            let model_str = model.unwrap_or_else(|| sess.model.clone());
            let loc = analyze_local_model_kv(&turn, &sess_id, &model_str, sess.provider);

            println!("┌──────────────────────────────────────────────────────────────────────────────────────────┐");
            println!("│ 🖥️  contextWatcher Local LLM & KV-Cache Slot Studio — Session: {sess_id:<25} │");
            println!("└──────────────────────────────────────────────────────────────────────────────────────────┘");
            println!("  • Engine & Model           : {} ({})", loc.model_name, loc.engine_kind);
            println!(
                "  • Context Window Usage     : {} / {} tok ({:.1}%) [Recommended num_ctx: {}]",
                turn.token_usage.input_tokens,
                loc.num_ctx_limit,
                loc.used_ctx_pct,
                loc.recommended_num_ctx
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
        }
        Commands::Autopilot { enable, disable } => {
            let store = CwStore::open(&db_path)?;
            let mut cfg = store.get_autopilot_config()?;
            if enable {
                cfg.enabled = true;
                store.set_autopilot_config(&cfg)?;
            } else if disable {
                cfg.enabled = false;
                store.set_autopilot_config(&cfg)?;
            }
            println!(
                "Auto-Pilot Status: {}",
                if cfg.enabled { "🟢 ENABLED" } else { "🔴 DISABLED" }
            );
            println!(
                "  1. Zero-Break Tail Hygiene : {} (spill >{} tok tool outputs to /tmp/cw_logs/)",
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
        }
        Commands::InstallIntegrations => {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("cw"));
            let agy_skill_dir = PathBuf::from(&home).join(".gemini/config/skills/context-watcher");
            let _ = std::fs::create_dir_all(&agy_skill_dir);
            let skill_src = PathBuf::from("plugins/agy/skills/context-watcher/SKILL.md");
            if skill_src.exists() {
                let _ = std::fs::copy(&skill_src, agy_skill_dir.join("SKILL.md"));
            }
            println!("✅ Installed Antigravity skill to: {}", agy_skill_dir.join("SKILL.md").display());
            println!("\n📌 Add this MCP server block to your harness config (Antigravity / Claude Code / Codex / OpenCode):");
            println!(
                "{{\n  \"mcpServers\": {{\n    \"context-watcher\": {{\n      \"command\": \"{}\",\n      \"args\": [\"mcp\", \"--daemon-url\", \"http://127.0.0.1:4400\"]\n    }}\n  }}\n}}",
                exe.display()
            );
        }
    }

    Ok(())
}

fn resolve_session(store: &CwStore, explicit: Option<String>) -> Result<Option<String>> {
    if let Some(s) = explicit {
        return Ok(Some(s));
    }
    let sessions = store.list_sessions()?;
    Ok(sessions.first().map(|s| s.id.clone()))
}

fn save_cli_mutation(
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

async fn serve_embedded_web_ui(uri: Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/');
    let target = if path.is_empty() { "index.html" } else { path };

    match WebAssets::get(target).or_else(|| WebAssets::get("index.html")) {
        Some(content) => {
            let mime = mime_guess::from_path(target).first_or_octet_stream();
            Response::builder()
                .header(header::CONTENT_TYPE, mime.as_ref())
                .body(Body::from(content.data.into_owned()))
                .unwrap()
        }
        None => (StatusCode::NOT_FOUND, "Web UI asset not found").into_response(),
    }
}
