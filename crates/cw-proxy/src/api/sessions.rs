use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use chrono::Utc;
use cw_core::{
    analyze_turn_from_payload,
    ir::{
        ContextTurn, FileContextState, HarnessKind, MutationOp, MutationRule, MutationStatus,
        ProviderKind, Session,
    },
    local_llm::{
        CostAndPerfReport, LocalModelAnalysis, analyze_local_model_kv, calculate_cost_and_perf,
    },
    mutation::apply_mutations_to_payload_with_policy,
    normalizer::detect_provider,
    optimizer::OptimizationStrategyPlan,
    radix::{RadixGraph, build_radix_graph},
};
use cw_harnesses::sync_mutations_to_transcript_on_disk;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    api::mutations::preview_turn,
    state::{DaemonState, WsEvent},
};

#[derive(Deserialize)]
pub struct SessionPage {
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

pub async fn api_list_sessions(
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

pub async fn api_get_session(
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
pub struct SessionGraphResponse {
    pub revision: String,
    pub mutation_preview: Option<ContextTurn>,
    pub session: Option<Session>,
    pub latest_turn: Option<ContextTurn>,
    pub turns: Vec<ContextTurn>,
    pub radix_graph: RadixGraph,
    pub mutations: Vec<MutationRule>,
    pub file_states: Vec<FileContextState>,
    pub optimization_plans: Vec<OptimizationStrategyPlan>,
    pub optimization_plan_status: Option<String>,
    pub timeline_has_more: bool,
    pub cost_and_perf: Option<CostAndPerfReport>,
    pub local_model_analysis: Option<LocalModelAnalysis>,
    pub autopilot_config: cw_core::autopilot::AutoPilotConfig,
    pub transcript_source_path: Option<String>,
}

#[derive(Deserialize, Default)]
pub struct SessionGraphQuery {
    /// Return only the latest turn for dashboard views that do not need full history.
    pub compact: Option<bool>,
    /// Limit timeline nodes to one page; older pages are requested explicitly.
    pub timeline_limit: Option<usize>,
    pub timeline_before_turn: Option<u32>,
    /// Defer expensive strategy planning until the Optimize view is opened.
    pub include_optimization_plans: Option<bool>,
}

pub async fn api_get_session_graph(
    State(state): State<DaemonState>,
    Path(id): Path<String>,
    Query(query): Query<SessionGraphQuery>,
) -> impl IntoResponse {
    let session = state.store.get_session(&id).ok().flatten();
    let compact = query.compact == Some(true);
    let snapshot = if compact {
        state
            .store
            .latest_turn_snapshot(&id)
            .map(|(latest, rules, revision)| (latest.into_iter().collect(), rules, revision))
    } else {
        state.store.session_history_snapshot(&id)
    };
    let (full_turns, mutations, revision) = match snapshot {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response();
        }
    };
    let latest_turn = full_turns.last().cloned();
    let timeline_page = query.timeline_limit.is_some();
    let radix_graph = match (&latest_turn, timeline_page) {
        (Some(_), true) => RadixGraph::default(),
        (Some(lt), false) => build_radix_graph(lt, &full_turns),
        (None, _) => RadixGraph::default(),
    };
    let (optimization_plans, optimization_plan_status) = if query.include_optimization_plans
        == Some(false)
        || query.timeline_before_turn.is_some()
    {
        (Vec::new(), None)
    } else {
        match (session.as_ref().map(|s| s.harness), &latest_turn) {
            (Some(HarnessKind::Copilot), _) => (
                Vec::new(),
                Some("Copilot sessions are imported for inspection only; CW cannot deliver mutations to Copilot.".to_string()),
            ),
            (_, None) => (Vec::new(), Some("No recorded request is available for optimization.".to_string())),
            (_, Some(turn)) if !turn.analysis_complete => (
                Vec::new(),
                Some("Request analysis is incomplete; CW cannot prove which nodes are safe to edit.".to_string()),
            ),
            (_, Some(turn)) if turn.normalization_provider.is_none() => (
                Vec::new(),
                Some("The request provider could not be identified; CW abstained.".to_string()),
            ),
            (_, Some(turn)) => {
                let raw = state.store.get_turn_raw_payload(&turn.id).ok().flatten();
                match raw {
                    None => (
                        Vec::new(),
                        Some("The original request payload is unavailable; CW cannot verify safe edit targets.".to_string()),
                    ),
                    Some(raw) if !cw_core::normalizer::supports_mutation_payload(
                        &raw,
                        turn.normalization_provider.unwrap(),
                    ) => (
                        Vec::new(),
                        Some("This request format contains unsupported or opaque content; CW abstained.".to_string()),
                    ),
                    Some(raw) => {
                        let signed = cw_core::mutation::has_signed_content(&raw);
                        let pin_policy =
                            state.store.get_pin_context_policy(&id).unwrap_or_default();
                        let plans = cw_core::optimizer::build_optimization_plans_for_payload_with_mutations(
                            turn,
                            &raw,
                            turn.normalization_provider.unwrap(),
                            &mutations,
                            &pin_policy,
                            session
                                .as_ref()
                                .and_then(|session| session.workspace_root.as_deref()),
                        );
                        let status = if plans.is_empty() {
                            Some(if signed {
                                if turn.normalization_provider == Some(ProviderKind::Anthropic)
                                    && cw_core::mutation::has_qualified_anthropic_thinking_signatures(&raw)
                                {
                                    "Signed thinking can bind its preceding request prefix. CW found no eligible target after the last signed block, so it left the request unchanged.".to_string()
                                } else {
                                    "This request contains a signature format without a verified local edit boundary; CW preserved it and abstained.".to_string()
                                }
                            } else {
                                "CW found no eligible cleanup candidates in this request.".to_string()
                            })
                        } else if signed {
                            Some("Signed blocks and any bound request prefix are preserved. Plans include only verified targets after the last signed block.".to_string())
                        } else {
                            None
                        };
                        (plans, status)
                    }
                }
            }
        }
    };

    let mutation_preview = latest_turn.as_ref().and_then(|latest| {
        let raw = state
            .store
            .get_turn_raw_payload(&latest.id)
            .ok()
            .flatten()?;
        let provider = latest
            .normalization_provider
            .or_else(|| session.as_ref().map(|session| session.provider))?;
        preview_turn(latest, &raw, &mutations, provider)
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
    let turns_lightweight: Vec<ContextTurn> = if compact {
        Vec::new()
    } else {
        full_turns
            .into_iter()
            .map(|mut turn| {
                turn.nodes.clear();
                turn
            })
            .collect()
    };

    let file_states = state.store.get_file_states(&id).unwrap_or_default();
    let autopilot_config = state.store.get_autopilot_config().unwrap_or_default();
    let transcript_source_path = state
        .store
        .get_session_source_path(&id)
        .ok()
        .flatten()
        .map(|(p, _)| p);

    let (mut mutation_preview, latest_turn) = if compact {
        (mutation_preview.or_else(|| latest_turn.clone()), None)
    } else {
        (mutation_preview, latest_turn)
    };
    let timeline_has_more =
        if let (Some(preview), Some(limit)) = (&mut mutation_preview, query.timeline_limit) {
            slice_timeline_page(preview, limit, query.timeline_before_turn)
        } else {
            false
        };
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
        optimization_plan_status,
        timeline_has_more,
        cost_and_perf,
        local_model_analysis,
        autopilot_config,
        transcript_source_path,
    })
    .into_response()
}

fn slice_timeline_page(turn: &mut ContextTurn, limit: usize, before_turn: Option<u32>) -> bool {
    let mut turn_sequences: Vec<u32> = turn
        .nodes
        .iter()
        .map(|node| node.turn_seq.max(1))
        .filter(|sequence| before_turn.is_none_or(|before| *sequence < before))
        .collect();
    turn_sequences.sort_unstable();
    turn_sequences.dedup();
    let limit = limit.clamp(1, 50);
    let start = turn_sequences.len().saturating_sub(limit);
    let has_more = start > 0;
    let page = &turn_sequences[start..];
    turn.nodes
        .retain(|node| page.binary_search(&node.turn_seq.max(1)).is_ok());
    has_more
}

pub async fn api_sync_transcript_back(
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

pub async fn api_list_turns(
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

pub async fn api_get_turn_raw(
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
pub struct FilesQuery {
    pub workspace_root: Option<String>,
}

pub async fn api_get_session_files(
    State(state): State<DaemonState>,
    Path(id): Path<String>,
    Query(q): Query<FilesQuery>,
) -> impl IntoResponse {
    let _ = q.workspace_root;
    let files = state.store.get_file_states(&id).unwrap_or_default();
    Json(files)
}

#[derive(Deserialize)]
pub struct ReplayRequest {
    pub from_turn_id: Option<String>,
    pub additional_mutations: Option<Vec<MutationOp>>,
}

pub async fn api_fork_replay(
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

    let mut_report = apply_mutations_to_payload_with_policy(
        &mut payload,
        provider,
        &rules,
        source_turn.sequence_number,
        &state.store.get_pin_context_policy(&id).unwrap_or_default(),
    );
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
            fork_turn.applied_mutation_ids = mut_report.applied_rule_ids;
            fork_turn.skipped_mutations = mut_report.skipped_rules;
            Json(json!({
                "preview": true,
                "upstream_executed": false,
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
pub struct IngestRequest {
    pub session_id: String,
    pub harness: Option<String>,
    pub provider: Option<String>,
    pub workspace_root: Option<String>,
    pub model: Option<String>,
    pub payload: Value,
}

pub async fn api_ingest_payload(
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
    let mut_report = apply_mutations_to_payload_with_policy(
        &mut mutated_payload,
        provider,
        &active_mutations,
        seq_num,
        &state
            .store
            .get_pin_context_policy(&req.session_id)
            .unwrap_or_default(),
    );

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
            turn.applied_mutation_ids = mut_report.applied_rule_ids;
            turn.skipped_mutations = mut_report.skipped_rules;
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

pub async fn api_end_session(
    State(state): State<DaemonState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let Ok(session_opt) = state.store.get_session(&id) else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to fetch session").into_response();
    };
    let Some(session) = session_opt else {
        return (StatusCode::NOT_FOUND, "Session not found").into_response();
    };

    // Durably persist session termination in the store to prevent reopening or horizon reversion
    if let Err(error) = state
        .store
        .mark_session_terminated(&id, Some("api_explicit_end"))
    {
        return (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response();
    }

    let Ok(turns) = state.store.list_turns(&id) else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to fetch turns").into_response();
    };
    let Ok(decisions) = state.store.list_shadow_decision_evidence(Some(&id)) else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to fetch decisions",
        )
            .into_response();
    };

    let mut all_outcomes = Vec::new();
    for t in &turns {
        let (t_provider, t_model, t_pricing, _verified) = state
            .store
            .resolve_turn_model_and_pricing(t, session.provider);
        let o = cw_core::calibration::ObservedProviderOutcome::from_completed_turn(
            t,
            t_provider,
            &t_model,
            t_pricing.as_ref(),
            t.created_at,
        );
        all_outcomes.push(o);
    }

    let mut updated_count = 0usize;
    for dec in &decisions {
        let base_outcome = all_outcomes
            .iter()
            .find(|o| o.request_index == dec.request_index);
        let pho = cw_core::calibration::evaluate_prospective_horizon(
            dec,
            base_outcome,
            &all_outcomes,
            dec.forecast_horizon_turns,
            true, // explicit session end signal
        );
        if state
            .store
            .save_prospective_horizon_observation(&pho)
            .is_ok()
        {
            updated_count += 1;
        }
    }

    Json(json!({
        "status": "terminated",
        "session_id": id,
        "horizons_updated": updated_count
    }))
    .into_response()
}
