use axum::{
    Json,
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use cw_core::{
    analyze_turn_from_payload,
    autopilot::{generate_ambient_savings_pulse, should_emit_savings_pulse},
    ir::{ContextTurn, MutationOp, MutationRule, MutationStatus, ProviderKind},
    mutation::{apply_mutations_to_payload, estimate_mutation_impact},
    optimizer::resolve_strategy_alias,
};
use cw_store::CwStore;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::state::{DaemonState, WsEvent};

#[derive(Deserialize)]
pub struct CreateMutationRequest {
    pub op: MutationOp,
    pub status: Option<MutationStatus>,
}

pub async fn api_list_mutations(
    State(state): State<DaemonState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let mutations = state.store.list_mutations(&id).unwrap_or_default();
    Json(mutations)
}

pub async fn api_create_mutation(
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
) -> Response {
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
                    Json(json!({"changed": count, "committed": count, "deleted_count": count})),
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

pub async fn api_commit_drafts(
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

pub async fn api_delete_mutation(
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
pub struct ClearMutationsQuery {
    pub only_pins: Option<bool>,
    pub label: Option<String>,
}

pub async fn api_clear_mutations(
    State(state): State<DaemonState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<ClearMutationsQuery>,
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
    let ids: Vec<_> = rules
        .into_iter()
        .filter(|r| {
            !q.only_pins.unwrap_or(true)
                || matches!(
                    &r.op,
                    MutationOp::PinContext { label, .. }
                        if q.label.as_ref().is_none_or(|f| label.to_lowercase().contains(&f.trim().to_lowercase()))
                )
        })
        .map(|r| r.id)
        .collect();
    mutation_write_result(
        &state,
        &id,
        state
            .store
            .change_mutations_at_revision(&id, expected, &ids, false),
    )
}

#[derive(Deserialize, Default)]
pub struct OptimizeRequest {
    pub strategy_id: Option<String>,
    pub selected_item_ids: Option<Vec<String>>,
    pub status: Option<MutationStatus>,
}

/// Applies either a specific `OptimizationStrategyPlan` (filtered by `selected_item_ids` checkboxes)
/// or all active lint suggestions if called without a body.
pub async fn api_auto_optimize(
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
    (
        [(
            "x-cw-revision",
            state.store.session_revision(&id).unwrap_or_default(),
        )],
        Json(json!({
            "applied_count": created.len(),
            "estimated_saved_tokens": created.iter().map(|r| r.estimated_saved_tokens.max(0) as u64).sum::<u64>(),
            "applied_mutations": created
        })),
    )
        .into_response()
}

pub fn recompute_latest_turn_in_store(store: &CwStore, session_id: &str) -> Option<ContextTurn> {
    let (turns, rules, _) = store.session_history_snapshot(session_id).ok()?;
    let latest = turns.last()?;
    let raw = store.get_turn_raw_payload(&latest.id).ok().flatten()?;
    let provider = store.get_session(session_id).ok().flatten()?.provider;
    preview_turn(latest, &raw, &rules, provider)
}

pub fn preview_turn(
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

pub fn recompute_latest_turn_with_mutations(state: &DaemonState, session_id: &str) {
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
