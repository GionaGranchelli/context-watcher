use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use cw_core::{
    autopilot::AutoPilotConfig,
    hud::{evaluate_attention_ecg, generate_context_hud_summary},
};
use serde_json::json;

use crate::state::DaemonState;

pub async fn api_get_autopilot(State(state): State<DaemonState>) -> impl IntoResponse {
    let cfg = state.store.get_autopilot_config().unwrap_or_default();
    Json(cfg)
}

pub async fn api_set_autopilot(
    State(state): State<DaemonState>,
    Json(cfg): Json<AutoPilotConfig>,
) -> impl IntoResponse {
    match state.store.set_autopilot_config(&cfg) {
        Ok(()) => Json(json!({"autopilot_config": cfg})).into_response(),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response(),
    }
}

pub async fn api_get_session_effectiveness(
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

pub async fn api_get_all_effectiveness(State(state): State<DaemonState>) -> impl IntoResponse {
    match state.store.list_mutation_effectiveness(None) {
        Ok(records) => Json(json!({ "effectiveness": records })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

pub async fn api_list_recent_pulses(State(state): State<DaemonState>) -> impl IntoResponse {
    let pulses = state
        .store
        .list_recent_ambient_pulses(50)
        .unwrap_or_default();
    Json(json!({ "pulses": pulses }))
}

pub async fn api_get_session_pulse(
    State(state): State<DaemonState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let pulse = state.store.get_latest_pulse_for_session(&id).ok().flatten();
    Json(json!({ "session_id": id, "pulse": pulse }))
}

pub async fn api_get_active_hud(State(state): State<DaemonState>) -> impl IntoResponse {
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

pub async fn api_get_session_hud(
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

pub async fn api_get_session_ecg(
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
