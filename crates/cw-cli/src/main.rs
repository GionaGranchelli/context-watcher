use std::{path::PathBuf, process::Stdio};

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use cw_harnesses::{scan_and_import_all_transcripts_once, spawn_transcript_watcher};
use cw_mcp::run_mcp_stdio_server;
use cw_proxy::{DaemonState, build_api_router, build_proxy_router};
use cw_store::CwStore;
use cw_tui::run_tui;

mod commands;
mod update;

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
    /// Discover local harness transcripts without blocking the daemon
    #[command(hide = true)]
    Discover,
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
    /// Inspect, authorize, or revoke experimental PinContext reinforcement through the control API
    PinPolicy {
        #[arg(long)]
        session: String,
        #[arg(long, default_value = "http://127.0.0.1:4400")]
        daemon_url: String,
        #[command(subcommand)]
        action: PinPolicyAction,
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
        /// Loopback daemon used by the separate consent policy workspace
        #[arg(long, default_value = "http://127.0.0.1:4400")]
        daemon_url: String,
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
    /// Store a committed PinContext rule; reinforcement requires separate session consent
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
    /// Replay historical benchmark traces and calibrate shadow policy predictions (CW-117B/CW-117C)
    Calibrate {
        /// Optional path to historical benchmark dataset (defaults to committed cw115 datasets)
        #[arg(long)]
        dataset: Option<String>,
        /// Continuation horizon in turns (default: 3)
        #[arg(long, default_value_t = 3)]
        horizon: u32,
        /// Generate deterministic prospective synthetic qualification report (CW-117C)
        #[arg(long)]
        synthetic: bool,
        /// Correlate prospective decisions and outcomes from database session
        #[arg(long)]
        session: Option<String>,
        /// Output machine-readable JSON calibration report
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
    /// Check harness readiness, environment isolation, and live provider qualification preconditions (CW-119)
    Readiness {
        /// Specific harness to check (codex, omp, hermes, all)
        #[arg(long, default_value = "all")]
        harness: String,
        /// Output machine-readable JSON report
        #[arg(long)]
        json: bool,
    },
    /// Alias for `cw init`: install contextWatcher integrations for supported harnesses, including Hermes
    InstallIntegrations,
}

fn l0_smoke_spend_guard_from_environment() -> Result<Option<cw_core::spend_guard::SpendGuard>> {
    let budget = std::env::var("CW_L0_SMOKE_SPEND_CEILING_USD").ok();
    let request_cap = std::env::var("CW_L0_SMOKE_REQUEST_CAP").ok();
    l0_smoke_spend_guard(budget.as_deref(), request_cap.as_deref())
}

fn l0_smoke_spend_guard(
    budget: Option<&str>,
    request_cap: Option<&str>,
) -> Result<Option<cw_core::spend_guard::SpendGuard>> {
    use cw_core::spend_guard::{SpendGuard, SpendGuardConfig, parse_authorization};

    let (Some(budget), Some(request_cap)) = (budget, request_cap) else {
        if budget.is_some() || request_cap.is_some() {
            bail!("CW L0 smoke budget and request cap must be configured together");
        }
        return Ok(None);
    };
    let budget = parse_authorization(Some(budget))?;
    let request_cap = request_cap.parse::<usize>()?;
    if !(1..=8).contains(&request_cap) {
        bail!("CW L0 smoke request cap must be within 1..=8");
    }
    let config = SpendGuardConfig {
        authorized_budget_usd: budget,
        session_reservation_usd: budget,
        max_session_cost_usd: budget,
        max_requests_per_session: request_cap,
        request_reservation_usd: budget,
        ..Default::default()
    };
    Ok(Some(SpendGuard::new_with_campaign_request_limit(
        config,
        request_cap,
    )))
}

#[cfg(test)]
mod l0_smoke_guard_tests {
    use super::*;
    use cw_core::spend_guard::SpendGuardError;

    #[test]
    fn smoke_guard_is_absent_or_requires_both_bounded_values() {
        assert!(l0_smoke_spend_guard(None, None).unwrap().is_none());
        assert!(l0_smoke_spend_guard(Some("0.30"), None).is_err());
        assert!(l0_smoke_spend_guard(None, Some("1")).is_err());
        assert!(l0_smoke_spend_guard(Some("0"), Some("1")).is_err());
        assert!(l0_smoke_spend_guard(Some("0.30"), Some("9")).is_err());
    }

    #[test]
    fn smoke_guard_reserves_full_ceiling_and_shares_global_request_cap() {
        let guard = l0_smoke_spend_guard(Some("0.30"), Some("1"))
            .unwrap()
            .unwrap();
        assert_eq!(guard.config().request_reservation_usd, 0.30);
        let _first = guard.admit_request("session-a").unwrap();
        assert!(matches!(
            guard.admit_request("session-b"),
            Err(SpendGuardError::RequestLimitExceeded { count: 2, limit: 1 })
        ));
    }
}

#[derive(Subcommand)]
enum PinPolicyAction {
    /// Show consent, stored pin contents/status, request eligibility and future-generation scope
    Preview,
    /// Display the preview and full warning before explicitly enabling session reinforcement
    Enable {
        /// Acknowledge the displayed latency, cache, cost and behavior risks and session-wide scope
        #[arg(long)]
        acknowledge_risk: bool,
    },
    /// Revoke consent without deleting stored rules
    Disable,
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
            commands::session::run_purge(&store, &session, keep_turns)?;
        }
        Commands::Preview { session } => {
            let store = CwStore::open(&db_path)?;
            commands::session::run_preview(&store, &session)?;
        }
        Commands::PinPolicy {
            session,
            daemon_url,
            action,
        } => {
            commands::pin_policy::run(&session, &daemon_url, action).await?;
        }
        Commands::AuthToken => {
            println!("{}", cw_core::control::control_token(true)?);
        }
        Commands::Discover => {
            let store = CwStore::open(&db_path)?;
            let imported = scan_and_import_all_transcripts_once(&store);
            println!("Discovered local harness transcripts imported={imported}");
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
                commands::daemon::start_daemon_detached(port, proxy_port, workspace)?;
                return Ok(());
            }

            tracing_subscriber::fmt()
                .with_env_filter(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| "info".into()),
                )
                .init();

            let store = CwStore::open(&db_path)?;
            spawn_transcript_watcher(store.clone(), workspace.clone());

            let mut state = DaemonState::new(store.clone());
            state.control_token = cw_core::control::control_token(true)?;
            state.spend_guard = l0_smoke_spend_guard_from_environment()?;
            if state.spend_guard.is_some() != state.live_provider_model.is_some() {
                bail!("CW L0 smoke budget and model authorization must be configured together");
            }
            if state.spend_guard.is_some()
                && (state
                    .live_allowed_session_id
                    .as_deref()
                    .is_none_or(|id| !cw_core::control::is_valid_session_id(id))
                    || state.live_authorization_expires_at.is_none())
            {
                bail!("CW L0 smoke session binding and authorization expiry are required");
            }
            if state.spend_guard.is_some()
                && (!matches!(
                    state.live_provider_model.as_ref(),
                    Some((cw_core::ir::ProviderKind::OpenAI, model)) if model == "gpt-6-luna"
                ) || state.openai_upstream != "https://api.openai.com"
                    || state.openai_api_key.is_none()
                    || !state.capture_wire_egress
                    || !state.capture_wire_response)
            {
                bail!(
                    "CW L0 smoke requires the authorized OpenAI model, endpoint, private key, and complete wire capture"
                );
            }
            let proxy_app = build_proxy_router(state.clone());
            let ui_and_api_app =
                build_api_router(state).fallback(commands::web::serve_embedded_web_ui);

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
            if workspace.is_none() {
                let executable = std::env::current_exe()?;
                match std::process::Command::new(executable)
                    .arg("--db")
                    .arg(&db_path)
                    .arg("discover")
                    .stdin(Stdio::null())
                    .stdout(Stdio::inherit())
                    .stderr(Stdio::inherit())
                    .spawn()
                {
                    Ok(mut child) => {
                        tokio::task::spawn_blocking(move || {
                            if let Err(error) = child.wait() {
                                tracing::warn!(%error, "Could not reap transcript discovery worker");
                            }
                        });
                    }
                    Err(error) => {
                        tracing::warn!(%error, "Could not start local transcript discovery");
                    }
                }
            }

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
                commands::daemon::start_daemon_detached(port, proxy_port, workspace)?;
            }
            DaemonAction::Stop { port } => {
                commands::daemon::stop_daemon(port)?;
            }
            DaemonAction::Restart {
                port,
                proxy_port,
                workspace,
            } => {
                commands::daemon::restart_daemon(port, proxy_port, workspace)?;
            }
            DaemonAction::Status { port } => {
                commands::daemon::status_daemon(port)?;
            }
        },
        Commands::Wrap {
            proxy_port,
            session_id,
            cmd,
        } => {
            commands::wrap::run_wrap(proxy_port, session_id, cmd)?;
        }
        Commands::Tui {
            session,
            daemon_url,
        } => {
            let store = CwStore::open(&db_path)?;
            run_tui(store, session, &daemon_url).await?;
        }
        Commands::Status { session } => {
            let store = CwStore::open(&db_path)?;
            commands::session::run_status(&store, session, &db_path)?;
        }
        Commands::Tree { session } => {
            let store = CwStore::open(&db_path)?;
            commands::session::run_tree(&store, session)?;
        }
        Commands::Lint { session, fix } => {
            let store = CwStore::open(&db_path)?;
            commands::session::run_lint(&store, session, fix)?;
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
            commands::session::run_optimize(
                &store, level, strategy, dry_run, explain, draft, session, all, project, agent,
                sync_back,
            )?;
        }
        Commands::Fold {
            start_turn,
            end_turn,
            summary,
            session,
            draft,
        } => {
            let store = CwStore::open(&db_path)?;
            commands::session::run_fold(&store, start_turn, end_turn, summary, session, draft)?;
        }
        Commands::Prune {
            target,
            summary,
            session,
        } => {
            let store = CwStore::open(&db_path)?;
            commands::session::run_prune(&store, target, summary, session)?;
        }
        Commands::Dedupe { file, session } => {
            let store = CwStore::open(&db_path)?;
            commands::session::run_dedupe(&store, file, session)?;
        }
        Commands::Gate {
            disable_server,
            disable_tool,
            session,
        } => {
            let store = CwStore::open(&db_path)?;
            commands::session::run_gate(&store, disable_server, disable_tool, session)?;
        }
        Commands::Pin {
            label,
            content,
            session,
        } => {
            let store = CwStore::open(&db_path)?;
            commands::session::run_pin(&store, label, content, session)?;
        }
        Commands::Import {
            path,
            harness,
            workspace,
        } => {
            let store = CwStore::open(&db_path)?;
            commands::session::run_import(&store, path, harness, workspace)?;
        }
        Commands::SyncBack { session } => {
            let store = CwStore::open(&db_path)?;
            commands::session::run_sync_back(&store, session)?;
        }
        Commands::Local { session, model } => {
            let store = CwStore::open(&db_path)?;
            commands::session::run_local(&store, session, model)?;
        }
        Commands::Autopilot {
            enable,
            disable,
            min_health,
            strategy,
            sync_to_disk,
        } => {
            let store = CwStore::open(&db_path)?;
            commands::session::run_autopilot(
                &store,
                enable,
                disable,
                min_health,
                strategy,
                sync_to_disk,
            )?;
        }
        Commands::Gain { history } => {
            let store = CwStore::open(&db_path)?;
            commands::reports::run_gain_report(&store, history)?;
        }
        Commands::Pulse { session, limit } => {
            let store = CwStore::open(&db_path)?;
            commands::session::run_pulse(&store, session, limit)?;
        }
        Commands::Swarm {
            project,
            model,
            min_members,
            json,
        } => {
            let store = CwStore::open(&db_path)?;
            commands::reports::run_swarm_report(&store, project, model, min_members, json)?;
        }
        Commands::Slot {
            session,
            engine,
            slot_id,
            splice_range,
            json,
        } => {
            let store = CwStore::open(&db_path)?;
            commands::slot::run_slot_bridge_report(
                &store,
                session,
                engine,
                slot_id,
                splice_range,
                json,
            )
            .await?;
        }
        Commands::Hud {
            session,
            compact,
            ecg,
            json,
        } => {
            let store = CwStore::open(&db_path)?;
            commands::reports::run_hud_report(&store, session, compact, ecg, json)?;
        }
        Commands::Calibrate {
            dataset,
            horizon,
            synthetic,
            session,
            json,
        } => {
            commands::calibrate::handle_calibrate(
                dataset, horizon, synthetic, session, json, &db_path,
            )?;
        }
        Commands::Init { dry_run, no_start } => {
            commands::init::run_init_installer(dry_run, !no_start)?;
        }
        Commands::Hook { quiet, session } => {
            let store = CwStore::open(&db_path)?;
            commands::session::run_hook_check(&store, quiet, session)?;
        }
        Commands::Readiness { harness, json } => {
            commands::readiness::run_readiness(harness, json)?;
        }
        Commands::InstallIntegrations => {
            commands::init::run_init_installer(false, false)?;
        }
    }

    Ok(())
}
