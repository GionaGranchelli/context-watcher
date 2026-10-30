use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use cw_core::ir::FirewallRule;
use serde_json::json;

use crate::state::{DaemonState, InterceptResolution, PendingInterceptInfo, WsEvent};

pub async fn api_list_firewall_rules(State(state): State<DaemonState>) -> impl IntoResponse {
    let rules = state.store.list_firewall_rules(None).unwrap_or_default();
    Json(rules)
}

pub async fn api_upsert_firewall_rule(
    State(state): State<DaemonState>,
    Json(rule): Json<FirewallRule>,
) -> impl IntoResponse {
    match state.store.upsert_firewall_rule(&rule) {
        Ok(()) => Json(rule).into_response(),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response(),
    }
}

pub async fn api_list_intercepts(State(state): State<DaemonState>) -> impl IntoResponse {
    let map = state.intercepts.lock().await;
    let list: Vec<PendingInterceptInfo> = map.values().map(|i| i.info.clone()).collect();
    Json(list)
}

pub async fn api_resolve_intercept(
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
