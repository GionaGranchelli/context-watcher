mod forwarding;
use axum::{
    Json, Router,
    body::Body,
    extract::{
        Path, Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, HeaderValue, Request, StatusCode, Uri, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use bytes::Bytes;
use chrono::Utc;
use cw_core::{
    analyze_turn_from_payload,
    autopilot::{
        AmbientSavingsPulse, AutoPilotConfig, evaluate_autopilot_policies,
        generate_ambient_savings_pulse, should_emit_savings_pulse,
    },
    hud::{evaluate_attention_ecg, generate_context_hud_summary},
    ir::{
        ContextTurn, FileContextState, FirewallRule, HarnessKind, MutationOp, MutationRule,
        MutationStatus, NodeKind, ProviderKind, Session,
    },
    local_llm::{
        CostAndPerfReport, LocalModelAnalysis, analyze_local_model_kv, calculate_cost_and_perf,
        strip_internal_cw_fields,
    },
    mutation::{apply_mutations_to_payload, estimate_mutation_impact, evaluate_firewall},
    normalizer::detect_provider,
    optimizer::{OptimizationStrategyPlan, resolve_strategy_alias},
    radix::{RadixGraph, build_radix_graph},
    slot_bridge::{
        DEFAULT_PAGED_ATTENTION_BLOCK_SIZE, EngineSlotInfo, assign_deterministic_slot,
        plan_slot_splicing,
    },
    swarm::{analyze_swarm_prefix_sharing, build_swarm_radix_graph},
    validator::validate_protocol_invariants,
};
use cw_harnesses::sync_mutations_to_transcript_on_disk;
use cw_store::{CwStore, MutationEffectivenessRecord};
use forwarding::handle_proxy_request;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, broadcast, oneshot};
use tower_http::cors::{AllowOrigin, CorsLayer};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum WsEvent {
    SessionUpdated {
        session: Session,
    },
    TurnCompleted {
        session_id: String,
        turn: Box<ContextTurn>,
    },
    MutationsUpdated {
        session_id: String,
        mutations: Vec<MutationRule>,
    },
    FileContextStatusChanged {
        session_id: String,
        workspace_root: Option<String>,
        file_states: Vec<FileContextState>,
    },
    FirewallIntercepted {
        intercept: PendingInterceptInfo,
    },
    FirewallResolved {
        intercept_id: String,
    },
    AmbientSavingsPulse {
        session_id: String,
        pulse: Box<AmbientSavingsPulse>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingInterceptInfo {
    pub intercept_id: String,
    pub session_id: String,
    pub turn_seq: u32,
    pub rule_name: String,
    pub payload_preview: Value,
    pub created_at: chrono::DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterceptResolution {
    pub action: ApprovalAction,
    pub edited_payload: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalAction {
    Approve,
    Abort,
    Edit,
}

struct ActiveIntercept {
    info: PendingInterceptInfo,
    tx: oneshot::Sender<InterceptResolution>,
}

#[derive(Clone)]
pub struct DaemonState {
    pub store: CwStore,
    pub control_token: String,
    pub ws_tx: broadcast::Sender<WsEvent>,
    pub http_client: reqwest::Client,
    pub anthropic_upstream: String,
    pub openai_upstream: String,
    pub deepseek_upstream: String,
    pub gemini_upstream: String,
    pub ollama_upstream: String,
    pub llamacpp_upstream: String,
    pub llamacpp_api_key: Option<String>,
    pub vllm_upstream: String,
    intercepts: Arc<Mutex<HashMap<String, ActiveIntercept>>>,
}

impl DaemonState {
    pub fn new(store: CwStore) -> Self {
        let (ws_tx, _) = broadcast::channel(256);
        let http_client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(300))
            .build()
            .unwrap_or_default();

        Self {
            store,
            control_token: Uuid::new_v4().simple().to_string(),
            ws_tx,
            http_client,
            anthropic_upstream: std::env::var("CW_ANTHROPIC_UPSTREAM")
                .unwrap_or_else(|_| "https://api.anthropic.com".to_string()),
            openai_upstream: std::env::var("CW_OPENAI_UPSTREAM")
                .unwrap_or_else(|_| "https://api.openai.com".to_string()),
            deepseek_upstream: std::env::var("CW_DEEPSEEK_UPSTREAM")
                .unwrap_or_else(|_| "https://api.deepseek.com".to_string()),
            gemini_upstream: std::env::var("CW_GEMINI_UPSTREAM")
                .unwrap_or_else(|_| "https://generativelanguage.googleapis.com".to_string()),
            ollama_upstream: std::env::var("CW_OLLAMA_UPSTREAM")
                .unwrap_or_else(|_| "http://127.0.0.1:11434".to_string()),
            llamacpp_api_key: std::env::var("CW_LLAMACPP_API_KEY").ok(),
            llamacpp_upstream: std::env::var("CW_LLAMACPP_UPSTREAM")
                .unwrap_or_else(|_| "http://127.0.0.1:8080".to_string()),
            vllm_upstream: std::env::var("CW_VLLM_UPSTREAM")
                .unwrap_or_else(|_| "http://127.0.0.1:8000".to_string()),
            intercepts: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn broadcast(&self, ev: WsEvent) {
        let _ = self.ws_tx.send(ev);
    }
}

/// Returns true when `host` (a `Host` header value, optionally with a port) names the
/// loopback interface. Used to reject DNS-rebinding requests, where a hostile domain is
/// re-pointed at 127.0.0.1 and the browser sends `Host: evil.example`.
fn is_loopback_host(host: &str) -> bool {
    let host = host.trim();
    let name = if let Some(rest) = host.strip_prefix('[') {
        // IPv6 literal: "[::1]" or "[::1]:4400"
        match rest.split_once(']') {
            Some((addr, tail)) if tail.is_empty() || tail.starts_with(':') => addr,
            _ => return false,
        }
    } else {
        match host.rsplit_once(':') {
            Some((name, port)) if port.chars().all(|c| c.is_ascii_digit()) => name,
            Some(_) => return false,
            None => host,
        }
    };
    let name = name.to_ascii_lowercase();
    name == "localhost" || name == "127.0.0.1" || name == "::1"
}

/// Returns true when an `Origin` header value is a page served from the local machine
/// (the embedded web UI, or the Vite dev server). `null` and every remote origin are refused.
fn is_loopback_origin(origin: &str) -> bool {
    match origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
    {
        Some(rest) if !rest.contains('/') => is_loopback_host(rest),
        _ => false,
    }
}

/// CORS policy for the daemon: only pages served from loopback may read responses.
/// Replaces the previous `CorsLayer::permissive()`, which let any website read session
/// transcripts and drive the control API from the user's browser.
fn local_cors_layer() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(|origin: &HeaderValue, _| {
            origin.to_str().map(is_loopback_origin).unwrap_or(false)
        }))
        .allow_methods(tower_http::cors::Any)
        .allow_headers(tower_http::cors::Any)
}

/// Rejects requests that did not come from the local machine's own pages or tools:
/// * a `Host` header that is not loopback (blocks DNS rebinding), and
/// * an `Origin` header that is not loopback (blocks cross-site requests, including
///   "simple" form POSTs and cross-origin WebSocket upgrades, which CORS does not stop).
///
/// CLI, MCP and harness clients send no `Origin` header and a loopback `Host`, so they are
/// unaffected.
async fn local_only_guard(req: Request<Body>, next: Next) -> Response {
    let headers = req.headers();
    if headers
        .get(header::HOST)
        .is_some_and(|h| !h.to_str().map(is_loopback_host).unwrap_or(false))
    {
        return (
            StatusCode::FORBIDDEN,
            "contextWatcher: non-loopback Host refused",
        )
            .into_response();
    }
    if headers.get(header::ORIGIN).is_some_and(|o| {
        let origin = o.to_str().unwrap_or("");
        let host = headers
            .get(header::HOST)
            .and_then(|h| h.to_str().ok())
            .unwrap_or("");
        !is_loopback_origin(origin)
            || (origin != format!("http://{host}")
                && origin != format!("https://{host}")
                && std::env::var("CW_UI_ORIGIN").ok().as_deref() != Some(origin))
    }) {
        return (
            StatusCode::FORBIDDEN,
            "contextWatcher: cross-origin request refused",
        )
            .into_response();
    }
    next.run(req).await
}

async fn control_guard(
    State(state): State<DaemonState>,
    req: Request<Body>,
    next: Next,
) -> Response {
    if req.uri().path() == "/api/v1/health" {
        return next.run(req).await;
    }
    let bearer = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let control_header = req
        .headers()
        .get("x-cw-control-token")
        .and_then(|v| v.to_str().ok());
    let ws_token = req
        .headers()
        .get(header::SEC_WEBSOCKET_PROTOCOL)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            v.split(',')
                .map(str::trim)
                .find_map(|v| v.strip_prefix("cw-token."))
        });
    if bearer.or(control_header).or(ws_token) != Some(state.control_token.as_str()) {
        return (
            StatusCode::UNAUTHORIZED,
            "Use cw auth-token to connect a local client",
        )
            .into_response();
    }
    next.run(req).await
}

/// Builds the Intercepting Proxy Router (`127.0.0.1:4401`).
pub fn build_proxy_router(state: DaemonState) -> Router {
    Router::new()
        .route("/health", get(|| async { Json(json!({ "status": "ok" })) }))
        .route(
            "/api/v1/sessions/:id/transform",
            post(api_transform_context),
        )
        .fallback(handle_proxy_request)
        .layer(local_cors_layer())
        .layer(middleware::from_fn(local_only_guard))
        .with_state(state)
}

/// Builds the Control & REST/WS API Router (`/api/v1/*` on `127.0.0.1:4400`).
pub fn build_api_router(state: DaemonState) -> Router {
    Router::new()
        .route(
            "/api/v1/health",
            get(|| async { Json(json!({ "status": "ok" })) }),
        )
        .route("/api/v1/sessions", get(api_list_sessions))
        .route("/api/v1/sessions/:id", get(api_get_session))
        .route("/api/v1/sessions/:id/graph", get(api_get_session_graph))
        .route("/api/v1/sessions/:id/turns", get(api_list_turns))
        .route(
            "/api/v1/sessions/:id/turns/:turn_id/raw",
            get(api_get_turn_raw),
        )
        .route("/api/v1/sessions/:id/files", get(api_get_session_files))
        .route(
            "/api/v1/sessions/:id/mutations",
            get(api_list_mutations)
                .post(api_create_mutation)
                .delete(api_clear_mutations),
        )
        .route(
            "/api/v1/sessions/:id/mutations/commit-drafts",
            post(api_commit_drafts),
        )
        .route(
            "/api/v1/sessions/:id/mutations/:mutation_id",
            delete(api_delete_mutation),
        )
        .route(
            "/api/v1/sessions/:id/transform",
            post(api_transform_context),
        )
        .route("/api/v1/sessions/:id/optimize", post(api_auto_optimize))
        .route(
            "/api/v1/sessions/:id/effectiveness",
            get(api_get_session_effectiveness),
        )
        .route("/api/v1/effectiveness", get(api_get_all_effectiveness))
        .route("/api/v1/pulses", get(api_list_recent_pulses))
        .route("/api/v1/sessions/:id/pulses", get(api_get_session_pulse))
        .route("/api/v1/swarms", get(api_list_swarms))
        .route("/api/v1/swarms/:group_id/graph", get(api_get_swarm_graph))
        .route("/api/v1/engine/slots", get(api_get_engine_slots))
        .route(
            "/api/v1/engine/slots/:slot_id/splice",
            post(api_splice_engine_slot),
        )
        .route("/api/v1/hud", get(api_get_active_hud))
        .route("/api/v1/sessions/:id/hud", get(api_get_session_hud))
        .route("/api/v1/sessions/:id/ecg", get(api_get_session_ecg))
        .route(
            "/api/v1/sessions/:id/sync-back",
            post(api_sync_transcript_back),
        )
        .route("/api/v1/sessions/:id/replay", post(api_fork_replay))
        .route("/api/v1/ingest", post(api_ingest_payload))
        .route(
            "/api/v1/autopilot",
            get(api_get_autopilot).post(api_set_autopilot),
        )
        .route(
            "/api/v1/firewall/rules",
            get(api_list_firewall_rules).post(api_upsert_firewall_rule),
        )
        .route("/api/v1/firewall/intercepts", get(api_list_intercepts))
        .route(
            "/api/v1/firewall/intercepts/:intercept_id/resolve",
            post(api_resolve_intercept),
        )
        .route("/api/v1/ws", get(api_ws_handler))
        .layer(middleware::from_fn_with_state(state.clone(), control_guard))
        .layer(local_cors_layer())
        .layer(middleware::from_fn(local_only_guard))
        .with_state(state)
}

fn extract_cached_tokens_from_json(resp: &Value) -> Option<u32> {
    // Support llama.cpp `timings.cache_n`
    if let Some(cache_n) = resp
        .get("timings")
        .and_then(|t| t.get("cache_n"))
        .and_then(|v| v.as_u64())
    {
        return Some(cache_n as u32);
    }

    // Support Anthropic SSE `message_start.message.usage`
    let usage = resp
        .get("usage")
        .or_else(|| resp.get("usageMetadata"))
        .or_else(|| resp.get("message").and_then(|m| m.get("usage")))?;

    usage
        .get("cache_read_input_tokens")
        .or_else(|| {
            usage
                .get("prompt_tokens_details")
                .and_then(|d| d.get("cached_tokens"))
        })
        .or_else(|| {
            usage
                .get("input_tokens_details")
                .and_then(|d| d.get("cached_tokens"))
        })
        .or_else(|| usage.get("cachedContentTokenCount"))
        .and_then(|v| v.as_u64())
        .map(|n| n as u32)
}

pub fn extract_cached_tokens_from_sse(chunk_text: &str) -> Option<u32> {
    for line in chunk_text.lines() {
        if let Some(data) = line.strip_prefix("data: ")
            && let Ok(val) = serde_json::from_str::<Value>(data)
            && let Some(c) = extract_cached_tokens_from_json(&val)
        {
            return Some(c);
        }
    }
    None
}

#[derive(Default, Debug)]
pub struct SseChunkAccumulator {
    buffer: String,
}

impl SseChunkAccumulator {
    pub fn push_chunk(&mut self, bytes: &[u8]) -> Option<u32> {
        let text = String::from_utf8_lossy(bytes);
        self.buffer.push_str(&text);
        if let Some(idx) = self.buffer.rfind('\n') {
            let complete = &self.buffer[..=idx];
            let cached = extract_cached_tokens_from_sse(complete);
            self.buffer.drain(..=idx);
            return cached;
        }
        None
    }

    pub fn flush_cached(&mut self) -> Option<u32> {
        let res = extract_cached_tokens_from_sse(&self.buffer);
        self.buffer.clear();
        res
    }
}

// ============================================================================
// Daemon Control REST & WebSocket Handlers
// ============================================================================

#[derive(Deserialize)]
struct SessionPage {
    limit: Option<usize>,
    offset: Option<usize>,
}
async fn api_list_sessions(
    State(state): State<DaemonState>,
    Query(page): Query<SessionPage>,
) -> impl IntoResponse {
    match state
        .store
        .list_sessions_page(page.limit.unwrap_or(200), page.offset.unwrap_or(0))
    {
        Ok(sessions) => Json(sessions).into_response(),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response(),
    }
}

async fn api_get_session(
    State(state): State<DaemonState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.store.get_session(&id) {
        Ok(Some(s)) => Json(json!(s)).into_response(),
        _ => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "Session not found" })),
        )
            .into_response(),
    }
}

#[derive(Serialize)]
struct SessionGraphResponse {
    revision: String,
    mutation_preview: Option<ContextTurn>,
    session: Option<Session>,
    latest_turn: Option<ContextTurn>,
    turns: Vec<ContextTurn>,
    radix_graph: RadixGraph,
    mutations: Vec<MutationRule>,
    file_states: Vec<FileContextState>,
    optimization_plans: Vec<OptimizationStrategyPlan>,
    cost_and_perf: Option<CostAndPerfReport>,
    local_model_analysis: Option<LocalModelAnalysis>,
    autopilot_config: AutoPilotConfig,
    transcript_source_path: Option<String>,
}

async fn api_get_session_graph(
    State(state): State<DaemonState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let session = state.store.get_session(&id).ok().flatten();
    let (full_turns, mutations, revision) = match state.store.session_history_snapshot(&id) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response();
        }
    };
    let latest_turn = full_turns.last().cloned();
    let radix_graph = match &latest_turn {
        Some(lt) => build_radix_graph(lt, &full_turns),
        None => RadixGraph::default(),
    };
    let optimization_plans = match &latest_turn {
        Some(lt) => state.store.optimization_plans(lt).unwrap_or_default(),
        None => Vec::new(),
    };

    let mutation_preview = latest_turn.as_ref().and_then(|latest| {
        let raw = state
            .store
            .get_turn_raw_payload(&latest.id)
            .ok()
            .flatten()?;
        preview_turn(latest, &raw, &mutations, session.as_ref()?.provider)
    });

    let model_str = session
        .as_ref()
        .map(|s| s.model.as_str())
        .unwrap_or("claude-sonnet-4");
    let provider_kind = session
        .as_ref()
        .map(|s| s.provider)
        .unwrap_or(ProviderKind::Anthropic);

    let cost_and_perf = latest_turn
        .as_ref()
        .map(|lt| calculate_cost_and_perf(lt, model_str, provider_kind));
    let local_model_analysis = latest_turn
        .as_ref()
        .map(|lt| analyze_local_model_kv(lt, &id, model_str, provider_kind));

    // Pillar 6: Lightweight historical turns (strip heavy `nodes` array from older turns so 350k+ token sessions load in <5ms)
    let turns_lightweight: Vec<ContextTurn> = full_turns
        .into_iter()
        .map(|mut t| {
            t.nodes.clear();
            t
        })
        .collect();

    let file_states = state.store.get_file_states(&id).unwrap_or_default();
    let autopilot_config = state.store.get_autopilot_config().unwrap_or_default();
    let transcript_source_path = state
        .store
        .get_session_source_path(&id)
        .ok()
        .flatten()
        .map(|(p, _)| p);

    Json(SessionGraphResponse {
        revision,
        mutation_preview,
        session,
        latest_turn,
        turns: turns_lightweight,
        radix_graph,
        mutations,
        file_states,
        optimization_plans,
        cost_and_perf,
        local_model_analysis,
        autopilot_config,
        transcript_source_path,
    })
    .into_response()
}

async fn api_sync_transcript_back(
    State(state): State<DaemonState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match sync_mutations_to_transcript_on_disk(&state.store, &id) {
        Ok(report) => Json(json!(report)).into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn api_get_autopilot(State(state): State<DaemonState>) -> impl IntoResponse {
    let cfg = state.store.get_autopilot_config().unwrap_or_default();
    Json(cfg)
}

async fn api_set_autopilot(
    State(state): State<DaemonState>,
    Json(cfg): Json<AutoPilotConfig>,
) -> impl IntoResponse {
    match state.store.set_autopilot_config(&cfg) {
        Ok(()) => Json(json!({"autopilot_config":cfg})).into_response(),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response(),
    }
}

async fn api_list_turns(
    State(state): State<DaemonState>,
    Path(id): Path<String>,
    Query(page): Query<SessionPage>,
) -> impl IntoResponse {
    match state
        .store
        .list_turns_page(&id, page.limit.unwrap_or(100), page.offset.unwrap_or(0))
    {
        Ok(turns) => Json(turns).into_response(),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response(),
    }
}

async fn api_get_turn_raw(
    State(state): State<DaemonState>,
    Path((id, turn_id)): Path<(String, String)>,
) -> impl IntoResponse {
    match state.store.get_turn(&turn_id) {
        Ok(Some(turn)) if turn.session_id == id => {}
        Ok(_) => return (StatusCode::NOT_FOUND, "Turn does not belong to session").into_response(),
        Err(error) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response();
        }
    }
    match state.store.get_turn_raw_payload(&turn_id) {
        Ok(Some(v)) => Json(v).into_response(),
        _ => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "Payload not found" })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct FilesQuery {
    workspace_root: Option<String>,
}

async fn api_get_session_files(
    State(state): State<DaemonState>,
    Path(id): Path<String>,
    Query(q): Query<FilesQuery>,
) -> impl IntoResponse {
    let _ = q.workspace_root;
    let files = state.store.get_file_states(&id).unwrap_or_default();
    Json(files)
}

#[derive(Deserialize)]
struct CreateMutationRequest {
    op: MutationOp,
    status: Option<MutationStatus>,
}

async fn api_list_mutations(
    State(state): State<DaemonState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let mutations = state.store.list_mutations(&id).unwrap_or_default();
    Json(mutations)
}

async fn api_create_mutation(
    State(state): State<DaemonState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(req): Json<CreateMutationRequest>,
) -> impl IntoResponse {
    let latest_turn = state.store.get_latest_turn(&id).ok().flatten();
    let (saved, kv_break) = match &latest_turn {
        Some(t) => estimate_mutation_impact(&req.op, &t.nodes, Some(&t.kv_cache_report)),
        None => (0, 0),
    };

    let rule = MutationRule {
        id: format!("mut_{}", &Uuid::new_v4().to_string()[..8]),
        session_id: id.clone(),
        op: req.op,
        status: req.status.unwrap_or(MutationStatus::Committed),
        estimated_saved_tokens: saved,
        estimated_kv_break_tokens: kv_break,
        created_at: Utc::now(),
    };
    let Some(expected) = headers.get(header::IF_MATCH).and_then(|h| h.to_str().ok()) else {
        return (
            StatusCode::PRECONDITION_REQUIRED,
            "Refresh the session graph and send If-Match",
        )
            .into_response();
    };
    if let Err(error) =
        state
            .store
            .save_mutations_at_revision(&id, Some(expected), std::slice::from_ref(&rule))
    {
        return (StatusCode::PRECONDITION_FAILED, error.to_string()).into_response();
    }

    // If committed immediately and we have a latest turn with raw payload, re-evaluate latest turn in-place for instant UI feedback!
    recompute_latest_turn_with_mutations(&state, &id);

    let all_mutations = state.store.list_mutations(&id).unwrap_or_default();
    state.broadcast(WsEvent::MutationsUpdated {
        session_id: id.clone(),
        mutations: all_mutations,
    });
    (
        [(
            "x-cw-revision",
            state.store.session_revision(&id).unwrap_or_default(),
        )],
        Json(rule),
    )
        .into_response()
}

fn mutation_write_result(
    state: &DaemonState,
    id: &str,
    result: Result<usize, cw_store::StoreError>,
) -> axum::response::Response {
    match result {
        Ok(count) => {
            recompute_latest_turn_with_mutations(state, id);
            if let Ok(mutations) = state.store.list_mutations(id) {
                state.broadcast(WsEvent::MutationsUpdated {
                    session_id: id.to_owned(),
                    mutations,
                });
            }
            match state.store.session_revision(id) {
                Ok(revision) => (
                    [("x-cw-revision", revision)],
                    Json(json!({"changed":count,"committed":count,"deleted_count":count})),
                )
                    .into_response(),
                Err(error) => {
                    (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response()
                }
            }
        }
        Err(cw_store::StoreError::Conflict) => (
            StatusCode::PRECONDITION_FAILED,
            "Refresh the session graph before changing mutations",
        )
            .into_response(),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response(),
    }
}

async fn api_commit_drafts(
    State(state): State<DaemonState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(expected) = headers.get(header::IF_MATCH).and_then(|h| h.to_str().ok()) else {
        return (
            StatusCode::PRECONDITION_REQUIRED,
            "Send If-Match from the session graph",
        )
            .into_response();
    };
    mutation_write_result(
        &state,
        &id,
        state
            .store
            .change_mutations_at_revision(&id, expected, &[], true),
    )
}

async fn api_delete_mutation(
    State(state): State<DaemonState>,
    Path((id, mutation_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(expected) = headers.get(header::IF_MATCH).and_then(|h| h.to_str().ok()) else {
        return (
            StatusCode::PRECONDITION_REQUIRED,
            "Send If-Match from the session graph",
        )
            .into_response();
    };
    match state.store.list_mutations(&id) {
        Ok(rules) if !rules.iter().any(|r| r.id == mutation_id) => {
            return (StatusCode::NOT_FOUND, "Mutation does not belong to session").into_response();
        }
        Err(error) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response();
        }
        _ => {}
    }
    mutation_write_result(
        &state,
        &id,
        state
            .store
            .change_mutations_at_revision(&id, expected, &[mutation_id], false),
    )
}

#[derive(Deserialize, Default)]
struct ClearMutationsQuery {
    only_pins: Option<bool>,
    label: Option<String>,
}

async fn api_clear_mutations(
    State(state): State<DaemonState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    axum::extract::Query(q): axum::extract::Query<ClearMutationsQuery>,
) -> impl IntoResponse {
    let Some(expected) = headers.get(header::IF_MATCH).and_then(|h| h.to_str().ok()) else {
        return (
            StatusCode::PRECONDITION_REQUIRED,
            "Send If-Match from the session graph",
        )
            .into_response();
    };
    let rules = match state.store.list_mutations(&id) {
        Ok(rules) => rules,
        Err(error) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response();
        }
    };
    let ids: Vec<_> = rules.into_iter().filter(|r| !q.only_pins.unwrap_or(true) || matches!(&r.op, MutationOp::PinContext { label, .. } if q.label.as_ref().is_none_or(|f| label.to_lowercase().contains(&f.trim().to_lowercase())))).map(|r| r.id).collect();
    mutation_write_result(
        &state,
        &id,
        state
            .store
            .change_mutations_at_revision(&id, expected, &ids, false),
    )
}

#[derive(Deserialize)]
pub struct TransformContextRequest {
    pub messages: Value,
    pub provider: Option<String>,
}

async fn api_transform_context(
    State(state): State<DaemonState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(req): Json<TransformContextRequest>,
) -> impl IntoResponse {
    let authorized = headers
        .get("x-cw-control-token")
        .and_then(|v| v.to_str().ok())
        == Some(state.control_token.as_str());
    if !authorized {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "status": "forbidden",
                "messages": req.messages,
                "applied_rules": [],
                "saved_tokens": 0,
            })),
        )
            .into_response();
    }

    if !req.messages.is_array() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "status": "bad_request",
                "error": "messages must be a JSON array",
                "messages": req.messages,
                "applied_rules": [],
                "saved_tokens": 0,
            })),
        )
            .into_response();
    }

    let provider = req
        .provider
        .as_deref()
        .map(ProviderKind::from_str_lossy)
        .unwrap_or(ProviderKind::OpenAI);

    let rules = state.store.list_mutations(&id).unwrap_or_default();
    if rules.is_empty() {
        return (
            StatusCode::OK,
            Json(json!({
                "status": "ok",
                "messages": req.messages,
                "applied_rules": [],
                "saved_tokens": 0,
            })),
        )
            .into_response();
    }

    let mut payload = json!({
        "messages": req.messages,
    });

    let seq = state
        .store
        .get_latest_turn(&id)
        .ok()
        .flatten()
        .map(|t| t.sequence_number + 1)
        .unwrap_or(1);

    let report = cw_core::mutation::apply_mutations_to_payload(&mut payload, provider, &rules, seq);

    // Run Pre-Forward Validation Gate
    if let Err(gate_err) = cw_core::validator::validate_protocol_invariants(&payload, provider) {
        tracing::warn!(%gate_err, "Pre-forward validation failed during context transformation; failing open");
        return (
            StatusCode::OK,
            Json(json!({
                "status": "fallback_raw",
                "messages": req.messages,
                "applied_rules": [],
                "saved_tokens": 0,
            })),
        )
            .into_response();
    }

    let final_messages = payload.get("messages").cloned().unwrap_or(req.messages);

    (
        StatusCode::OK,
        Json(json!({
            "status": "ok",
            "messages": final_messages,
            "applied_rules": report.applied_rule_ids,
            "saved_tokens": report.saved_tokens,
        })),
    )
        .into_response()
}

#[derive(Deserialize, Default)]
struct OptimizeRequest {
    strategy_id: Option<String>,
    selected_item_ids: Option<Vec<String>>,
    status: Option<MutationStatus>,
}

/// Applies either a specific `OptimizationStrategyPlan` (filtered by `selected_item_ids` checkboxes)
/// or all active lint suggestions if called without a body.
async fn api_auto_optimize(
    State(state): State<DaemonState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let req: OptimizeRequest = if body.is_empty() {
        OptimizeRequest::default()
    } else {
        match serde_json::from_slice(&body) {
            Ok(req) => req,
            Err(_) => {
                return (StatusCode::BAD_REQUEST, "Invalid optimization request").into_response();
            }
        }
    };
    let Some(expected) = headers.get(header::IF_MATCH).and_then(|h| h.to_str().ok()) else {
        return (
            StatusCode::PRECONDITION_REQUIRED,
            "Refresh the graph and send If-Match",
        )
            .into_response();
    };
    let target_status = req.status.unwrap_or(MutationStatus::Committed);
    let mut created = Vec::new();

    if let Ok(Some(latest)) = state.store.get_latest_turn(&id) {
        if let Some(raw_strat_id) = req.strategy_id {
            let strat_id = resolve_strategy_alias(&raw_strat_id);
            if strat_id == "deep_trajectory_fold" {
                return (
                    StatusCode::BAD_REQUEST,
                    "Automatic trajectory folding is unavailable without a verified outcome summary; use cw_fold_turns or `cw fold <start_turn> <end_turn> --summary <TEXT>`",
                )
                    .into_response();
            }
            let plans = match state.store.optimization_plans(&latest) {
                Ok(plans) => plans,
                Err(error) => {
                    return (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response();
                }
            };
            if let Some(plan) = plans.into_iter().find(|p| p.id == strat_id) {
                for item in plan.items {
                    if let Some(ref allowed) = req.selected_item_ids
                        && !allowed.contains(&item.item_id)
                    {
                        continue;
                    }
                    let rule = MutationRule {
                        id: format!("mut_{}", &Uuid::new_v4().to_string()[..8]),
                        session_id: id.clone(),
                        op: item.mutation_op,
                        status: target_status,
                        estimated_saved_tokens: item.saved_tokens,
                        estimated_kv_break_tokens: item.kv_break_tokens,
                        created_at: Utc::now(),
                    };
                    created.push(rule);
                }
            }
        } else {
            for finding in &latest.lint_findings {
                if let Some(op) = &finding.suggested_mutation {
                    let (saved, kv_break) =
                        estimate_mutation_impact(op, &latest.nodes, Some(&latest.kv_cache_report));
                    let rule = MutationRule {
                        id: format!("mut_{}", &Uuid::new_v4().to_string()[..8]),
                        session_id: id.clone(),
                        op: op.clone(),
                        status: target_status,
                        estimated_saved_tokens: saved,
                        estimated_kv_break_tokens: kv_break,
                        created_at: Utc::now(),
                    };
                    created.push(rule);
                }
            }
        }
    }
    if let Err(error) = state
        .store
        .save_mutations_at_revision(&id, Some(expected), &created)
    {
        return (StatusCode::PRECONDITION_FAILED, error.to_string()).into_response();
    }
    recompute_latest_turn_with_mutations(&state, &id);
    ([ ("x-cw-revision",state.store.session_revision(&id).unwrap_or_default()) ],Json(json!({ "applied_count":created.len(), "estimated_saved_tokens":created.iter().map(|r|r.estimated_saved_tokens.max(0) as u64).sum::<u64>(), "applied_mutations": created }))).into_response()
}

async fn api_get_session_effectiveness(
    State(state): State<DaemonState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.store.list_mutation_effectiveness(Some(&id)) {
        Ok(records) => Json(json!({ "effectiveness": records })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn api_get_all_effectiveness(State(state): State<DaemonState>) -> impl IntoResponse {
    match state.store.list_mutation_effectiveness(None) {
        Ok(records) => Json(json!({ "effectiveness": records })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn api_list_recent_pulses(State(state): State<DaemonState>) -> impl IntoResponse {
    let pulses = state
        .store
        .list_recent_ambient_pulses(50)
        .unwrap_or_default();
    Json(json!({ "pulses": pulses }))
}

async fn api_get_session_pulse(
    State(state): State<DaemonState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let pulse = state.store.get_latest_pulse_for_session(&id).ok().flatten();
    Json(json!({ "session_id": id, "pulse": pulse }))
}

#[derive(Debug, Deserialize)]
pub struct SwarmQueryParams {
    pub project: Option<String>,
    pub model: Option<String>,
    pub min_members: Option<usize>,
}

async fn api_list_swarms(
    State(state): State<DaemonState>,
    Query(params): Query<SwarmQueryParams>,
) -> impl IntoResponse {
    let sessions = state.store.list_sessions().unwrap_or_default();
    let mut sessions_with_turns = Vec::new();

    for sess in &sessions {
        if let Some(ref proj) = params.project {
            let ws = sess
                .workspace_root
                .as_deref()
                .map(|w| w.trim_end_matches('/').rsplit('/').next().unwrap_or(w))
                .unwrap_or("default_workspace");
            if !ws.to_lowercase().contains(&proj.to_lowercase()) {
                continue;
            }
        }
        if let Some(ref mod_filter) = params.model
            && !sess
                .model
                .to_lowercase()
                .contains(&mod_filter.to_lowercase())
        {
            continue;
        }

        if let Ok(Some(latest_turn)) = state.store.get_latest_turn(&sess.id) {
            sessions_with_turns.push((sess, latest_turn));
        }
    }

    let items: Vec<(&Session, &ContextTurn)> =
        sessions_with_turns.iter().map(|(s, t)| (*s, t)).collect();

    let mut swarms = analyze_swarm_prefix_sharing(&items);
    if let Some(min_m) = params.min_members {
        swarms.retain(|s| s.member_count >= min_m);
    }
    Json(swarms)
}

async fn api_get_swarm_graph(
    State(state): State<DaemonState>,
    Path(group_id): Path<String>,
) -> impl IntoResponse {
    let sessions = state.store.list_sessions().unwrap_or_default();
    let mut sessions_with_turns = Vec::new();

    for sess in &sessions {
        if let Ok(Some(latest_turn)) = state.store.get_latest_turn(&sess.id) {
            sessions_with_turns.push((sess, latest_turn));
        }
    }

    let items: Vec<(&Session, &ContextTurn)> =
        sessions_with_turns.iter().map(|(s, t)| (*s, t)).collect();

    let swarms = analyze_swarm_prefix_sharing(&items);
    if let Some(swarm) = swarms.into_iter().find(|s| s.group_id == group_id) {
        let graph = build_swarm_radix_graph(&swarm);
        Json(json!({ "swarm": swarm, "radix_graph": graph })).into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": format!("Swarm group '{group_id}' not found") })),
        )
            .into_response()
    }
}

#[derive(Debug, Deserialize)]
pub struct EngineSlotsQuery {
    pub engine_url: Option<String>,
}

async fn api_get_engine_slots(
    State(state): State<DaemonState>,
    Query(params): Query<EngineSlotsQuery>,
) -> impl IntoResponse {
    let upstream_url = params
        .engine_url
        .unwrap_or_else(|| state.llamacpp_upstream.clone());
    let slots_url = format!("{}/slots", upstream_url.trim_end_matches('/'));

    let mut req_builder = state
        .http_client
        .get(&slots_url)
        .timeout(Duration::from_millis(2500));
    if upstream_url == state.llamacpp_upstream
        && let Some(key) = &state.llamacpp_api_key
    {
        req_builder = req_builder.header("Authorization", format!("Bearer {key}"));
    }

    let mut raw_slots = Vec::new();
    let upstream_reachable = match req_builder.send().await {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(slots_json) = resp.json::<Vec<serde_json::Value>>().await {
                raw_slots = slots_json;
                true
            } else {
                false
            }
        }
        _ => false,
    };

    let sessions = state.store.list_sessions().unwrap_or_default();
    let mut slots_info = Vec::new();

    if upstream_reachable && !raw_slots.is_empty() {
        let total_slots = raw_slots.len() as u32;
        for slot in raw_slots {
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
    }

    Json(json!({
        "upstream_url": upstream_url,
        "upstream_reachable": upstream_reachable,
        "memory_basis": "model-independent estimate; not physical residency",
        "block_size": DEFAULT_PAGED_ATTENTION_BLOCK_SIZE,
        "total_slots": slots_info.len(),
        "slots": slots_info,
    }))
}

#[derive(Debug, Deserialize)]
pub struct SpliceRequest {
    pub session_id: Option<String>,
    pub prune_range: (u32, u32),
    pub block_size: Option<u32>,
    pub execute: Option<bool>,
}

async fn api_splice_engine_slot(
    State(state): State<DaemonState>,
    Path(slot_id): Path<u32>,
    Json(req): Json<SpliceRequest>,
) -> impl IntoResponse {
    let bs = req.block_size.unwrap_or(DEFAULT_PAGED_ATTENTION_BLOCK_SIZE);
    let mut total_tokens = 0u32;
    let mut prefix_locked = 256u32;

    if let Some(ref sid) = req.session_id
        && let Ok(Some(turn)) = state.store.get_latest_turn(sid)
    {
        total_tokens = turn.token_usage.input_tokens;
        for n in &turn.nodes {
            match n.kind {
                NodeKind::SystemPrompt { .. }
                | NodeKind::InjectedRule { .. }
                | NodeKind::SkillCatalog { .. }
                | NodeKind::McpToolSchema { .. } => {
                    prefix_locked += n.token_span.count;
                }
                _ => {}
            }
        }
    }

    if total_tokens == 0 {
        total_tokens = req.prune_range.1.max(1024);
    }

    let plan = plan_slot_splicing(slot_id, total_tokens, prefix_locked, req.prune_range, bs);

    if req.execute.unwrap_or(false) {
        return (
            StatusCode::NOT_IMPLEMENTED,
            "Engine mutation is not qualified; this endpoint only previews a plan",
        )
            .into_response();
    }
    Json(json!({
        "slot_id": slot_id,
        "plan": plan,
        "upstream_executed": false,
        "execution_supported": false,
        "estimated": true,
    }))
    .into_response()
}

async fn api_get_active_hud(State(state): State<DaemonState>) -> impl IntoResponse {
    let sessions = state.store.list_sessions().unwrap_or_default();
    let latest_session = sessions.last();

    if let Some(s) = latest_session {
        let turn = state.store.get_latest_turn(&s.id).ok().flatten();
        let plans = turn
            .as_ref()
            .and_then(|turn| state.store.optimization_plans(turn).ok())
            .unwrap_or_default();
        let cuttable = plans
            .iter()
            .find(|p| p.id == "max_token_diet")
            .map(|p| p.saved_tokens)
            .unwrap_or(0);
        let hud = generate_context_hud_summary(
            &s.id,
            s.harness.as_str(),
            &s.model,
            turn.as_ref(),
            cuttable,
        );
        Json(hud).into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "No active sessions found" })),
        )
            .into_response()
    }
}

async fn api_get_session_hud(
    State(state): State<DaemonState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let Some(s) = state.store.get_session(&id).ok().flatten() else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": format!("Session '{id}' not found") })),
        )
            .into_response();
    };

    let turn = state.store.get_latest_turn(&s.id).ok().flatten();
    let plans = turn
        .as_ref()
        .and_then(|turn| state.store.optimization_plans(turn).ok())
        .unwrap_or_default();
    let cuttable = plans
        .iter()
        .find(|p| p.id == "max_token_diet")
        .map(|p| p.saved_tokens)
        .unwrap_or(0);
    let hud =
        generate_context_hud_summary(&s.id, s.harness.as_str(), &s.model, turn.as_ref(), cuttable);
    Json(hud).into_response()
}

async fn api_get_session_ecg(
    State(state): State<DaemonState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let Some(turn) = state.store.get_latest_turn(&id).ok().flatten() else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": format!("No turns found for session '{id}'") })),
        )
            .into_response();
    };

    let ecg = evaluate_attention_ecg(&turn);
    Json(ecg).into_response()
}

#[derive(Deserialize)]
struct ReplayRequest {
    from_turn_id: Option<String>,
    additional_mutations: Option<Vec<MutationOp>>,
}

async fn api_fork_replay(
    State(state): State<DaemonState>,
    Path(id): Path<String>,
    Json(req): Json<ReplayRequest>,
) -> impl IntoResponse {
    let source = match &req.from_turn_id {
        Some(tid) => state.store.get_turn(tid),
        None => state.store.get_latest_turn(&id),
    };
    let Some(source_turn) = (match source {
        Ok(turn) => turn.filter(|turn| turn.session_id == id),
        Err(error) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response();
        }
    }) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "Source turn not found" })),
        )
            .into_response();
    };

    let Some(mut payload) = state
        .store
        .get_turn_raw_payload(&source_turn.id)
        .ok()
        .flatten()
    else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Raw payload not available for this turn" })),
        )
            .into_response();
    };

    let session = state.store.get_session(&id).ok().flatten();
    let provider = session
        .as_ref()
        .map(|s| s.provider)
        .unwrap_or(ProviderKind::Anthropic);
    let provider = source_turn.normalization_provider.unwrap_or(provider);
    let mut rules = match state.store.list_mutations(&id) {
        Ok(rules) => rules,
        Err(error) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response();
        }
    };
    if let Some(extra) = req.additional_mutations {
        for op in extra {
            rules.push(MutationRule {
                id: format!("fork_mut_{}", &Uuid::new_v4().to_string()[..6]),
                session_id: id.clone(),
                op,
                status: MutationStatus::Committed,
                estimated_saved_tokens: 0,
                estimated_kv_break_tokens: 0,
                created_at: Utc::now(),
            });
        }
    }

    let mut_report =
        apply_mutations_to_payload(&mut payload, provider, &rules, source_turn.sequence_number);
    let fork_turn_id = format!(
        "fork_{}_{}",
        source_turn.sequence_number,
        &Uuid::new_v4().to_string()[..6]
    );

    match analyze_turn_from_payload(
        fork_turn_id,
        id.clone(),
        Some(source_turn.id.clone()),
        source_turn.sequence_number,
        &payload,
        provider,
        Some(&source_turn.nodes),
        None,
        None,
    ) {
        Ok((mut fork_turn, _)) => {
            fork_turn.outcome = Some("preview".into());
            fork_turn.token_usage.saved_by_mutations = mut_report.saved_tokens;
            Json(json!({
                "preview":true,
                "upstream_executed":false,
                "original_turn": source_turn,
                "forked_turn": fork_turn,
                "saved_tokens": mut_report.saved_tokens
            }))
            .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct IngestRequest {
    session_id: String,
    harness: Option<String>,
    provider: Option<String>,
    workspace_root: Option<String>,
    model: Option<String>,
    payload: Value,
}

async fn api_ingest_payload(
    State(state): State<DaemonState>,
    Json(req): Json<IngestRequest>,
) -> impl IntoResponse {
    let provider = req
        .provider
        .as_deref()
        .map(ProviderKind::from_str_lossy)
        .unwrap_or_else(|| detect_provider("", &req.payload));
    let harness = req
        .harness
        .as_deref()
        .map(HarnessKind::from_str_lossy)
        .unwrap_or(HarnessKind::GenericProxy);

    let session = Session {
        id: req.session_id.clone(),
        parent_session_id: None,
        harness,
        workspace_root: req.workspace_root.clone(),
        model: req.model.unwrap_or_else(|| "default-model".to_string()),
        provider,
        title: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };
    if let Err(error) = state.store.upsert_session(&session) {
        return (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response();
    }

    let prev_turn = state.store.get_latest_turn(&req.session_id).ok().flatten();
    let seq_num = match state.store.reserve_turn_sequence(&req.session_id) {
        Ok(seq) => seq,
        Err(error) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response();
        }
    };

    let mut mutated_payload = req.payload.clone();
    let active_mutations = state
        .store
        .list_mutations(&req.session_id)
        .unwrap_or_default();
    let mut_report =
        apply_mutations_to_payload(&mut mutated_payload, provider, &active_mutations, seq_num);

    let turn_id = format!("turn_{}_{}", req.session_id, seq_num);
    match analyze_turn_from_payload(
        turn_id,
        req.session_id.clone(),
        prev_turn.as_ref().map(|t| t.id.clone()),
        seq_num,
        &mutated_payload,
        provider,
        prev_turn.as_ref().map(|t| t.nodes.as_slice()),
        None,
        req.workspace_root.as_deref(),
    ) {
        Ok((mut turn, file_states)) => {
            turn.token_usage.saved_by_mutations = mut_report.saved_tokens;
            turn.outcome = Some("imported".into());
            if let Err(error) = state
                .store
                .save_turn(&turn, Some(&req.payload), Some(&mutated_payload), None)
                .and_then(|()| state.store.save_file_states(&req.session_id, &file_states))
            {
                return (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response();
            }
            state.broadcast(WsEvent::TurnCompleted {
                session_id: req.session_id.clone(),
                turn: Box::new(turn.clone()),
            });
            state.broadcast(WsEvent::FileContextStatusChanged {
                session_id: req.session_id,
                workspace_root: req.workspace_root,
                file_states,
            });
            Json(json!(turn)).into_response()
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn api_list_firewall_rules(State(state): State<DaemonState>) -> impl IntoResponse {
    let rules = state.store.list_firewall_rules(None).unwrap_or_default();
    Json(rules)
}

async fn api_upsert_firewall_rule(
    State(state): State<DaemonState>,
    Json(rule): Json<FirewallRule>,
) -> impl IntoResponse {
    match state.store.upsert_firewall_rule(&rule) {
        Ok(()) => Json(rule).into_response(),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response(),
    }
}

async fn api_list_intercepts(State(state): State<DaemonState>) -> impl IntoResponse {
    let map = state.intercepts.lock().await;
    let list: Vec<PendingInterceptInfo> = map.values().map(|i| i.info.clone()).collect();
    Json(list)
}

async fn api_resolve_intercept(
    State(state): State<DaemonState>,
    Path(intercept_id): Path<String>,
    Json(res): Json<InterceptResolution>,
) -> impl IntoResponse {
    let removed = state.intercepts.lock().await.remove(&intercept_id);
    if let Some(active) = removed {
        let _ = active.tx.send(res);
        state.broadcast(WsEvent::FirewallResolved {
            intercept_id: intercept_id.clone(),
        });
        Json(json!({ "resolved": true, "intercept_id": intercept_id })).into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "Intercept not found" })),
        )
            .into_response()
    }
}

async fn api_ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<DaemonState>,
) -> impl IntoResponse {
    ws.protocols(["cw"])
        .on_upgrade(move |socket| handle_ws_client(socket, state))
}

async fn handle_ws_client(mut socket: WebSocket, state: DaemonState) {
    let mut rx = state.ws_tx.subscribe();
    while let Ok(ev) = rx.recv().await {
        if let Ok(txt) = serde_json::to_string(&ev)
            && socket.send(Message::Text(txt)).await.is_err()
        {
            break;
        }
    }
}

pub fn recompute_latest_turn_in_store(store: &CwStore, session_id: &str) -> Option<ContextTurn> {
    let (turns, rules, _) = store.session_history_snapshot(session_id).ok()?;
    let latest = turns.last()?;
    let raw = store.get_turn_raw_payload(&latest.id).ok().flatten()?;
    let provider = store.get_session(session_id).ok().flatten()?.provider;
    preview_turn(latest, &raw, &rules, provider)
}

fn preview_turn(
    latest: &ContextTurn,
    raw: &Value,
    rules: &[MutationRule],
    provider: ProviderKind,
) -> Option<ContextTurn> {
    let mut payload = raw.clone();
    let provider = latest.normalization_provider.unwrap_or(provider);
    let report = apply_mutations_to_payload(&mut payload, provider, rules, latest.sequence_number);
    let (mut preview, _) = analyze_turn_from_payload(
        latest.id.clone(),
        latest.session_id.clone(),
        latest.parent_turn_id.clone(),
        latest.sequence_number,
        &payload,
        provider,
        None,
        None,
        None,
    )
    .ok()?;
    preview.outcome = Some("preview".into());
    preview.token_usage.saved_by_mutations = report.saved_tokens;
    preview.applied_mutation_ids = report.applied_rule_ids;
    Some(preview)
}

fn recompute_latest_turn_with_mutations(state: &DaemonState, session_id: &str) {
    if let Some(updated_turn) = recompute_latest_turn_in_store(&state.store, session_id) {
        let session = state.store.get_session(session_id).ok().flatten();
        let workspace_root = session.as_ref().and_then(|s| s.workspace_root.clone());
        let file_states = state.store.get_file_states(session_id).unwrap_or_default();
        let model = session
            .as_ref()
            .map(|s| s.model.as_str())
            .unwrap_or("claude-sonnet-4");
        let provider = session
            .as_ref()
            .map(|s| s.provider)
            .unwrap_or(ProviderKind::Anthropic);

        let cur_saved = updated_turn.token_usage.saved_by_mutations as u64;
        if cur_saved > 0 {
            let last_pulse = state
                .store
                .get_latest_pulse_for_session(session_id)
                .ok()
                .flatten();
            let last_saved = last_pulse.map(|p| p.cumulative_tokens_saved).unwrap_or(0);
            if should_emit_savings_pulse(cur_saved, last_saved, 100) {
                let pulse = generate_ambient_savings_pulse(
                    session_id,
                    &updated_turn,
                    model,
                    provider,
                    cur_saved,
                );
                let _ = state.store.record_ambient_pulse(&pulse);
                eprintln!("{}", pulse.formatted_message);
                state.broadcast(WsEvent::AmbientSavingsPulse {
                    session_id: session_id.to_string(),
                    pulse: Box::new(pulse),
                });
            }
        }

        state.broadcast(WsEvent::TurnCompleted {
            session_id: session_id.to_string(),
            turn: Box::new(updated_turn),
        });
        state.broadcast(WsEvent::FileContextStatusChanged {
            session_id: session_id.to_string(),
            workspace_root,
            file_states,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::Request;
    use tower::ServiceExt;

    #[test]
    fn test_loopback_host_and_origin_checks() {
        for ok in [
            "127.0.0.1",
            "127.0.0.1:4400",
            "localhost:4401",
            "LOCALHOST",
            "[::1]:4400",
            "[::1]",
        ] {
            assert!(is_loopback_host(ok), "{ok} should be accepted");
        }
        for bad in [
            "evil.example",
            "evil.example:4400",
            "127.0.0.1.evil.example",
            "localhost.evil:80",
            "[::1]x",
            "127.0.0.1:44a0",
            "",
        ] {
            assert!(!is_loopback_host(bad), "{bad} should be refused");
        }
        assert!(is_loopback_origin("http://127.0.0.1:4400"));
        assert!(is_loopback_origin("http://localhost:5173"));
        assert!(!is_loopback_origin("null"));
        assert!(!is_loopback_origin("https://evil.example"));
        assert!(!is_loopback_origin("http://localhost.evil.example"));
    }

    #[tokio::test]
    async fn test_api_refuses_cross_origin_and_rebinding() {
        let state = DaemonState::new(CwStore::open_in_memory().unwrap());
        let token = state.control_token.clone();
        let api_app = build_api_router(state);

        let cross_origin = Request::builder()
            .uri("/api/v1/sessions")
            .header("host", "127.0.0.1:4400")
            .header("origin", "https://evil.example")
            .body(Body::empty())
            .unwrap();
        let res = api_app.clone().oneshot(cross_origin).await.unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);

        let rebinding = Request::builder()
            .uri("/api/v1/sessions")
            .header("host", "evil.example:4400")
            .body(Body::empty())
            .unwrap();
        let res = api_app.clone().oneshot(rebinding).await.unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);

        let local_ui = Request::builder()
            .uri("/api/v1/sessions")
            .header("host", "127.0.0.1:4400")
            .header("origin", "http://127.0.0.1:4400")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let res = api_app.clone().oneshot(local_ui).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers().get("access-control-allow-origin").unwrap(),
            "http://127.0.0.1:4400"
        );

        let cli = Request::builder()
            .uri("/api/v1/health")
            .header("host", "127.0.0.1:4400")
            .body(Body::empty())
            .unwrap();
        let res = api_app.oneshot(cli).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_proxy_dry_run_and_api_mutations_and_replay() {
        let store = CwStore::open_in_memory().unwrap();
        let state = DaemonState::new(store);
        let store = state.store.clone();
        let token = state.control_token.clone();
        let proxy_app = build_proxy_router(state.clone());
        let api_app = build_api_router(state);

        // 1. Send dry-run Anthropic request through proxy
        let req_payload = json!({
            "model": "claude-sonnet-4",
            "max_tokens": 1024,
            "messages": [
                {
                    "role": "assistant",
                    "content": [
                        { "type": "tool_use", "id": "c1", "name": "read_file", "input": { "Path": "src/main.rs" } }
                    ]
                },
                {
                    "role": "user",
                    "content": [
                        { "type": "tool_result", "tool_use_id": "c1", "content": "fn main() { println!(\"v1\"); }" }
                    ]
                },
                {
                    "role": "assistant",
                    "content": [
                        { "type": "tool_use", "id": "c2", "name": "read_file", "input": { "Path": "src/main.rs" } }
                    ]
                },
                {
                    "role": "user",
                    "content": [
                        { "type": "tool_result", "tool_use_id": "c2", "content": "fn main() { println!(\"v1\"); }" }
                    ]
                }
            ]
        });

        let req = Request::builder()
            .method("POST")
            .uri("/anthropic/v1/messages")
            .header("content-type", "application/json")
            .header("x-cw-session-id", "test_sess")
            .header("x-cw-control-token", &token)
            .header("x-cw-dry-run", "1")
            .body(Body::from(serde_json::to_vec(&req_payload).unwrap()))
            .unwrap();

        let resp = proxy_app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // 2. Call 1-Click Optimize on the control API
        let opt_req = Request::builder()
            .method("POST")
            .uri("/api/v1/sessions/test_sess/optimize")
            .header("if-match", store.session_revision("test_sess").unwrap())
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let opt_resp = api_app.clone().oneshot(opt_req).await.unwrap();
        assert_eq!(opt_resp.status(), StatusCode::OK);

        // 3. Call Time-Travel Fork & Replay on the control API
        let replay_req = Request::builder()
            .method("POST")
            .uri("/api/v1/sessions/test_sess/replay")
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({ "from_turn_id": null })).unwrap(),
            ))
            .unwrap();
        let replay_resp = api_app.oneshot(replay_req).await.unwrap();
        assert_eq!(replay_resp.status(), StatusCode::OK);
        let bytes = to_bytes(replay_resp.into_body(), 1_000_000).await.unwrap();
        let val: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(val.get("forked_turn").is_some());
    }

    #[tokio::test]
    async fn test_pre_forward_validation_gate_fail_open_fallback() {
        use cw_core::validator::validate_protocol_invariants;

        let valid_raw = json!({
            "model": "claude-3-7-sonnet",
            "messages": [
                { "role": "user", "content": "Hello" },
                { "role": "assistant", "content": [{ "type": "text", "text": "Hi" }] }
            ]
        });

        // Corrupted mutation payload with orphan tool result
        let invalid_mutated = json!({
            "model": "claude-3-7-sonnet",
            "messages": [
                { "role": "user", "content": "Hello" },
                { "role": "assistant", "content": [{ "type": "text", "text": "Hi" }] },
                {
                    "role": "user",
                    "content": [
                        { "type": "tool_result", "tool_use_id": "orphan_c99", "content": "bad" }
                    ]
                }
            ]
        });

        assert!(validate_protocol_invariants(&valid_raw, ProviderKind::Anthropic).is_ok());
        assert!(validate_protocol_invariants(&invalid_mutated, ProviderKind::Anthropic).is_err());

        // In cw-proxy, the fail-open fallback replaces invalid_mutated with valid_raw
        let mut outbound = invalid_mutated.clone();
        if validate_protocol_invariants(&outbound, ProviderKind::Anthropic).is_err()
            && validate_protocol_invariants(&valid_raw, ProviderKind::Anthropic).is_ok()
        {
            outbound = valid_raw.clone();
        }

        assert_eq!(outbound, valid_raw);
        assert!(validate_protocol_invariants(&outbound, ProviderKind::Anthropic).is_ok());
    }

    #[test]
    fn test_split_chunk_sse_accumulator() {
        let mut acc = SseChunkAccumulator::default();

        // Chunk 1: Partial SSE event split right in the middle of json key
        let chunk1 =
            b"event: message_delta\ndata: {\"type\": \"message_delta\", \"usage\": {\"cache_";
        let res1 = acc.push_chunk(chunk1);
        assert_eq!(res1, None); // Not ready yet

        // Chunk 2: The rest of the key and values, followed by newline
        let chunk2 = b"read_input_tokens\": 2048}}\n\n";
        let res2 = acc.push_chunk(chunk2);
        assert_eq!(res2, Some(2048)); // Successfully reassembled and parsed!

        // Chunk 3: Normal complete chunk
        let chunk3 = b"data: {\"usage\": {\"input_tokens_details\": {\"cached_tokens\": 512}}}\n";
        let res3 = acc.push_chunk(chunk3);
        assert_eq!(res3, Some(512));
    }
}
