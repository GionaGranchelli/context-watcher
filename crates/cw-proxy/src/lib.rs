use axum::{
    Json, Router,
    body::Body,
    extract::{
        Path, Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use bytes::Bytes;
use chrono::Utc;
use cw_core::{
    analyze_turn_from_payload,
    ir::{
        ContextTurn, FileContextState, FirewallRule, HarnessKind, MutationOp, MutationRule,
        MutationStatus, ProviderKind, Session,
    },
    mutation::{apply_mutations_to_payload, estimate_mutation_impact, evaluate_firewall},
    normalizer::detect_provider,
    radix::{RadixGraph, build_radix_graph},
};
use cw_store::CwStore;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, broadcast, oneshot};
use tower_http::cors::CorsLayer;
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
    pub action: String, // "approve" | "abort" | "edit"
    pub edited_payload: Option<Value>,
}

struct ActiveIntercept {
    info: PendingInterceptInfo,
    tx: oneshot::Sender<InterceptResolution>,
}

#[derive(Clone)]
pub struct DaemonState {
    pub store: CwStore,
    pub ws_tx: broadcast::Sender<WsEvent>,
    pub http_client: reqwest::Client,
    pub anthropic_upstream: String,
    pub openai_upstream: String,
    pub gemini_upstream: String,
    intercepts: Arc<Mutex<HashMap<String, ActiveIntercept>>>,
}

impl DaemonState {
    pub fn new(store: CwStore) -> Self {
        let (ws_tx, _) = broadcast::channel(256);
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(300))
            .build()
            .unwrap_or_default();

        Self {
            store,
            ws_tx,
            http_client,
            anthropic_upstream: std::env::var("CW_ANTHROPIC_UPSTREAM")
                .unwrap_or_else(|_| "https://api.anthropic.com".to_string()),
            openai_upstream: std::env::var("CW_OPENAI_UPSTREAM")
                .unwrap_or_else(|_| "https://api.openai.com".to_string()),
            gemini_upstream: std::env::var("CW_GEMINI_UPSTREAM")
                .unwrap_or_else(|_| "https://generativelanguage.googleapis.com".to_string()),
            intercepts: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn broadcast(&self, ev: WsEvent) {
        let _ = self.ws_tx.send(ev);
    }
}

/// Builds the Intercepting Proxy Router (`127.0.0.1:4401`).
pub fn build_proxy_router(state: DaemonState) -> Router {
    Router::new()
        .route("/health", get(|| async { Json(json!({ "status": "ok" })) }))
        .fallback(handle_proxy_request)
        .layer(CorsLayer::permissive())
        .with_state(state)
}

/// Builds the Control & REST/WS API Router (`/api/v1/*` on `127.0.0.1:4400`).
pub fn build_api_router(state: DaemonState) -> Router {
    Router::new()
        .route("/api/v1/health", get(|| async { Json(json!({ "status": "ok" })) }))
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
            get(api_list_mutations).post(api_create_mutation),
        )
        .route(
            "/api/v1/sessions/:id/mutations/commit-drafts",
            post(api_commit_drafts),
        )
        .route(
            "/api/v1/sessions/:id/mutations/:mutation_id",
            delete(api_delete_mutation),
        )
        .route("/api/v1/sessions/:id/optimize", post(api_auto_optimize))
        .route("/api/v1/sessions/:id/replay", post(api_fork_replay))
        .route("/api/v1/ingest", post(api_ingest_payload))
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
        .layer(CorsLayer::permissive())
        .with_state(state)
}

async fn handle_proxy_request(
    State(state): State<DaemonState>,
    method: axum::http::Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let start_instant = Instant::now();
    let path_and_query = uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/");
    let path = uri.path();

    // Determine target upstream base URL and stripped path
    let (upstream_base, forward_path, default_provider) =
        if let Some(rest) = path_and_query.strip_prefix("/anthropic") {
            (
                state.anthropic_upstream.clone(),
                if rest.is_empty() { "/" } else { rest },
                ProviderKind::Anthropic,
            )
        } else if let Some(rest) = path_and_query.strip_prefix("/openai") {
            (
                state.openai_upstream.clone(),
                if rest.is_empty() { "/" } else { rest },
                ProviderKind::OpenAI,
            )
        } else if let Some(rest) = path_and_query.strip_prefix("/gemini") {
            (
                state.gemini_upstream.clone(),
                if rest.is_empty() { "/" } else { rest },
                ProviderKind::Gemini,
            )
        } else if path.contains("v1beta") || path.contains("generateContent") {
            (
                state.gemini_upstream.clone(),
                path_and_query,
                ProviderKind::Gemini,
            )
        } else if path.ends_with("/v1/messages") {
            (
                state.anthropic_upstream.clone(),
                path_and_query,
                ProviderKind::Anthropic,
            )
        } else {
            (
                state.openai_upstream.clone(),
                path_and_query,
                ProviderKind::OpenAI,
            )
        };

    // Parse JSON payload if POST/PUT
    let Ok(raw_payload) = serde_json::from_slice::<Value>(&body) else {
        return forward_raw_passthrough(&state, method, &upstream_base, forward_path, headers, body)
            .await;
    };

    let provider = detect_provider(path, &raw_payload);
    let _ = default_provider;

    // Resolve or create session
    let session_id = headers
        .get("x-cw-session-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| resolve_or_create_session_id(&state.store, provider, &raw_payload));

    let harness = headers
        .get("x-cw-harness")
        .and_then(|v| v.to_str().ok())
        .map(HarnessKind::from_str_lossy)
        .unwrap_or(HarnessKind::GenericProxy);

    let workspace_root = headers
        .get("x-cw-workspace")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let model = raw_payload
        .get("model")
        .and_then(|m| m.as_str())
        .unwrap_or("unknown-model")
        .to_string();

    let session = Session {
        id: session_id.clone(),
        parent_session_id: None,
        harness,
        workspace_root: workspace_root.clone(),
        model,
        provider,
        title: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };
    let _ = state.store.upsert_session(&session);

    let prev_turn = state.store.get_latest_turn(&session_id).ok().flatten();
    let seq_num = prev_turn
        .as_ref()
        .map(|t| t.sequence_number + 1)
        .unwrap_or(1);
    let prev_nodes = prev_turn.as_ref().map(|t| t.nodes.as_slice());

    // 1. Apply active deterministic mutations from the Session's Mutation Ledger
    let mut mutated_payload = raw_payload.clone();
    let active_mutations = state
        .store
        .list_mutations(&session_id)
        .unwrap_or_default();
    let mut_report =
        apply_mutations_to_payload(&mut mutated_payload, provider, &active_mutations, seq_num);

    // 2. Analyze turn & run Context Firewall
    let turn_id = format!("turn_{}_{}", session_id, seq_num);
    if let Ok((preflight_turn, _)) = analyze_turn_from_payload(
        turn_id.clone(),
        session_id.clone(),
        prev_turn.as_ref().map(|t| t.id.clone()),
        seq_num,
        &mutated_payload,
        provider,
        prev_nodes,
        None,
        workspace_root.as_deref(),
    ) {
        let fw_rules = state
            .store
            .list_firewall_rules(Some(&session_id))
            .unwrap_or_default();
        let verdict = evaluate_firewall(
            &mut mutated_payload,
            provider,
            &preflight_turn.nodes,
            &preflight_turn.kv_cache_report,
            &fw_rules,
            seq_num,
        );

        if let Some(paused_rule) = verdict.paused_by_rule {
            let intercept_id = format!("int_{}", &Uuid::new_v4().to_string()[..8]);
            let (tx, rx) = oneshot::channel();
            let info = PendingInterceptInfo {
                intercept_id: intercept_id.clone(),
                session_id: session_id.clone(),
                turn_seq: seq_num,
                rule_name: paused_rule.name.clone(),
                payload_preview: mutated_payload.clone(),
                created_at: Utc::now(),
            };
            state
                .intercepts
                .lock()
                .await
                .insert(intercept_id.clone(), ActiveIntercept { info: info.clone(), tx });
            state.broadcast(WsEvent::FirewallIntercepted { intercept: info });

            let timeout_secs = match paused_rule.action {
                cw_core::ir::FirewallAction::PauseForHumanApproval { timeout_secs } => {
                    timeout_secs as u64
                }
                _ => 120,
            };

            if let Ok(Ok(res)) =
                tokio::time::timeout(Duration::from_secs(timeout_secs), rx).await
            {
                if res.action == "abort" {
                    return (
                        StatusCode::FORBIDDEN,
                        Json(json!({
                            "error": {
                                "message": format!("Request aborted by contextWatcher Firewall rule '{}'", paused_rule.name),
                                "type": "context_watcher_firewall"
                            }
                        })),
                    )
                        .into_response();
                }
                if let Some(edited) = res.edited_payload {
                    mutated_payload = edited;
                }
            }
        }
    }

    // 3. Forward mutated payload to upstream provider (or local mock mode if header `x-cw-dry-run` is set)
    if headers.get("x-cw-dry-run").is_some() {
        let (mut final_turn, file_states) = analyze_turn_from_payload(
            turn_id,
            session_id.clone(),
            prev_turn.as_ref().map(|t| t.id.clone()),
            seq_num,
            &mutated_payload,
            provider,
            prev_nodes,
            None,
            workspace_root.as_deref(),
        )
        .unwrap();
        final_turn.token_usage.saved_by_mutations = mut_report.saved_tokens;
        final_turn.latency_ms = Some(start_instant.elapsed().as_millis() as u64);
        let _ = state.store.save_turn(
            &final_turn,
            Some(&raw_payload),
            Some(&mutated_payload),
            None,
        );
        let _ = state.store.save_file_states(&session_id, &file_states);
        state.broadcast(WsEvent::TurnCompleted {
            session_id: session_id.clone(),
            turn: Box::new(final_turn.clone()),
        });
        state.broadcast(WsEvent::FileContextStatusChanged {
            session_id,
            workspace_root,
            file_states,
        });
        return Json(json!({
            "cw_dry_run": true,
            "turn": final_turn,
            "mutated_payload": mutated_payload
        }))
        .into_response();
    }

    let target_url = format!("{}{}", upstream_base.trim_end_matches('/'), forward_path);
    let body_bytes = serde_json::to_vec(&mutated_payload).unwrap_or_else(|_| body.to_vec());

    let mut req_builder = state.http_client.request(method, &target_url);
    for (k, v) in &headers {
        let name = k.as_str().to_lowercase();
        if !matches!(
            name.as_str(),
            "host" | "content-length" | "x-cw-session-id" | "x-cw-harness" | "x-cw-workspace"
        ) {
            req_builder = req_builder.header(k, v);
        }
    }

    let upstream_resp = match req_builder.body(body_bytes).send().await {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": format!("Upstream proxy error: {e}") })),
            )
                .into_response();
        }
    };

    let status = upstream_resp.status();
    let resp_headers = upstream_resp.headers().clone();
    let is_sse = resp_headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|ct| ct.contains("text/event-stream"))
        .unwrap_or(false);

    if is_sse {
        // Stream SSE chunks directly to client while tapping usage metrics in background
        let stream = upstream_resp.bytes_stream();
        let state_clone = state.clone();
        let prev_nodes_owned = prev_nodes.map(|s| s.to_vec());

        let tapped_stream = stream.inspect(move |chunk_res| {
            if let Ok(bytes) = chunk_res {
                let text = String::from_utf8_lossy(bytes);
                if text.contains("\"usage\"") || text.contains("message_delta") {
                    let cached = extract_cached_tokens_from_sse(&text);
                    if let Ok((mut turn, file_states)) = analyze_turn_from_payload(
                        turn_id.clone(),
                        session_id.clone(),
                        prev_turn.as_ref().map(|t| t.id.clone()),
                        seq_num,
                        &mutated_payload,
                        provider,
                        prev_nodes_owned.as_deref(),
                        cached,
                        workspace_root.as_deref(),
                    ) {
                        turn.token_usage.saved_by_mutations = mut_report.saved_tokens;
                        turn.latency_ms = Some(start_instant.elapsed().as_millis() as u64);
                        let _ = state_clone.store.save_turn(
                            &turn,
                            Some(&raw_payload),
                            Some(&mutated_payload),
                            None,
                        );
                        let _ = state_clone.store.save_file_states(&session_id, &file_states);
                        state_clone.broadcast(WsEvent::TurnCompleted {
                            session_id: session_id.clone(),
                            turn: Box::new(turn),
                        });
                    }
                }
            }
        });

        let mut response = Response::new(Body::from_stream(tapped_stream));
        *response.status_mut() = status;
        *response.headers_mut() = resp_headers;
        response
    } else {
        let resp_bytes = upstream_resp.bytes().await.unwrap_or_default();
        let resp_json: Option<Value> = serde_json::from_slice(&resp_bytes).ok();
        let actual_cached = resp_json.as_ref().and_then(extract_cached_tokens_from_json);

        if let Ok((mut turn, file_states)) = analyze_turn_from_payload(
            turn_id,
            session_id.clone(),
            prev_turn.as_ref().map(|t| t.id.clone()),
            seq_num,
            &mutated_payload,
            provider,
            prev_nodes,
            actual_cached,
            workspace_root.as_deref(),
        ) {
            turn.token_usage.saved_by_mutations = mut_report.saved_tokens;
            turn.latency_ms = Some(start_instant.elapsed().as_millis() as u64);
            let _ = state.store.save_turn(
                &turn,
                Some(&raw_payload),
                Some(&mutated_payload),
                resp_json.as_ref(),
            );
            let _ = state.store.save_file_states(&session_id, &file_states);
            state.broadcast(WsEvent::TurnCompleted {
                session_id: session_id.clone(),
                turn: Box::new(turn),
            });
            state.broadcast(WsEvent::FileContextStatusChanged {
                session_id,
                workspace_root,
                file_states,
            });
        }

        let mut response = Response::new(Body::from(resp_bytes));
        *response.status_mut() = status;
        *response.headers_mut() = resp_headers;
        response
    }
}

async fn forward_raw_passthrough(
    state: &DaemonState,
    method: axum::http::Method,
    upstream_base: &str,
    forward_path: &str,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let target_url = format!("{}{}", upstream_base.trim_end_matches('/'), forward_path);
    let mut req = state.http_client.request(method, &target_url);
    for (k, v) in &headers {
        if k.as_str() != "host" && k.as_str() != "content-length" {
            req = req.header(k, v);
        }
    }
    match req.body(body).send().await {
        Ok(resp) => {
            let status = resp.status();
            let h = resp.headers().clone();
            let b = resp.bytes().await.unwrap_or_default();
            let mut r = Response::new(Body::from(b));
            *r.status_mut() = status;
            *r.headers_mut() = h;
            r
        }
        Err(e) => (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    }
}

fn resolve_or_create_session_id(
    store: &CwStore,
    provider: ProviderKind,
    _payload: &Value,
) -> String {
    if let Ok(sessions) = store.list_sessions()
        && let Some(latest) = sessions.iter().find(|s| s.provider == provider) {
            return latest.id.clone();
        }
    format!("sess_{}", &Uuid::new_v4().to_string()[..8])
}

fn extract_cached_tokens_from_json(resp: &Value) -> Option<u32> {
    let usage = resp.get("usage").or_else(|| resp.get("usageMetadata"))?;
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

fn extract_cached_tokens_from_sse(chunk_text: &str) -> Option<u32> {
    for line in chunk_text.lines() {
        if let Some(data) = line.strip_prefix("data: ")
            && let Ok(val) = serde_json::from_str::<Value>(data)
                && let Some(c) = extract_cached_tokens_from_json(&val) {
                    return Some(c);
                }
    }
    None
}

// ============================================================================
// Daemon Control REST & WebSocket Handlers
// ============================================================================

async fn api_list_sessions(State(state): State<DaemonState>) -> impl IntoResponse {
    let sessions = state.store.list_sessions().unwrap_or_default();
    Json(sessions)
}

async fn api_get_session(
    State(state): State<DaemonState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.store.get_session(&id) {
        Ok(Some(s)) => Json(json!(s)).into_response(),
        _ => (StatusCode::NOT_FOUND, Json(json!({ "error": "Session not found" }))).into_response(),
    }
}

#[derive(Serialize)]
struct SessionGraphResponse {
    session: Option<Session>,
    latest_turn: Option<ContextTurn>,
    turns: Vec<ContextTurn>,
    radix_graph: RadixGraph,
    mutations: Vec<MutationRule>,
    file_states: Vec<FileContextState>,
}

async fn api_get_session_graph(
    State(state): State<DaemonState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let session = state.store.get_session(&id).ok().flatten();
    let turns = state.store.list_turns(&id).unwrap_or_default();
    let latest_turn = turns.last().cloned();
    let radix_graph = match &latest_turn {
        Some(lt) => build_radix_graph(lt, &turns),
        None => RadixGraph::default(),
    };
    let mutations = state.store.list_mutations(&id).unwrap_or_default();
    let file_states = state.store.get_file_states(&id).unwrap_or_default();

    Json(SessionGraphResponse {
        session,
        latest_turn,
        turns,
        radix_graph,
        mutations,
        file_states,
    })
}

async fn api_list_turns(
    State(state): State<DaemonState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let turns = state.store.list_turns(&id).unwrap_or_default();
    Json(turns)
}

async fn api_get_turn_raw(
    State(state): State<DaemonState>,
    Path((_id, turn_id)): Path<(String, String)>,
) -> impl IntoResponse {
    match state.store.get_turn_raw_payload(&turn_id) {
        Ok(Some(v)) => Json(v).into_response(),
        _ => (StatusCode::NOT_FOUND, Json(json!({ "error": "Payload not found" }))).into_response(),
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
    Json(req): Json<CreateMutationRequest>,
) -> impl IntoResponse {
    let latest_turn = state.store.get_latest_turn(&id).ok().flatten();
    let (saved, kv_break) = match &latest_turn {
        Some(t) => estimate_mutation_impact(&req.op, &t.nodes),
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
    let _ = state.store.save_mutation(&rule);

    // If committed immediately and we have a latest turn with raw payload, re-evaluate latest turn in-place for instant UI feedback!
    recompute_latest_turn_with_mutations(&state, &id);

    let all_mutations = state.store.list_mutations(&id).unwrap_or_default();
    state.broadcast(WsEvent::MutationsUpdated {
        session_id: id,
        mutations: all_mutations,
    });
    Json(rule)
}

async fn api_commit_drafts(
    State(state): State<DaemonState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let count = state.store.commit_all_draft_mutations(&id).unwrap_or(0);
    recompute_latest_turn_with_mutations(&state, &id);
    let all_mutations = state.store.list_mutations(&id).unwrap_or_default();
    state.broadcast(WsEvent::MutationsUpdated {
        session_id: id,
        mutations: all_mutations,
    });
    Json(json!({ "committed": count }))
}

async fn api_delete_mutation(
    State(state): State<DaemonState>,
    Path((id, mutation_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let deleted = state.store.delete_mutation(&mutation_id).unwrap_or(false);
    recompute_latest_turn_with_mutations(&state, &id);
    let all_mutations = state.store.list_mutations(&id).unwrap_or_default();
    state.broadcast(WsEvent::MutationsUpdated {
        session_id: id,
        mutations: all_mutations,
    });
    Json(json!({ "deleted": deleted }))
}

/// 1-Click Context Optimizer: commits `ShadowDedupe` + `NormalizePrefix` and folds any detected failed debugging loops.
async fn api_auto_optimize(
    State(state): State<DaemonState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let mut created = Vec::new();
    if let Ok(Some(latest)) = state.store.get_latest_turn(&id) {
        for finding in &latest.lint_findings {
            if let Some(op) = &finding.suggested_mutation {
                let (saved, kv_break) = estimate_mutation_impact(op, &latest.nodes);
                let rule = MutationRule {
                    id: format!("mut_{}", &Uuid::new_v4().to_string()[..8]),
                    session_id: id.clone(),
                    op: op.clone(),
                    status: MutationStatus::Committed,
                    estimated_saved_tokens: saved,
                    estimated_kv_break_tokens: kv_break,
                    created_at: Utc::now(),
                };
                let _ = state.store.save_mutation(&rule);
                created.push(rule);
            }
        }
    }
    recompute_latest_turn_with_mutations(&state, &id);
    Json(json!({ "applied_mutations": created }))
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
    let turns = state.store.list_turns(&id).unwrap_or_default();
    let Some(source_turn) = (match &req.from_turn_id {
        Some(tid) => turns.iter().find(|t| &t.id == tid).cloned(),
        None => turns.last().cloned(),
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
    let workspace_root = session.as_ref().and_then(|s| s.workspace_root.clone());

    let mut rules = state.store.list_mutations(&id).unwrap_or_default();
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
    let fork_turn_id = format!("fork_{}_{}", source_turn.sequence_number, &Uuid::new_v4().to_string()[..6]);

    match analyze_turn_from_payload(
        fork_turn_id,
        id.clone(),
        Some(source_turn.id.clone()),
        source_turn.sequence_number,
        &payload,
        provider,
        Some(&source_turn.nodes),
        None,
        workspace_root.as_deref(),
    ) {
        Ok((mut fork_turn, _)) => {
            fork_turn.token_usage.saved_by_mutations = mut_report.saved_tokens;
            let _ = state
                .store
                .save_turn(&fork_turn, Some(&payload), Some(&payload), None);
            state.broadcast(WsEvent::TurnCompleted {
                session_id: id,
                turn: Box::new(fork_turn.clone()),
            });
            Json(json!({
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
    let _ = state.store.upsert_session(&session);

    let prev_turn = state.store.get_latest_turn(&req.session_id).ok().flatten();
    let seq_num = prev_turn
        .as_ref()
        .map(|t| t.sequence_number + 1)
        .unwrap_or(1);

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
            let _ = state.store.save_turn(
                &turn,
                Some(&req.payload),
                Some(&mutated_payload),
                None,
            );
            let _ = state.store.save_file_states(&req.session_id, &file_states);
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
    let _ = state.store.upsert_firewall_rule(&rule);
    Json(rule)
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
    ws.on_upgrade(move |socket| handle_ws_client(socket, state))
}

async fn handle_ws_client(mut socket: WebSocket, state: DaemonState) {
    let mut rx = state.ws_tx.subscribe();
    while let Ok(ev) = rx.recv().await {
        if let Ok(txt) = serde_json::to_string(&ev)
            && socket.send(Message::Text(txt)).await.is_err() {
                break;
            }
    }
}

fn recompute_latest_turn_with_mutations(state: &DaemonState, session_id: &str) {
    let Some(latest) = state.store.get_latest_turn(session_id).ok().flatten() else {
        return;
    };
    let Some(mut raw_payload) = state.store.get_turn_raw_payload(&latest.id).ok().flatten() else {
        return;
    };
    let session = state.store.get_session(session_id).ok().flatten();
    let provider = session
        .as_ref()
        .map(|s| s.provider)
        .unwrap_or(ProviderKind::Anthropic);
    let workspace_root = session.as_ref().and_then(|s| s.workspace_root.clone());

    let rules = state.store.list_mutations(session_id).unwrap_or_default();
    let mut_report =
        apply_mutations_to_payload(&mut raw_payload, provider, &rules, latest.sequence_number);

    if let Ok((mut updated_turn, file_states)) = analyze_turn_from_payload(
        latest.id.clone(),
        session_id.to_string(),
        latest.parent_turn_id.clone(),
        latest.sequence_number,
        &raw_payload,
        provider,
        None,
        Some(latest.token_usage.cached_tokens),
        workspace_root.as_deref(),
    ) {
        updated_turn.token_usage.saved_by_mutations = mut_report.saved_tokens;
        let _ = state.store.save_turn(
            &updated_turn,
            None,
            Some(&raw_payload),
            None,
        );
        let _ = state.store.save_file_states(session_id, &file_states);
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

    #[tokio::test]
    async fn test_proxy_dry_run_and_api_mutations_and_replay() {
        let store = CwStore::open_in_memory().unwrap();
        let state = DaemonState::new(store);
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
            .header("x-cw-dry-run", "1")
            .body(Body::from(serde_json::to_vec(&req_payload).unwrap()))
            .unwrap();

        let resp = proxy_app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // 2. Call 1-Click Optimize on the control API
        let opt_req = Request::builder()
            .method("POST")
            .uri("/api/v1/sessions/test_sess/optimize")
            .body(Body::empty())
            .unwrap();
        let opt_resp = api_app.clone().oneshot(opt_req).await.unwrap();
        assert_eq!(opt_resp.status(), StatusCode::OK);

        // 3. Call Time-Travel Fork & Replay on the control API
        let replay_req = Request::builder()
            .method("POST")
            .uri("/api/v1/sessions/test_sess/replay")
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
}
