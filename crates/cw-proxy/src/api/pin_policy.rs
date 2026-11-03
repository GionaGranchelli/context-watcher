use crate::state::DaemonState;
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::json;

pub const RISK_WARNING: &str = "PinContext reinforcement can add tokens, increase local latency or cloud cost, disrupt cache reuse, and change agent behavior. Benefits are unproven. This is not protection from pruning.";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetPinContextPolicy {
    pub enabled: bool,
    #[serde(default)]
    pub acknowledge_risk: bool,
}

pub async fn get_policy(State(state): State<DaemonState>, Path(id): Path<String>) -> Response {
    match state.store.get_session(&id) {
        Ok(Some(_)) => {}
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
    match (state.store.get_pin_context_policy(&id), state.store.list_pin_context_policy_events(&id)) {
        (Ok(policy), Ok(events)) => Json(json!({
            "session_id": id, "enabled": policy.is_enabled_for(&id), "events": events, "risk_warning": RISK_WARNING,
        })).into_response(),
        _ => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

pub async fn set_policy(
    State(state): State<DaemonState>,
    Path(id): Path<String>,
    Json(req): Json<SetPinContextPolicy>,
) -> Response {
    if !cw_core::control::is_valid_session_id(&id) || (req.enabled && !req.acknowledge_risk) {
        return (StatusCode::BAD_REQUEST, Json(json!({"error":"Explicit risk acknowledgement and a valid session ID are required", "risk_warning":RISK_WARNING}))).into_response();
    }
    match state.store.get_session(&id) {
        Ok(Some(_)) => {}
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
    match state
        .store
        .set_pin_context_policy(&id, req.enabled, req.acknowledge_risk)
    {
        Ok(()) => {}
        Err(
            error @ (cw_store::StoreError::PinContextConsentRequired
            | cw_store::StoreError::PinContextSessionTerminated),
        ) => {
            return (
                StatusCode::CONFLICT,
                Json(json!({"error":error.to_string(), "risk_warning":RISK_WARNING})),
            )
                .into_response();
        }
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
    get_policy(State(state), Path(id)).await
}
