pub mod engine;
pub mod firewall;
pub mod hud;
pub mod mutations;
pub mod sessions;
pub mod transform;
pub mod ws;

use axum::{
    Json, Router, middleware,
    routing::{delete, get, post},
};
use serde_json::json;

pub use engine::{EngineSlotsQuery, SpliceRequest, SwarmQueryParams};
pub use mutations::recompute_latest_turn_in_store;
pub use transform::TransformContextRequest;

use crate::{
    security::{control_guard, local_cors_layer, local_only_guard},
    state::DaemonState,
};

/// Builds the Control & REST/WS API Router (`/api/v1/*` on `127.0.0.1:4400`).
pub fn build_api_router(state: DaemonState) -> Router {
    Router::new()
        .route(
            "/api/v1/health",
            get(|| async { Json(json!({ "status": "ok" })) }),
        )
        .route("/api/v1/sessions", get(sessions::api_list_sessions))
        .route("/api/v1/sessions/:id", get(sessions::api_get_session))
        .route(
            "/api/v1/sessions/:id/graph",
            get(sessions::api_get_session_graph),
        )
        .route("/api/v1/sessions/:id/turns", get(sessions::api_list_turns))
        .route(
            "/api/v1/sessions/:id/turns/:turn_id/raw",
            get(sessions::api_get_turn_raw),
        )
        .route(
            "/api/v1/sessions/:id/files",
            get(sessions::api_get_session_files),
        )
        .route(
            "/api/v1/sessions/:id/mutations",
            get(mutations::api_list_mutations)
                .post(mutations::api_create_mutation)
                .delete(mutations::api_clear_mutations),
        )
        .route(
            "/api/v1/sessions/:id/mutations/commit-drafts",
            post(mutations::api_commit_drafts),
        )
        .route(
            "/api/v1/sessions/:id/mutations/:mutation_id",
            delete(mutations::api_delete_mutation),
        )
        .route(
            "/api/v1/sessions/:id/transform",
            post(transform::api_transform_context),
        )
        .route(
            "/api/v1/sessions/:id/optimize",
            post(mutations::api_auto_optimize),
        )
        .route(
            "/api/v1/sessions/:id/effectiveness",
            get(hud::api_get_session_effectiveness),
        )
        .route("/api/v1/effectiveness", get(hud::api_get_all_effectiveness))
        .route("/api/v1/pulses", get(hud::api_list_recent_pulses))
        .route(
            "/api/v1/sessions/:id/pulses",
            get(hud::api_get_session_pulse),
        )
        .route("/api/v1/swarms", get(engine::api_list_swarms))
        .route(
            "/api/v1/swarms/:group_id/graph",
            get(engine::api_get_swarm_graph),
        )
        .route("/api/v1/engine/slots", get(engine::api_get_engine_slots))
        .route(
            "/api/v1/engine/slots/:slot_id/splice",
            post(engine::api_splice_engine_slot),
        )
        .route("/api/v1/hud", get(hud::api_get_active_hud))
        .route("/api/v1/sessions/:id/hud", get(hud::api_get_session_hud))
        .route("/api/v1/sessions/:id/ecg", get(hud::api_get_session_ecg))
        .route(
            "/api/v1/sessions/:id/sync-back",
            post(sessions::api_sync_transcript_back),
        )
        .route(
            "/api/v1/sessions/:id/replay",
            post(sessions::api_fork_replay),
        )
        .route("/api/v1/ingest", post(sessions::api_ingest_payload))
        .route(
            "/api/v1/autopilot",
            get(hud::api_get_autopilot).post(hud::api_set_autopilot),
        )
        .route(
            "/api/v1/firewall/rules",
            get(firewall::api_list_firewall_rules).post(firewall::api_upsert_firewall_rule),
        )
        .route(
            "/api/v1/firewall/intercepts",
            get(firewall::api_list_intercepts),
        )
        .route(
            "/api/v1/firewall/intercepts/:intercept_id/resolve",
            post(firewall::api_resolve_intercept),
        )
        .route("/api/v1/ws", get(ws::api_ws_handler))
        .layer(middleware::from_fn_with_state(state.clone(), control_guard))
        .layer(local_cors_layer())
        .layer(middleware::from_fn(local_only_guard))
        .with_state(state)
}
