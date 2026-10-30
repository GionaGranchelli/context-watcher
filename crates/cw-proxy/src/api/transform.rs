use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use cw_core::ir::ProviderKind;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::state::DaemonState;

#[derive(Deserialize)]
pub struct TransformContextRequest {
    pub messages: Value,
    pub provider: Option<String>,
}

pub async fn api_transform_context(
    State(state): State<DaemonState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(req): Json<TransformContextRequest>,
) -> Response {
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
