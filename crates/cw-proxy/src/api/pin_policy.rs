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
    let session = match state.store.get_session(&id) {
        Ok(Some(session)) => session,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let result = (|| -> Result<_, cw_store::StoreError> {
        let policy = state.store.get_pin_context_policy(&id)?;
        let events = state.store.list_pin_context_policy_events(&id)?;
        let rules = state.store.list_mutations(&id)?;
        let config = state.store.get_autopilot_config()?;
        let terminated = state.store.is_session_terminated(&id)?;
        let latest = state.store.get_latest_turn(&id)?;
        let preview_provider = latest
            .as_ref()
            .and_then(|turn| turn.normalization_provider)
            .unwrap_or(session.provider);
        let raw = latest
            .as_ref()
            .map(|turn| state.store.get_turn_raw_payload(&turn.id))
            .transpose()?
            .flatten();
        let preview = raw.map(|mut raw| {
            let mut report = cw_core::mutation::apply_mutations_to_payload_with_policy(
                &mut raw,
                preview_provider,
                &rules,
                latest
                    .as_ref()
                    .map(|turn| turn.sequence_number)
                    .unwrap_or(1),
                &if terminated {
                    cw_core::mutation::PinContextPolicy::default()
                } else {
                    cw_core::mutation::PinContextPolicy::explicit_opt_in(&id)
                },
            );
            if cw_core::validator::validate_protocol_invariants(&raw, preview_provider).is_err() {
                report.applied_rule_ids.clear();
            }
            report
        });
        let pins:Vec<_>=rules.iter().filter_map(|rule| {
            let cw_core::ir::MutationOp::PinContext {label,content,..}=&rule.op else {return None};
            Some(json!({"rule_id":rule.id,"label":label,"content":content,"status":rule.status,
                "eligible_in_latest_request_preview_after_opt_in":preview.as_ref().map(|report|report.applied_rule_ids.contains(&rule.id)),
                "currently_authorized":policy.is_enabled_for(&id) && session.provider!=cw_core::ir::ProviderKind::Gemini && rule.status==cw_core::ir::MutationStatus::Committed,
            }))
        }).collect();
        let may_generate = config.enabled
            && !(config.shadow_policy.enabled && config.shadow_policy.strict_shadow_only)
            && (config.auto_cache_safe_pin || config.auto_heal_low_health)
            && !terminated
            && session.provider != cw_core::ir::ProviderKind::Gemini;
        Ok(
            json!({"session_id":id,"enabled":policy.is_enabled_for(&id),"events":events,"risk_warning":RISK_WARNING,
                "activation_scope":"All eligible stored and future PinContext rules in this session until revocation or session end; not approval of individual content",
                "stored_pins":pins,"preview_basis":"Latest recorded request only; null eligibility means no request is available; future request shape must be checked again",
                "automatic_generation":{"autopilot_enabled":config.enabled,"auto_cache_safe_pin":config.auto_cache_safe_pin,
                    "auto_heal_low_health":config.auto_heal_low_health,"heal_strategy":config.heal_strategy,
                    "may_generate_after_opt_in":may_generate,"currently_permitted":may_generate && policy.is_enabled_for(&id)},
                "strategy_application_may_generate_after_opt_in":!terminated && session.provider!=cw_core::ir::ProviderKind::Gemini,
                "measured_savings":null,
            }),
        )
    })();
    match result {
        Ok(body) => Json(body).into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
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
