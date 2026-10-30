use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use cw_core::ir::ContextTurn;
use serde_json::{Value, json};

use crate::state::DaemonState;

/// Validates that a dry-run preview payload is a valid JSON object.
pub fn validate_dry_run_payload(body: &[u8]) -> Result<(), (StatusCode, &'static str)> {
    if !serde_json::from_slice::<Value>(body).is_ok_and(|value| value.is_object()) {
        Err((
            StatusCode::BAD_REQUEST,
            "Preview requires a supported JSON object",
        ))
    } else {
        Ok(())
    }
}

/// Finalizes a dry-run preview turn in the store and returns the preview JSON response.
pub fn render_dry_run_response(
    state: &DaemonState,
    mut turn: ContextTurn,
    outbound: Value,
) -> Response {
    turn.outcome = Some("preview".into());
    let _ = state.store.save_turn(&turn, None, Some(&outbound), None);
    Json(json!({
        "cw_dry_run": true,
        "turn": turn,
        "mutated_payload": outbound
    }))
    .into_response()
}
