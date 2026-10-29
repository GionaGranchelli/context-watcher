use anyhow::Result;
use axum::{
    body::Body,
    http::{StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use clap::{Parser, Subcommand};
use cw_core::{
    hud::{evaluate_attention_ecg, generate_context_hud_summary},
    ir::{
        ContextTurn, HarnessKind, KvNodeStatus, MutationOp, MutationRule, MutationStatus, Session,
    },
    local_llm::{analyze_local_model_kv, calculate_cost_and_perf},
    radix::build_radix_graph,
    slot_bridge::{
        DEFAULT_PAGED_ATTENTION_BLOCK_SIZE, EngineSlotInfo, SlotSplicingAction, SlotSplicingPlan,
        assign_deterministic_slot, plan_slot_splicing,
    },
    swarm::analyze_swarm_prefix_sharing,
};
use cw_harnesses::{
    import_agy_transcript, import_generic_jsonl_transcript, import_hermes_state_db,
    import_opencode_state_db, spawn_transcript_watcher, sync_mutations_to_transcript_on_disk,
};
use cw_mcp::run_mcp_stdio_server;
use cw_proxy::{DaemonState, build_api_router, build_proxy_router, recompute_latest_turn_in_store};
use cw_store::CwStore;
use cw_tui::run_tui;
use rust_embed::Embed;
use std::path::PathBuf;
use uuid::Uuid;

mod update;

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
    /// Update cw from the latest release (or build from the latest main branch)
    Update {
        /// Build from the latest main branch instead of downloading a release
        #[arg(long)]
        from_source: bool,
        /// Install integrations but leave the running daemon untouched
        #[arg(long)]
        no_restart: bool,
    },
    /// Print the private local control credential for connecting the web UI
    AuthToken,
    /// Explicitly purge cw history and owned exports; never deletes the source transcript
    Purge {
        #[arg(long)]
        session: String,
        #[arg(long, default_value_t = 0)]
        keep_turns: usize,
    },
    /// Print an estimated mutation preview without forwarding or rewriting history
    Preview {
        #[arg(long)]
        session: String,
    },
    /// Start the contextWatcher daemon (Control/Web UI server + HTTP/SSE Proxy + Transcript Watcher)
    Serve {
        #[arg(long, default_value_t = 4400)]
        port: u16,
        #[arg(long, default_value_t = 4401)]
        proxy_port: u16,
        #[arg(long)]
        workspace: Option<String>,
        /// Run daemon detached in the background
        #[arg(long, short = 'd')]
        detach: bool,
    },
    /// Manage the background contextWatcher daemon (start, stop, restart, status)
    Daemon {
        #[command(subcommand)]
        action: DaemonAction,
    },
    /// Wrap and launch an agent harness (agy, codex, pi, opencode, aider, claude, hermes) with proxy env vars injected
    Wrap {
        #[arg(long, default_value_t = 4401)]
        proxy_port: u16,
        /// Stable ID for multiple wrapper invocations in one harness conversation
        #[arg(long)]
        session_id: Option<String>,
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
        /// Optional positional intensity level (`min`, `mid`, `max`) or strategy name
        #[arg(value_name = "LEVEL_OR_STRATEGY")]
        level: Option<String>,
        /// Strategy ID: min | mid | max | cache_safe_boost | clean_code_state | surgical_log_diet | deep_trajectory_fold | max_token_diet
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
        /// Target a session ID; otherwise require a unique exact workspace match
        #[arg(long)]
        session: Option<String>,
        /// Apply the strategy across ALL sessions across all projects and agents
        #[arg(long)]
        all: bool,
        /// Apply the strategy across all sessions belonging to a specific project name (e.g. `aurora`, `android-agent`)
        #[arg(long)]
        project: Option<String>,
        /// Apply the strategy across all sessions belonging to a specific agent harness (`antigravity`, `codex`, `hermes`, `claude_code`)
        #[arg(long)]
        agent: Option<String>,
        /// Also export a separate compacted .jsonl copy alongside the transcript (leaving the source transcript unchanged)
        #[arg(long)]
        sync_back: bool,
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
    /// Export a separate compacted .jsonl copy of the harness transcript (leaving the source transcript unchanged)
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
        /// Set minimum health score threshold before self-healing triggers (default: 75)
        #[arg(long)]
        min_health: Option<u8>,
        /// Set optimization strategy for self-healing (min, mid, max, clean_code_state, etc.)
        #[arg(long)]
        strategy: Option<String>,
        /// Legacy setting retained for compatibility; automatic transcript writes are disabled
        #[arg(long)]
        sync_to_disk: Option<bool>,
    },
    /// Show token, KV-cache, and dollar ($) savings analytics across all sessions, agents, and projects (like `rtk gain`)
    Gain {
        /// Show detailed mutation overlay history
        #[arg(long)]
        history: bool,
    },
    /// Display recent ambient savings notifications ("We saved you X tokens, Y% KV Cache, Z% TTFT speedup")
    Pulse {
        /// Target a specific session ID
        #[arg(long)]
        session: Option<String>,
        /// Maximum number of pulses to display
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    /// Multi-Agent Swarm prefix sharing & VRAM multiplexing analysis (Horizon 2 / CW-060 / CW-061)
    Swarm {
        /// Filter by project workspace name
        #[arg(long)]
        project: Option<String>,
        /// Filter by model name
        #[arg(long)]
        model: Option<String>,
        /// Minimum member count in cluster (default: 2)
        #[arg(long, default_value_t = 2)]
        min_members: usize,
        /// Output raw JSON structure
        #[arg(long)]
        json: bool,
    },
    /// Ambient Context HUD & Attention ECG statusline (Horizon 4 / CW-080)
    Hud {
        /// Target a specific session ID (defaults to most recent active session)
        #[arg(long)]
        session: Option<String>,
        /// Emit ultra-compact glyph format for shell prompt (Starship / tmux / zellij)
        #[arg(long)]
        compact: bool,
        /// Display the full positional attention ECG curve (Lost-in-the-Middle analysis)
        #[arg(long)]
        ecg: bool,
        /// Output raw JSON structure
        #[arg(long)]
        json: bool,
    },
    /// Engine-Level PagedAttention & Slot Bridge (Horizon 3 / CW-070)
    Slot {
        /// Target a specific session ID
        #[arg(long)]
        session: Option<String>,
        /// Upstream local engine URL (defaults to CW_LLAMACPP_UPSTREAM or http://127.0.0.1:8080)
        #[arg(long)]
        engine: Option<String>,
        /// Target slot ID (0..N) to inspect or splice
        #[arg(long)]
        slot_id: Option<u32>,
        /// Simulate or execute PagedAttention block unmapping for a token range (e.g. "500..1200")
        #[arg(long)]
        splice_range: Option<String>,
        /// Output raw JSON structure
        #[arg(long)]
        json: bool,
    },
    /// Install contextWatcher integrations for Antigravity, Codex, Claude Code, Pi & Hermes
    Init {
        /// Only preview what files will be created or modified without writing to disk
        #[arg(long)]
        dry_run: bool,
        /// Do not automatically start the background daemon after registration
        #[arg(long)]
        no_start: bool,
    },
    /// Fast PostToolUse / PreCompact hook invoked by harnesses to enforce Auto-Pilot context hygiene
    Hook {
        #[arg(long)]
        quiet: bool,
        #[arg(long)]
        session: Option<String>,
    },
    /// Alias for `cw init`: install contextWatcher integrations for supported harnesses, including Hermes
    InstallIntegrations,
}

#[derive(Subcommand)]
enum DaemonAction {
    /// Start the contextWatcher daemon in the background
    Start {
        #[arg(long, default_value_t = 4400)]
        port: u16,
        #[arg(long, default_value_t = 4401)]
        proxy_port: u16,
        #[arg(long)]
        workspace: Option<String>,
    },
    /// Stop the background contextWatcher daemon
    Stop {
        #[arg(long, default_value_t = 4400)]
        port: u16,
    },
    /// Restart the background contextWatcher daemon
    Restart {
        #[arg(long, default_value_t = 4400)]
        port: u16,
        #[arg(long, default_value_t = 4401)]
        proxy_port: u16,
        #[arg(long)]
        workspace: Option<String>,
    },
    /// Inspect background daemon status and port health
    Status {
        #[arg(long, default_value_t = 4400)]
        port: u16,
    },
}

fn default_db_path() -> PathBuf {
    if let Ok(p) = std::env::var("CW_DB_PATH") {
        return PathBuf::from(p);
    }
    cw_core::home_dir().join(".local/share/context-watcher/cw.db")
}

fn valid_proxy_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn without_cw_headers(existing: &str) -> String {
    let lines: Vec<String> = existing
        .lines()
        .filter(|line| !is_cw_header(line))
        .map(str::to_owned)
        .collect();
    lines.join("\n")
}

fn without_cw_comma_separated_headers(existing: &str) -> String {
    let mut headers = Vec::new();
    let mut start = 0;
    for (index, ch) in existing.char_indices() {
        if ch == ','
            && existing[index + 1..]
                .split_once(':')
                .is_some_and(|(name, _)| !name.trim().is_empty() && !name.contains(','))
        {
            headers.push(&existing[start..index]);
            start = index + 1;
        }
    }
    headers.push(&existing[start..]);
    headers
        .into_iter()
        .filter(|header| !is_cw_header(header))
        .map(str::trim)
        .filter(|header| !header.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

fn is_cw_header(header: &str) -> bool {
    header
        .split_once(':')
        .is_some_and(|(name, _)| name.trim().to_ascii_lowercase().starts_with("x-cw-"))
}

fn codex_proxy_config_args(proxy_port: u16) -> Vec<String> {
    [
        "model_provider=\"context_watcher\"".to_owned(),
        "model_providers.context_watcher.name=\"contextWatcher\"".to_owned(),
        format!(
            "model_providers.context_watcher.base_url=\"http://127.0.0.1:{proxy_port}/openai/v1\""
        ),
        "model_providers.context_watcher.wire_api=\"responses\"".to_owned(),
        "model_providers.context_watcher.requires_openai_auth=true".to_owned(),
        "model_providers.context_watcher.env_key=\"OPENAI_API_KEY\"".to_owned(),
        "model_providers.context_watcher.env_http_headers={\"x-cw-session-id\"=\"CW_SESSION_ID\",\"x-cw-control-token\"=\"CW_CONTROL_TOKEN\"}".to_owned(),
        "shell_environment_policy.filters.CW_CONTROL_TOKEN=\"exclude\"".to_owned(),
    ]
    .into_iter()
    .flat_map(|setting| ["--config".to_owned(), setting])
    .collect()
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let db_path = cli.db.unwrap_or_else(default_db_path);

    match cli.command {
        Commands::Update {
            from_source,
            no_restart,
        } => {
            update::run(from_source, no_restart).await?;
        }
        Commands::Purge {
            session,
            keep_turns,
        } => {
            let store = CwStore::open(&db_path)?;
            println!(
                "Removed {} cw turn records. Source transcripts were not changed.",
                store.purge_history(&session, keep_turns)?
            );
        }
        Commands::Preview { session } => {
            let store = CwStore::open(&db_path)?;
            let latest = store
                .get_latest_turn(&session)?
                .ok_or_else(|| anyhow::anyhow!("No recorded turn"))?;
            let meta = store
                .get_session(&session)?
                .ok_or_else(|| anyhow::anyhow!("Session not found"))?;
            let mut payload = store
                .get_turn_raw_payload(&latest.id)?
                .ok_or_else(|| anyhow::anyhow!("Original unavailable"))?;
            let provider = latest.normalization_provider.unwrap_or(meta.provider);
            cw_core::mutation::apply_mutations_to_payload(
                &mut payload,
                provider,
                &store.list_mutations(&session)?,
                latest.sequence_number,
            );
            let preview = recompute_latest_turn_in_store(&store, &session)
                .ok_or_else(|| anyhow::anyhow!("Preview unavailable"))?;
            println!(
                "{}",
                serde_json::to_string(
                    &serde_json::json!({"turn":preview,"payload":payload,"basis":"estimated mutation preview; not an upstream measurement"})
                )?
            );
        }
        Commands::AuthToken => {
            println!("{}", cw_core::control::control_token(true)?);
        }
        Commands::Mcp { daemon_url } => {
            run_mcp_stdio_server(&daemon_url).await?;
        }
        Commands::Serve {
            port,
            proxy_port,
            workspace,
            detach,
        } => {
            if detach {
                start_daemon_detached(port, proxy_port, workspace)?;
                return Ok(());
            }

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

            let mut state = DaemonState::new(store);
            state.control_token = cw_core::control::control_token(true)?;
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
            println!("│   - DeepSeek Base URL  : http://{proxy_addr}/deepseek           │");
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
        Commands::Daemon { action } => match action {
            DaemonAction::Start {
                port,
                proxy_port,
                workspace,
            } => {
                start_daemon_detached(port, proxy_port, workspace)?;
            }
            DaemonAction::Stop { port } => {
                stop_daemon(port)?;
            }
            DaemonAction::Restart {
                port,
                proxy_port,
                workspace,
            } => {
                stop_daemon(port)?;
                start_daemon_detached(port, proxy_port, workspace)?;
            }
            DaemonAction::Status { port } => {
                status_daemon(port)?;
            }
        },
        Commands::Wrap {
            proxy_port,
            session_id,
            cmd,
        } => {
            let bin = &cmd[0];
            let is_codex = PathBuf::from(bin)
                .file_stem()
                .is_some_and(|name| name.eq_ignore_ascii_case("codex"));
            let is_hermes = PathBuf::from(bin)
                .file_stem()
                .is_some_and(|name| name.eq_ignore_ascii_case("hermes"));
            let is_gemini = PathBuf::from(bin)
                .file_stem()
                .is_some_and(|name| name.eq_ignore_ascii_case("gemini"));
            let mut args = Vec::new();
            if is_codex {
                args.extend(codex_proxy_config_args(proxy_port));
            }
            args.extend(cmd[1..].iter().cloned());
            if is_hermes && cmd.get(1).is_some_and(|arg| arg == "chat") {
                // Hermes config takes precedence over DEEPSEEK_BASE_URL; its chat CLI flag wins.
                args.extend([
                    "--base-url".to_owned(),
                    format!("http://127.0.0.1:{proxy_port}/deepseek"),
                ]);
            }
            let session_id = session_id.or_else(|| std::env::var("CW_SESSION_ID").ok());
            if session_id
                .as_deref()
                .is_some_and(|id| !valid_proxy_session_id(id))
            {
                anyhow::bail!("Invalid session ID; use 1-128 ASCII letters, digits, '_' or '-'");
            }
            let token = if is_codex {
                Some(cw_core::control::control_token(true)?)
            } else {
                None
            };
            let anthropic_headers =
                without_cw_headers(&std::env::var("ANTHROPIC_CUSTOM_HEADERS").unwrap_or_default());
            let gemini_headers = without_cw_comma_separated_headers(
                &std::env::var("GEMINI_CLI_CUSTOM_HEADERS").unwrap_or_default(),
            );
            let mut process = std::process::Command::new(bin);
            process
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
                    "OPENAI_API_BASE",
                    format!("http://127.0.0.1:{proxy_port}/openai/v1"),
                )
                .env(
                    "GOOGLE_GEMINI_BASE_URL",
                    format!("http://127.0.0.1:{proxy_port}/gemini"),
                )
                .env("ANTHROPIC_CUSTOM_HEADERS", anthropic_headers);
            if is_gemini {
                process.env("GEMINI_CLI_CUSTOM_HEADERS", gemini_headers);
            }
            process
                .env_remove("CW_CONTROL_TOKEN")
                .env_remove("CW_CONTROL_TOKEN_FILE");
            if let Some(token) = token {
                process.env("CW_CONTROL_TOKEN", token);
            }
            if is_hermes {
                process.env_remove("CW_SESSION_ID").env(
                    "DEEPSEEK_BASE_URL",
                    format!("http://127.0.0.1:{proxy_port}/deepseek"),
                );
                process.env("CW_CONTROL_TOKEN_FILE", cw_core::control::token_path());
                if let Some(session_id) = session_id.as_deref() {
                    process.env("CW_SESSION_ID", session_id);
                }
            } else if let Some(session_id) = session_id.as_deref() {
                process.env("CW_SESSION_ID", session_id);
            } else {
                process.env("CW_SESSION_ID", format!("wrap_{}", Uuid::new_v4().simple()));
            }
            let status = process.status()?;
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
                (turn.token_usage.input_tokens as f64
                    / turn.token_usage.context_limit.max(1) as f64)
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
                if fix && let Some(op) = &f.suggested_mutation {
                    save_cli_mutation(&store, &sess_id, op.clone(), false)?;
                    println!("    ✓ Applied suggested fix: {:?}", op);
                }
            }
        }
        Commands::Optimize {
            level,
            strategy,
            dry_run,
            explain,
            draft,
            session,
            all,
            project,
            agent,
            sync_back,
        } => {
            let store = CwStore::open(&db_path)?;
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
                            let _ = recompute_latest_turn_in_store(&store, &s.id);
                            if sync_back
                                && sync_mutations_to_transcript_on_disk(&store, &s.id).is_ok()
                            {
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

            let Some(sess_id) = resolve_session(&store, session)? else {
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
                println!(
                    "│ ⚡ contextWatcher Optimization & Cut Studio — Session: {sess_id:<33} │"
                );
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
                    let updated_turn = recompute_latest_turn_in_store(&store, &sess_id);
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
                        let report = sync_mutations_to_transcript_on_disk(&store, &sess_id)?;
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
            save_cli_mutation(
                &store,
                &sess_id,
                MutationOp::ShadowDedupe { file_path: file },
                false,
            )?;
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
                let ids = import_opencode_state_db(&path, &store, 20)?;
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
        }
        Commands::Local { session, model } => {
            let store = CwStore::open(&db_path)?;
            let Some(sess_id) = resolve_session(&store, session)? else {
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
            println!(
                "│ 🖥️  contextWatcher Local LLM & KV-Cache Slot Studio — Session: {sess_id:<25} │"
            );
            println!(
                "└──────────────────────────────────────────────────────────────────────────────────────────┘"
            );
            println!(
                "  • Engine & Model           : {} ({})",
                loc.model_name, loc.engine_kind
            );
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
        Commands::Autopilot {
            enable,
            disable,
            min_health,
            strategy,
            sync_to_disk,
        } => {
            let store = CwStore::open(&db_path)?;
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
                cfg.auto_heal_low_health,
                cfg.heal_strategy,
                cfg.min_health_score,
                cfg.auto_sync_to_disk
            );
        }
        Commands::Gain { history } => {
            let store = CwStore::open(&db_path)?;
            run_gain_report(&store, history)?;
        }
        Commands::Pulse { session, limit } => {
            let store = CwStore::open(&db_path)?;
            if let Some(sess_target) = session {
                let Some(sess_id) = resolve_session(&store, Some(sess_target))? else {
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
        }
        Commands::Swarm {
            project,
            model,
            min_members,
            json,
        } => {
            let store = CwStore::open(&db_path)?;
            run_swarm_report(&store, project, model, min_members, json)?;
        }
        Commands::Slot {
            session,
            engine,
            slot_id,
            splice_range,
            json,
        } => {
            let store = CwStore::open(&db_path)?;
            run_slot_bridge_report(&store, session, engine, slot_id, splice_range, json).await?;
        }
        Commands::Hud {
            session,
            compact,
            ecg,
            json,
        } => {
            let store = CwStore::open(&db_path)?;
            run_hud_report(&store, session, compact, ecg, json)?;
        }
        Commands::Init { dry_run, no_start } => {
            run_init_installer(dry_run, !no_start)?;
        }
        Commands::Hook { quiet, session } => {
            let store = CwStore::open(&db_path)?;
            run_hook_check(&store, quiet, session)?;
        }
        Commands::InstallIntegrations => {
            run_init_installer(false, false)?;
        }
    }

    Ok(())
}

fn resolve_session(store: &CwStore, explicit: Option<String>) -> Result<Option<String>> {
    let selected = explicit
        .or_else(|| std::env::var("CW_SESSION_ID").ok())
        .filter(|s| !s.trim().is_empty());
    if let Some(id) = selected {
        anyhow::ensure!(store.get_session(&id)?.is_some(), "Session not found: {id}");
        return Ok(Some(id));
    }
    let cwd = std::env::current_dir()?;
    let sessions = store.list_sessions()?;
    let matching: Vec<_> = sessions
        .iter()
        .filter(|s| {
            s.workspace_root
                .as_deref()
                .is_some_and(|root| std::path::Path::new(root) == cwd)
        })
        .collect();
    match matching.as_slice() {
        [] => Ok(None),
        [session] => Ok(Some(session.id.clone())),
        _ => anyhow::bail!("Several sessions match this workspace. Pass --session explicitly."),
    }
}

fn save_cli_mutation(store: &CwStore, session_id: &str, op: MutationOp, draft: bool) -> Result<()> {
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

fn project_label_from_workspace(ws: Option<&str>) -> String {
    let Some(w) = ws.map(str::trim).filter(|s| !s.is_empty()) else {
        return "General Chat".to_string();
    };
    w.trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(w)
        .to_string()
}

fn run_gain_report(store: &CwStore, show_history: bool) -> Result<()> {
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

fn run_swarm_report(
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

async fn run_slot_bridge_report(
    store: &CwStore,
    session: Option<String>,
    engine: Option<String>,
    slot_id_filter: Option<u32>,
    splice_range_str: Option<String>,
    output_json: bool,
) -> Result<()> {
    let upstream_url = engine
        .or_else(|| std::env::var("CW_LLAMACPP_UPSTREAM").ok())
        .unwrap_or_else(|| "http://127.0.0.1:8080".to_string());

    let target_session_id = resolve_session(store, session)?;
    let target_turn = match &target_session_id {
        Some(sid) => store.get_latest_turn(sid)?,
        None => None,
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(2500))
        .build()
        .unwrap_or_default();

    let slots_url = format!("{}/slots", upstream_url.trim_end_matches('/'));
    let mut req_b = client.get(&slots_url);
    if let Ok(key) = std::env::var("CW_LLAMACPP_API_KEY") {
        req_b = req_b.header("Authorization", format!("Bearer {key}"));
    }

    let mut upstream_slots = Vec::new();
    let upstream_reachable = match req_b.send().await {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(slots) = resp.json::<Vec<serde_json::Value>>().await {
                upstream_slots = slots;
                true
            } else {
                false
            }
        }
        _ => false,
    };

    let sessions = store.list_sessions()?;
    let mut slots_info = Vec::new();

    if upstream_reachable && !upstream_slots.is_empty() {
        let total_slots = upstream_slots.len() as u32;
        for slot in upstream_slots {
            let id = slot.get("id").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            let n_ctx = slot.get("n_ctx").and_then(|v| v.as_u64()).unwrap_or(131072) as u32;
            let is_processing = slot
                .get("is_processing")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let n_prompt_tokens = slot
                .get("n_prompt_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;

            let bound_session_id = sessions
                .iter()
                .find(|s| assign_deterministic_slot(&s.id, total_slots) == id)
                .map(|s| s.id.clone());

            let allocated_blocks = n_prompt_tokens.div_ceil(DEFAULT_PAGED_ATTENTION_BLOCK_SIZE);
            let vram_usage_mb = (n_prompt_tokens as f32) * 0.25;

            slots_info.push(EngineSlotInfo {
                id,
                n_ctx,
                is_processing,
                n_prompt_tokens,
                bound_session_id,
                allocated_blocks,
                vram_usage_mb,
            });
        }
    } else {
        let total_slots = 4u32;
        for id in 0..total_slots {
            let bound_session = sessions
                .iter()
                .find(|s| assign_deterministic_slot(&s.id, total_slots) == id);
            let (n_prompt_tokens, bound_id) = if let Some(s) = bound_session {
                let tok = store
                    .get_latest_turn(&s.id)
                    .ok()
                    .flatten()
                    .map(|t| t.token_usage.input_tokens)
                    .unwrap_or(0);
                (tok, Some(s.id.clone()))
            } else {
                (0, None)
            };

            let allocated_blocks = n_prompt_tokens.div_ceil(DEFAULT_PAGED_ATTENTION_BLOCK_SIZE);
            let vram_usage_mb = (n_prompt_tokens as f32) * 0.25;

            slots_info.push(EngineSlotInfo {
                id,
                n_ctx: 131072,
                is_processing: false,
                n_prompt_tokens,
                bound_session_id: bound_id,
                allocated_blocks,
                vram_usage_mb,
            });
        }
    }

    let mut splice_plan: Option<SlotSplicingPlan> = None;
    if let Some(range_str) = splice_range_str {
        let parts: Vec<&str> = range_str.split("..").collect();
        if parts.len() == 2
            && let (Ok(start), Ok(end)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>())
        {
            let target_slot = slot_id_filter.unwrap_or(0);
            let total_tok = target_turn
                .as_ref()
                .map(|t| t.token_usage.input_tokens)
                .unwrap_or(end.max(2048));
            let mut locked_prefix = 256u32;
            if let Some(ref t) = target_turn {
                for n in &t.nodes {
                    match n.kind {
                        cw_core::ir::NodeKind::SystemPrompt { .. }
                        | cw_core::ir::NodeKind::InjectedRule { .. }
                        | cw_core::ir::NodeKind::SkillCatalog { .. }
                        | cw_core::ir::NodeKind::McpToolSchema { .. } => {
                            locked_prefix += n.token_span.count;
                        }
                        _ => {}
                    }
                }
            }
            splice_plan = Some(plan_slot_splicing(
                target_slot,
                total_tok,
                locked_prefix,
                (start, end),
                DEFAULT_PAGED_ATTENTION_BLOCK_SIZE,
            ));
        }
    }

    if output_json {
        let out = serde_json::json!({
            "upstream_url": upstream_url,
            "upstream_reachable": upstream_reachable,
            "block_size": DEFAULT_PAGED_ATTENTION_BLOCK_SIZE,
            "slots": slots_info,
            "splice_plan": splice_plan,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    println!("\n🧩 contextWatcher Engine-Level PagedAttention & Slot Bridge (Horizon 3)");
    println!(
        "════════════════════════════════════════════════════════════════════════════════════"
    );
    println!(
        "Upstream Engine : {} ({})",
        upstream_url,
        if upstream_reachable {
            "🟢 Online"
        } else {
            "⚪ Simulated/Offline"
        }
    );
    println!(
        "PagedAttention  : {} tokens / block (16-token page granularity)",
        DEFAULT_PAGED_ATTENTION_BLOCK_SIZE
    );
    if let Some(ref sid) = target_session_id {
        println!("Target Session  : {}", sid);
    }
    println!("────────────────────────────────────────────────────────────────────────────────");
    println!(
        "  {:<6} {:<10} {:>12} {:>14} {:>12}  {:<26}",
        "SLOT", "STATUS", "TOKENS", "PAGES (16t)", "VRAM (MB)", "BOUND SESSION"
    );
    println!("  {}", "─".repeat(84));

    for s in &slots_info {
        if let Some(target_slot) = slot_id_filter
            && s.id != target_slot
        {
            continue;
        }
        let status_str = if s.is_processing {
            "🟡 Busy"
        } else {
            "🟢 Idle"
        };
        let bound_str = s.bound_session_id.as_deref().unwrap_or("<unbound>");
        println!(
            "  #{:<5} {:<10} {:>12} {:>14} {:>11.1}M  {:<26}",
            s.id, status_str, s.n_prompt_tokens, s.allocated_blocks, s.vram_usage_mb, bound_str
        );
    }

    if let Some(ref plan) = splice_plan {
        println!(
            "\n✂️  PAGEDATTENTION SPLICING SIMULATION (Range: {}..{} tok):",
            plan.target_token_range.0, plan.target_token_range.1
        );
        println!("  Target Slot              : #{}", plan.slot_id);
        println!(
            "  Clean Pages Evicted      : {} blocks ({} tokens)",
            plan.clean_blocks_evicted,
            plan.clean_blocks_evicted * plan.block_size
        );
        println!(
            "  Partial Boundary Slack   : {} tokens (sub-block remainder)",
            plan.partial_boundary_slack_tokens
        );
        println!(
            "  GPU VRAM Saved           : {:.1} MB",
            plan.estimated_vram_saved_mb
        );
        println!(
            "  Prefill Avoided Latency  : ~{} ms (zero-recompute speedup)",
            plan.prefill_recompute_avoided_ms
        );
        match &plan.action {
            SlotSplicingAction::PhysicalPageUnmap {
                blocks_freed,
                tokens_freed,
                vram_freed_mb,
            } => {
                println!(
                    "  Action Result            : 🟢 PhysicalPageUnmap: unmapped {} physical GPU pages ({} tok, {:.1} MB)",
                    blocks_freed, tokens_freed, vram_freed_mb
                );
            }
            SlotSplicingAction::TailTruncation {
                new_n_past,
                tokens_dropped,
            } => {
                println!(
                    "  Action Result            : 🟡 TailTruncation: set n_past to {} (dropped {} tail tokens)",
                    new_n_past, tokens_dropped
                );
            }
            SlotSplicingAction::BlockAlignmentPad {
                original_range,
                aligned_range,
                slack_tokens,
            } => {
                println!(
                    "  Action Result            : 🔵 BlockAlignmentPad: estimated expansion {:?} -> {:?} (+{} tok); physical unmap unsupported",
                    original_range, aligned_range, slack_tokens
                );
            }
            SlotSplicingAction::SubBlockNoop {
                unaligned_tokens,
                reason,
            } => {
                println!(
                    "  Action Result            : ⚪ SubBlockNoop: {} ({} tokens)",
                    reason, unaligned_tokens
                );
            }
        }
    }

    println!(
        "\n💡 Tip: When pruning intermediate nodes, cw splices physical PagedAttention blocks without invalidating the cached prefix!\n"
    );
    Ok(())
}

fn run_hud_report(
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

fn run_hook_check(store: &CwStore, quiet: bool, explicit_session: Option<String>) -> Result<()> {
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

fn is_daemon_running(port: u16) -> bool {
    use std::net::TcpStream;
    use std::time::Duration;
    if let Ok(addr) = format!("127.0.0.1:{port}").parse() {
        TcpStream::connect_timeout(&addr, Duration::from_millis(150)).is_ok()
    } else {
        false
    }
}

fn start_daemon_detached(port: u16, proxy_port: u16, workspace: Option<String>) -> Result<()> {
    if is_daemon_running(port) {
        println!("contextWatcher daemon is already running in background.");
        println!("  • Web UI : http://127.0.0.1:{port}");
        println!("  • Proxy  : http://127.0.0.1:{proxy_port}");
        return Ok(());
    }

    // Try systemd user service first if available on Linux
    let has_systemctl = std::process::Command::new("systemctl")
        .args(["--user", "is-enabled", "contextwatcher.service"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if has_systemctl {
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "start", "contextwatcher.service"])
            .status();
        std::thread::sleep(std::time::Duration::from_millis(300));
        if is_daemon_running(port) {
            println!("⚡ Started contextWatcher background daemon via systemd user service.");
            println!("   • Web UI : http://127.0.0.1:{port}");
            println!("   • Proxy  : http://127.0.0.1:{proxy_port}");
            return Ok(());
        }
    }

    // Fallback: spawn detached child process with ~/.cw/cw.log
    let cw_dir = cw_core::home_dir().join(".cw");
    std::fs::create_dir_all(&cw_dir)?;
    let log_path = cw_dir.join("cw.log");
    let pid_path = cw_dir.join("cw.pid");

    let exe = std::env::current_exe()?;
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    let err_file = log_file.try_clone()?;

    let mut cmd = std::process::Command::new(exe);
    cmd.args([
        "serve",
        "--port",
        &port.to_string(),
        "--proxy-port",
        &proxy_port.to_string(),
    ]);
    if let Some(ws) = workspace {
        cmd.args(["--workspace", &ws]);
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(log_file))
        .stderr(std::process::Stdio::from(err_file));

    let child = cmd.spawn()?;
    let pid = child.id();
    std::fs::write(&pid_path, pid.to_string())?;

    for _ in 0..15 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        if is_daemon_running(port) {
            break;
        }
    }

    if is_daemon_running(port) {
        println!("⚡ Started contextWatcher daemon in background (PID: {pid})");
        println!("   • Web UI : http://127.0.0.1:{port}");
        println!("   • Proxy  : http://127.0.0.1:{proxy_port}");
        println!("   • Logs   : {}", log_path.display());
    } else {
        println!(
            "⚠️ Daemon process spawned (PID: {pid}), waiting for port {port}. Logs: {}",
            log_path.display()
        );
    }

    Ok(())
}

fn stop_daemon(port: u16) -> Result<()> {
    // 1. Try systemd user service stop
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "stop", "contextwatcher.service"])
        .status();

    // 2. Also check ~/.cw/cw.pid
    let pid_path = cw_core::home_dir().join(".cw/cw.pid");
    if let Ok(pid_str) = std::fs::read_to_string(&pid_path) {
        let p = pid_str.trim();
        #[cfg(windows)]
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/PID", p])
            .status();
        #[cfg(not(windows))]
        let _ = std::process::Command::new("kill").arg(p).status();
        let _ = std::fs::remove_file(&pid_path);
    }

    std::thread::sleep(std::time::Duration::from_millis(200));
    if !is_daemon_running(port) {
        println!("✓ contextWatcher background daemon stopped.");
    } else {
        println!("⚠️ Daemon still active on port {port}.");
    }
    Ok(())
}

fn status_daemon(port: u16) -> Result<()> {
    let running = is_daemon_running(port);
    let pid_path = cw_core::home_dir().join(".cw/cw.pid");
    let pid = std::fs::read_to_string(&pid_path)
        .ok()
        .map(|s| s.trim().to_string());

    if running {
        println!("🟢 contextWatcher daemon is RUNNING (Web UI: http://127.0.0.1:{port})");
        if let Some(p) = pid {
            println!("   PID: {p}");
        }
    } else {
        println!("🔴 contextWatcher daemon is NOT running.");
        println!("   Run `cw daemon start` to start it in the background.");
    }
    Ok(())
}

/// Registers the `contextwatcher` MCP server for Claude Code at user scope.
///
/// Claude Code reads user-scoped MCP servers from `~/.claude.json` (top-level `mcpServers`),
/// not from `~/.claude/settings.json`. Prefer the official `claude mcp add` CLI; if it is not
/// on PATH, merge the entry into `~/.claude.json` directly, keeping a backup.
fn register_claude_code_mcp(home_path: &std::path::Path, cw_cmd_str: &str) {
    let via_cli = std::process::Command::new("claude")
        .args(["mcp", "remove", "--scope", "user", "contextwatcher"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
        && std::process::Command::new("claude")
            .args([
                "mcp",
                "add",
                "--scope",
                "user",
                "contextwatcher",
                "--",
                cw_cmd_str,
                "mcp",
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
    if via_cli {
        return;
    }

    let config_path = home_path.join(".claude.json");
    let mut root: serde_json::Value = match std::fs::read_to_string(&config_path) {
        Ok(s) => match serde_json::from_str(&s) {
            Ok(v) => v,
            // Never overwrite a config we could not parse.
            Err(_) => return,
        },
        Err(_) => serde_json::json!({}),
    };
    let Some(obj) = root.as_object_mut() else {
        return;
    };
    let servers = obj
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));
    let Some(servers) = servers.as_object_mut() else {
        return;
    };
    servers.insert(
        "contextwatcher".to_string(),
        serde_json::json!({ "type": "stdio", "command": cw_cmd_str, "args": ["mcp"], "env": {} }),
    );
    if config_path.exists() {
        let _ = std::fs::copy(&config_path, home_path.join(".claude.json.cw.bak"));
    }
    if let Ok(pretty) = serde_json::to_string_pretty(&root) {
        let tmp = home_path.join(".claude.json.cw.tmp");
        if std::fs::write(&tmp, format!("{pretty}\n")).is_ok() {
            let _ = std::fs::rename(&tmp, &config_path);
        }
    }
}

fn run_init_installer(dry_run: bool, auto_start: bool) -> Result<()> {
    let home_path = cw_core::home_dir();
    let current_exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("cw"));
    let local_bin_dir = home_path.join(".local/bin");
    let installed_cw_bin = local_bin_dir.join("cw");

    println!("┌──────────────────────────────────────────────────────────────────────────────┐");
    println!(
        "│ contextWatcher (`cw init`) — Multi-Harness Plugin & Hook Installer {}│",
        if dry_run { "(DRY RUN)" } else { "         " }
    );
    println!("└──────────────────────────────────────────────────────────────────────────────┘");

    // 1. Install `cw` binary into ~/.local/bin/cw
    if !dry_run {
        let _ = std::fs::create_dir_all(&local_bin_dir);
        if current_exe != installed_cw_bin && current_exe.exists() {
            let tmp_bin = local_bin_dir.join("cw.tmp");
            if std::fs::copy(&current_exe, &tmp_bin).is_ok() {
                let _ = std::fs::rename(&tmp_bin, &installed_cw_bin);
            }
        }
    }
    let cw_cmd_str = if installed_cw_bin.exists() || !dry_run {
        installed_cw_bin.to_string_lossy().to_string()
    } else {
        current_exe.to_string_lossy().to_string()
    };
    println!("  [1/5] Global CLI Binary      : {}", cw_cmd_str);

    // 2. Antigravity Plugin (~/.gemini/config/plugins/contextwatcher/)
    let agy_plugin_dir = home_path.join(".gemini/config/plugins/contextwatcher");
    let agy_skill_dir = agy_plugin_dir.join("skills/context-watcher");
    let agy_skill_cw_dir = agy_plugin_dir.join("skills/cw");
    let agy_global_skill_dir = home_path.join(".gemini/config/skills/context-watcher");
    let agy_global_skill_cw_dir = home_path.join(".gemini/config/skills/cw");
    let agy_commands_dir = agy_plugin_dir.join("commands");
    let skill_md = r#"---
name: context-watcher
description: Inspects and compacts the active LLM context window, KV cache prefix, stale file reads, and noisy tool outputs via contextWatcher (`cw`). Use when context exceeds 80k tokens, health score drops, or the user asks to optimize/compact context.
---

# contextWatcher (`cw`) — Context & KV-Cache Optimization Skill

When invoked for `/context-watcher` or `/cw` with any subcommand or level (`min`, `mid`, `max`, `status`, `gain`):

**Execute the exact command directly using the shell/bash tool immediately on turn 1. Do not perform preliminary environment checks, do not search for config files, and do not run `which cw` or `--help`.**

### Direct Shell Commands:
- **Optimization:**
  - `min`: `~/.local/bin/cw optimize -s min --sync-back` (0-KV-break uncached duplicate reads/tail bloat + rule reminders; use `clean_code_state` for full covered read deduplication)
  - `mid`: `~/.local/bin/cw optimize -s mid --sync-back` (compacts old tool/command outputs and unsigned thinking blocks)
  - `max`: `~/.local/bin/cw optimize -s max --sync-back` (combines covered read deduplication, old tool/thinking compaction, and rule reminders; use `cw_fold_turns` with an explicit summary to fold turns)
- **Inspection:**
  - `status`: `~/.local/bin/cw status`
  - `gain`: `~/.local/bin/cw gain`

If the `cw_optimize_strategy` MCP tool is directly available in your active tool list, you may call it with `{"strategy_id": "<level>", "export_compacted": true}` (which applies the session overlay and exports a separate compacted `.jsonl` copy without modifying the source transcript in-place). Otherwise, execute the shell command above directly.
"#;

    let skill_cw_md = r#"---
name: cw
description: Inspects and compacts the active LLM context window, KV cache prefix, stale file reads, and noisy tool outputs via contextWatcher (`cw`). Use when context exceeds 80k tokens, health score drops, or the user asks to optimize/compact context.
---

# contextWatcher (`cw`) — Context & KV-Cache Optimization Skill

When invoked for `/cw` or `/context-watcher` with any subcommand or level (`min`, `mid`, `max`, `status`, `gain`):

**Execute the exact command directly using the shell/bash tool immediately on turn 1. Do not perform preliminary environment checks, do not search for config files, and do not run `which cw` or `--help`.**

### Direct Shell Commands:
- **Optimization:**
  - `min`: `~/.local/bin/cw optimize -s min --sync-back` (0-KV-break uncached duplicate reads/tail bloat + rule reminders; use `clean_code_state` for full covered read deduplication)
  - `mid`: `~/.local/bin/cw optimize -s mid --sync-back` (compacts old tool/command outputs and unsigned thinking blocks)
  - `max`: `~/.local/bin/cw optimize -s max --sync-back` (combines covered read deduplication, old tool/thinking compaction, and rule reminders; use `cw_fold_turns` with an explicit summary to fold turns)
- **Inspection:**
  - `status`: `~/.local/bin/cw status`
  - `gain`: `~/.local/bin/cw gain`

If the `cw_optimize_strategy` MCP tool is directly available in your active tool list, you may call it with `{"strategy_id": "<level>", "export_compacted": true}` (which applies the session overlay and exports a separate compacted `.jsonl` copy without modifying the source transcript in-place). Otherwise, execute the shell command above directly.
"#;

    let agy_mcp_json = serde_json::to_string_pretty(&serde_json::json!({
        "mcpServers": {
            "contextwatcher": {
                "command": cw_cmd_str,
                "args": ["mcp"],
                "cwd": "",
                "env": null
            }
        }
    }))?;

    if !dry_run {
        let _ = std::fs::create_dir_all(&agy_skill_dir);
        let _ = std::fs::create_dir_all(&agy_skill_cw_dir);
        let _ = std::fs::create_dir_all(&agy_global_skill_dir);
        let _ = std::fs::create_dir_all(&agy_global_skill_cw_dir);
        let _ = std::fs::create_dir_all(&agy_commands_dir);
        let _ = std::fs::write(
            agy_plugin_dir.join("plugin.json"),
            "{\n  \"name\": \"contextwatcher\"\n}\n",
        );
        let _ = std::fs::write(agy_plugin_dir.join("mcp_config.json"), &agy_mcp_json);
        let _ = std::fs::write(agy_skill_dir.join("SKILL.md"), skill_md);
        let _ = std::fs::write(agy_skill_cw_dir.join("SKILL.md"), skill_cw_md);
        let _ = std::fs::write(agy_global_skill_dir.join("SKILL.md"), skill_md);
        let _ = std::fs::write(agy_global_skill_cw_dir.join("SKILL.md"), skill_cw_md);
        let _ = std::fs::write(
            agy_commands_dir.join("cw.toml"),
            "description = \"Run contextWatcher compaction: /cw optimize min|mid|max, /cw gain, or /cw status\"\n\
             prompt = \"Execute contextWatcher optimization with intensity {{args}} (min, mid, or max). Call tool cw_optimize_strategy with strategy_id: '{{args}}' and export_compacted: true to apply session overlays and export a compacted copy.\"\n",
        );
        let _ = std::fs::write(
            agy_commands_dir.join("context-watcher.toml"),
            "description = \"Run contextWatcher compaction: /context-watcher optimize min|mid|max, gain, or status\"\n\
             prompt = \"Execute contextWatcher optimization with intensity {{args}} (min, mid, or max). Call tool cw_optimize_strategy with strategy_id: '{{args}}' and export_compacted: true to apply session overlays and export a compacted copy.\"\n",
        );
        let _ = std::fs::write(
            agy_commands_dir.join("cw-gain.toml"),
            "description = \"Show contextWatcher token and dollar ($) savings scoreboard\"\n\
             prompt = \"Run the shell command `cw gain` using run_command and display the savings scoreboard.\"\n",
        );
    }
    println!(
        "  [2/6] Antigravity Plugin     : {} (MCP + /context-watcher & /cw slash commands)",
        agy_plugin_dir.display()
    );

    // 3. Codex CLI (~/.codex/config.toml + ~/.codex/CW.md + ~/.codex/skills/context-watcher/ & skills/cw/)
    let codex_dir = home_path.join(".codex");
    if codex_dir.exists() {
        let codex_cfg = codex_dir.join("config.toml");
        let codex_cw_md = codex_dir.join("CW.md");
        let codex_agents_md = codex_dir.join("AGENTS.md");
        let codex_skill_cw_dir = codex_dir.join("skills/cw");
        let codex_skill_full_dir = codex_dir.join("skills/context-watcher");

        let cw_md_content = format!(
            "# contextWatcher (`cw`) — Context & KV-Cache Optimizer\n\n\
             `cw` monitors and compacts context bloat (stale file reads, old tool outputs, reasoning blocks) across turns.\n\n\
             ## Commands & MCP Tools\n\n\
             - Slash commands: `/context-watcher optimize min|mid|max` or `/cw optimize min|mid|max`, `/cw gain`, `/cw status`\n\
             - MCP tools: `cw_context_status`, `cw_optimize_strategy`, `cw_evict_stale_reads`, `cw_fold_turns`, `cw_pin_context`\n\
             - CLI commands: `{cw_cmd_str} gain`, `{cw_cmd_str} status`, `{cw_cmd_str} optimize min|mid|max`\n"
        );

        if !dry_run {
            let _ = std::fs::create_dir_all(&codex_skill_cw_dir);
            let _ = std::fs::create_dir_all(&codex_skill_full_dir);
            let _ = std::fs::write(&codex_cw_md, cw_md_content);
            let _ = std::fs::write(codex_skill_cw_dir.join("SKILL.md"), skill_cw_md);
            let _ = std::fs::write(codex_skill_full_dir.join("SKILL.md"), skill_md);

            // Also install into ~/.agents/skills if the folder exists (scanned by Codex)
            let shared_agents_skills = home_path.join(".agents/skills");
            if shared_agents_skills.exists() {
                let _ = std::fs::create_dir_all(shared_agents_skills.join("cw"));
                let _ = std::fs::create_dir_all(shared_agents_skills.join("context-watcher"));
                let _ = std::fs::write(shared_agents_skills.join("cw/SKILL.md"), skill_cw_md);
                let _ = std::fs::write(
                    shared_agents_skills.join("context-watcher/SKILL.md"),
                    skill_md,
                );
            }
            if codex_cfg.exists()
                && let Ok(existing) = std::fs::read_to_string(&codex_cfg)
                && !existing.contains("[mcp_servers.contextwatcher]")
            {
                let _ = std::fs::copy(&codex_cfg, codex_dir.join("config.toml.cw.bak"));
                let appended = format!(
                    "{existing}\n\n[mcp_servers.contextwatcher]\ncommand = \"{cw_cmd_str}\"\nargs = [\"mcp\"]\n"
                );
                let _ = std::fs::write(&codex_cfg, appended);
            }
            if codex_agents_md.exists()
                && let Ok(existing_agents) = std::fs::read_to_string(&codex_agents_md)
            {
                let ref_line = format!("@{}", codex_cw_md.display());
                if !existing_agents.contains(&ref_line) {
                    let updated = format!("{}\n{}\n", existing_agents.trim_end(), ref_line);
                    let _ = std::fs::write(&codex_agents_md, updated);
                }
            }
        }
        println!(
            "  [3/6] Codex CLI Integration  : {} ([mcp_servers.contextwatcher] + /context-watcher & /cw skills)",
            codex_cfg.display()
        );
    }

    // 4. Claude Code (~/.claude/settings.json + commands/context-watcher.md & cw.md)
    let claude_dir = home_path.join(".claude");
    if claude_dir.exists() {
        let settings_path = claude_dir.join("settings.json");
        let commands_dir = claude_dir.join("commands");
        let skills_full_dir = claude_dir.join("skills/context-watcher");
        let skills_cw_dir = claude_dir.join("skills/cw");
        if !dry_run {
            let _ = std::fs::create_dir_all(&commands_dir);
            let _ = std::fs::create_dir_all(&skills_full_dir);
            let _ = std::fs::create_dir_all(&skills_cw_dir);

            let claude_cmd_content = "# /cw [optimize min|mid|max | gain | status]\n\n\
                 Execute contextWatcher optimization with arguments: `$ARGUMENTS`.\n\
                 - If `$ARGUMENTS` starts with `optimize` or `opt` or is `min`, `mid`, or `max`: call tool `cw_optimize_strategy` with `strategy_id` set to the level (defaulting to `\"mid\"`) and `export_compacted: true`.\n\
                 - If `$ARGUMENTS` is `gain`: run `cw gain` via Bash.\n\
                 - If `$ARGUMENTS` is `status`: call tool `cw_context_status`.\n";

            let _ = std::fs::write(commands_dir.join("cw.md"), claude_cmd_content);
            let _ = std::fs::write(commands_dir.join("context-watcher.md"), claude_cmd_content);
            let _ = std::fs::write(skills_full_dir.join("SKILL.md"), skill_md);
            let _ = std::fs::write(skills_cw_dir.join("SKILL.md"), skill_cw_md);

            register_claude_code_mcp(&home_path, &cw_cmd_str);

            // Earlier versions wrote `mcpServers.contextwatcher` into settings.json, which
            // Claude Code does not read MCP servers from. Remove that stale entry if present.
            if settings_path.exists()
                && let Some(mut root) = std::fs::read_to_string(&settings_path)
                    .ok()
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            {
                let mut changed = false;
                if let Some(obj) = root.as_object_mut() {
                    let should_remove_mcps = if let Some(mcp) =
                        obj.get_mut("mcpServers").and_then(|v| v.as_object_mut())
                    {
                        changed = mcp.remove("contextwatcher").is_some();
                        mcp.is_empty()
                    } else {
                        false
                    };
                    if should_remove_mcps {
                        obj.remove("mcpServers");
                    }
                }
                if changed {
                    let _ = std::fs::copy(&settings_path, claude_dir.join("settings.json.cw.bak"));
                    if let Ok(pretty) = serde_json::to_string_pretty(&root) {
                        let _ = std::fs::write(&settings_path, format!("{pretty}\n"));
                    }
                }
            }
        }
        println!(
            "  [4/6] Claude Code Config     : {} (user-scope MCP server `contextwatcher` + /context-watcher & /cw slash commands)",
            home_path.join(".claude.json").display()
        );
    }

    // 5. Hermes Agent (~/.hermes/skills/context-watcher/ & skills/cw/)
    let hermes_dir = home_path.join(".hermes");
    if hermes_dir.exists() {
        let hermes_skill_cw_dir = hermes_dir.join("skills/cw");
        let hermes_skill_full_dir = hermes_dir.join("skills/context-watcher");
        let hermes_plugin_dir = hermes_dir.join("plugins/context-watcher");
        if !dry_run {
            let _ = std::fs::create_dir_all(&hermes_skill_cw_dir);
            let _ = std::fs::create_dir_all(&hermes_skill_full_dir);
            let _ = std::fs::write(hermes_skill_cw_dir.join("SKILL.md"), skill_cw_md);
            let _ = std::fs::write(hermes_skill_full_dir.join("SKILL.md"), skill_md);
            std::fs::create_dir_all(&hermes_plugin_dir)?;
            std::fs::write(
                hermes_plugin_dir.join("plugin.yaml"),
                include_str!("../../../plugins/hermes/context-watcher/plugin.yaml"),
            )?;
            std::fs::write(
                hermes_plugin_dir.join("__init__.py"),
                include_str!("../../../plugins/hermes/context-watcher/__init__.py"),
            )?;
            if std::process::Command::new("hermes")
                .args([
                    "plugins",
                    "enable",
                    "context-watcher",
                    "--no-allow-tool-override",
                ])
                .status()
                .is_ok_and(|status| status.success())
            {
                println!("           Hermes contextWatcher middleware enabled for new sessions.");
            } else {
                println!(
                    "           Run `hermes plugins enable context-watcher` to activate proxy middleware."
                );
            }
            println!(
                "           Launch with `cw wrap hermes chat` (resume with `cw wrap hermes chat --resume <id>`); direct `hermes` calls bypass the cw proxy."
            );
        }
        println!(
            "  [5/6] Hermes Skill + Plugin  : {} (/context-watcher & /cw; enable proxy middleware)",
            hermes_skill_full_dir.display()
        );
    }

    // 6. Native Pi & OMP Agent (~/.pi/agent/settings.json + ~/.omp/agent/skills/)
    let pi_agent_dir = home_path.join(".pi/agent");
    let omp_agent_dir = home_path.join(".omp/agent");
    if pi_agent_dir.exists() || omp_agent_dir.exists() {
        let pi_ext_dir = home_path.join("Development/contextWatcher/plugins/pi-opencode");
        if !dry_run && pi_ext_dir.exists() {
            let _ = std::process::Command::new("pi")
                .args(["install", &pi_ext_dir.to_string_lossy()])
                .status();
            let _ = std::process::Command::new("omp")
                .args(["plugin", "install", &pi_ext_dir.to_string_lossy()])
                .status();
        }
        if !dry_run && omp_agent_dir.exists() {
            let omp_skill_cw = omp_agent_dir.join("skills/cw");
            let omp_skill_full = omp_agent_dir.join("skills/context-watcher");
            let _ = std::fs::create_dir_all(&omp_skill_cw);
            let _ = std::fs::create_dir_all(&omp_skill_full);
            let _ = std::fs::write(omp_skill_cw.join("SKILL.md"), skill_cw_md);
            let _ = std::fs::write(omp_skill_full.join("SKILL.md"), skill_md);
        }
        println!(
            "  [6/6] Native Pi & OMP Agent  : {} (Native /context-watcher & /cw commands registered)",
            pi_ext_dir.display()
        );
    }

    // 7. OpenCode (~/.config/opencode/config.json or ~/.opencode/config.json)
    let opencode_cfg_dir = home_path.join(".config/opencode");
    let opencode_alt_dir = home_path.join(".opencode");
    let opencode_data_dir = home_path.join(".local/share/opencode");
    if opencode_cfg_dir.exists() || opencode_alt_dir.exists() || opencode_data_dir.exists() {
        let target_cfg = if opencode_alt_dir.exists() {
            opencode_alt_dir.join("config.json")
        } else {
            opencode_cfg_dir.join("config.json")
        };
        if !dry_run {
            if let Some(parent) = target_cfg.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let mut cfg_val: serde_json::Value = std::fs::read_to_string(&target_cfg)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_else(|| serde_json::json!({}));
            if !cfg_val.is_object() {
                cfg_val = serde_json::json!({});
            }
            cfg_val["mcp"]["contextWatcher"] = serde_json::json!({
                "type": "local",
                "command": ["cw", "mcp", "--daemon-url", "http://127.0.0.1:4400"],
                "enabled": true
            });
            let _ = std::fs::write(&target_cfg, serde_json::to_string_pretty(&cfg_val).unwrap());
        }
        println!(
            "  [7/7] OpenCode Integration   : {} (MCP server registered)",
            target_cfg.display()
        );
    }

    // 5. Systemd User Service (~/.config/systemd/user/contextwatcher.service)
    let systemd_dir = home_path.join(".config/systemd/user");
    let service_path = systemd_dir.join("contextwatcher.service");
    let service_unit = format!(
        "[Unit]\n\
         Description=contextWatcher Daemon (Web UI :4400 + LLM Proxy :4401 + Transcript Watcher)\n\
         After=network.target\n\n\
         [Service]\n\
         Type=simple\n\
         ExecStart={cw_cmd_str} serve --port 4400 --proxy-port 4401\n\
         Restart=on-failure\n\
         RestartSec=3\n\n\
         [Install]\n\
         WantedBy=default.target\n"
    );
    if !dry_run {
        let _ = std::fs::create_dir_all(&systemd_dir);
        let _ = std::fs::write(&service_path, service_unit);
        if auto_start {
            let mut started = false;
            let has_systemctl = std::process::Command::new("systemctl")
                .args(["--user", "daemon-reload"])
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if has_systemctl {
                let _ = std::process::Command::new("systemctl")
                    .args(["--user", "enable", "--now", "contextwatcher.service"])
                    .status();
                std::thread::sleep(std::time::Duration::from_millis(200));
                if is_daemon_running(4400) {
                    started = true;
                }
            }
            if !started && !is_daemon_running(4400) {
                let _ = start_daemon_detached(4400, 4401, None);
            }
        }
    }
    println!(
        "  [5/5] Systemd User Service   : {}",
        service_path.display()
    );

    println!("\n✅ contextWatcher integration complete! Verify anytime with:");
    println!(
        "   cw gain              # Show token & dollar ($) savings across all agents & projects"
    );
    println!("   cw status            # Inspect latest active session health & KV cache hit ratio");
    println!("   cw optimize          # Run or preview the 5 compaction strategies");
    Ok(())
}

#[cfg(test)]
mod wrap_tests {
    use super::*;
    #[test]
    fn wrapper_removes_cw_credentials_from_harness_environment() {
        let headers = without_cw_headers(
            "x-existing: yes\nx-cw-session-id: forged\nX-CW-CONTROL-TOKEN: stale\nX-CW-PRIVATE: value",
        );
        assert_eq!(headers, "x-existing: yes");
        assert_eq!(
            without_cw_comma_separated_headers(
                "x-existing: yes, x-cw-session-id: forged, authorization: Bearer a,b, X-CW-PRIVATE: stale"
            ),
            "x-existing: yes, authorization: Bearer a,b"
        );
        assert!(valid_proxy_session_id("pi_session-12"));
        assert!(!valid_proxy_session_id("../other"));
        assert!(!valid_proxy_session_id(""));
    }

    #[test]
    fn codex_wrapper_uses_proxy_provider_and_environment_backed_headers() {
        let args = codex_proxy_config_args(5432);
        let settings = args.join(" ");
        assert!(settings.contains("model_provider=\"context_watcher\""));
        assert!(settings.contains("127.0.0.1:5432/openai/v1"));
        assert!(settings.contains("x-cw-session-id\"=\"CW_SESSION_ID"));
        assert!(settings.contains("x-cw-control-token\"=\"CW_CONTROL_TOKEN"));
        assert!(settings.contains("env_key=\"OPENAI_API_KEY\""));
        assert!(settings.contains("shell_environment_policy.filters.CW_CONTROL_TOKEN"));
        assert!(!settings.contains("private-token"));
    }
}
