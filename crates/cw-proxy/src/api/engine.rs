use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use cw_core::{
    ir::{ContextTurn, NodeKind, Session},
    slot_bridge::{
        DEFAULT_PAGED_ATTENTION_BLOCK_SIZE, EngineSlotInfo, assign_deterministic_slot,
        plan_slot_splicing,
    },
    swarm::{analyze_swarm_prefix_sharing, build_swarm_radix_graph},
};
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

use crate::state::DaemonState;

#[derive(Debug, Deserialize)]
pub struct SwarmQueryParams {
    pub project: Option<String>,
    pub model: Option<String>,
    pub min_members: Option<usize>,
}

pub async fn api_list_swarms(
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

pub async fn api_get_swarm_graph(
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

pub async fn api_get_engine_slots(
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

pub async fn api_splice_engine_slot(
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
