pub mod forwarding;
pub mod preview;
pub mod recorder;
pub mod routing;

use axum::{
    Json, Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::Utc;
use cw_core::{
    analyze_turn_from_payload,
    autopilot::evaluate_autopilot_policies_with_policy,
    ir::{HarnessKind, ProviderKind, Session},
    local_llm::strip_internal_cw_fields,
    mutation::{apply_mutations_to_payload_with_policy, evaluate_firewall, has_signed_content},
    normalizer::detect_provider,
    validator::validate_protocol_invariants,
};
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::{
    api::transform::api_transform_context,
    proxy::{
        forwarding::forward_to_upstream,
        preview::{render_dry_run_response, validate_dry_run_payload},
        recorder::{InterceptCleanup, Recorder},
        routing::resolve_upstream_target,
    },
    security::{local_cors_layer, local_only_guard},
    state::{
        ActiveIntercept, ApprovalAction, DaemonState, InterceptResolution, PendingInterceptInfo,
        WsEvent,
    },
};

fn fallback_invalid_mutation(
    raw: &Value,
    outbound: &mut Value,
    provider: ProviderKind,
    applied: &mut Vec<String>,
    skipped: &mut Vec<cw_core::ir::SkippedMutation>,
) {
    if validate_protocol_invariants(outbound, provider).is_err()
        && validate_protocol_invariants(raw, provider).is_ok()
    {
        *outbound = raw.clone();
        skipped.extend(
            applied
                .drain(..)
                .map(|rule_id| cw_core::ir::SkippedMutation {
                    rule_id,
                    reason: "PostMutationValidationFailed".into(),
                }),
        );
    }
}

/// Builds the Intercepting Proxy Router (`127.0.0.1:4401`).
pub fn build_proxy_router(state: DaemonState) -> Router {
    Router::new()
        .route("/health", get(|| async { Json(json!({ "status": "ok" })) }))
        .route(
            "/api/v1/sessions/:id/transform",
            post(api_transform_context),
        )
        .fallback(handle_proxy_request)
        .layer(local_cors_layer())
        .layer(axum::middleware::from_fn(local_only_guard))
        .with_state(state)
}

pub async fn handle_proxy_request(
    State(state): State<DaemonState>,
    method: axum::http::Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let started = Instant::now();
    if headers.contains_key("x-cw-dry-run")
        && let Err(err) = validate_dry_run_payload(&body)
    {
        return err.into_response();
    }

    let target = resolve_upstream_target(&state, &uri);
    let mut outbound_body = body.to_vec();
    let mut recorder = None;

    'capture: {
        if let Ok(raw) = serde_json::from_slice::<Value>(&body)
            && raw.is_object()
        {
            let provider = detect_provider(uri.path(), &raw);
            let authorized = headers
                .get("x-cw-control-token")
                .and_then(|v| v.to_str().ok())
                == Some(state.control_token.as_str());
            let explicit = headers.get("x-cw-session-id").and_then(|v| v.to_str().ok());
            if explicit.is_some() && !authorized {
                return (
                    StatusCode::FORBIDDEN,
                    "Session-bound requests require the local control credential",
                )
                    .into_response();
            }
            if explicit.is_some_and(|id| !cw_core::control::is_valid_session_id(id)) {
                return (StatusCode::BAD_REQUEST, "Invalid cw session ID").into_response();
            }
            let is_conversation_payload = raw.get("messages").is_some()
                || raw.get("input").is_some()
                || raw.get("contents").is_some()
                || raw.get("prompt").is_some();
            if !is_conversation_payload && !headers.contains_key("x-cw-dry-run") {
                break 'capture;
            }
            let session_id = explicit
                .map(str::to_owned)
                .unwrap_or_else(|| format!("sess_{}", Uuid::new_v4().simple()));
            let workspace = headers
                .get("x-cw-workspace")
                .filter(|_| authorized)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            let model = raw
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or("unknown-model")
                .to_string();
            let existing_session = state.store.get_session(&session_id).ok().flatten();
            if let Some(ref existing) = existing_session
                && existing.provider != provider
            {
                return (
                    StatusCode::CONFLICT,
                    "Session provider changed; start a new session",
                )
                    .into_response();
            }
            let workspace = workspace.or_else(|| {
                existing_session
                    .as_ref()
                    .and_then(|existing| existing.workspace_root.clone())
            });
            let session = Session {
                id: session_id.clone(),
                parent_session_id: existing_session
                    .as_ref()
                    .and_then(|existing| existing.parent_session_id.clone()),
                harness: existing_session
                    .as_ref()
                    .map(|existing| existing.harness)
                    .unwrap_or(HarnessKind::GenericProxy),
                workspace_root: workspace.clone(),
                model,
                provider,
                title: existing_session
                    .as_ref()
                    .and_then(|existing| existing.title.clone()),
                created_at: existing_session
                    .as_ref()
                    .map(|existing| existing.created_at)
                    .unwrap_or_else(Utc::now),
                updated_at: Utc::now(),
            };
            let cfg = state.store.get_autopilot_config().unwrap_or_default();
            let strict_shadow = cfg.shadow_policy.enabled && cfg.shadow_policy.strict_shadow_only;
            let mutation_requested = !strict_shadow
                && cfg.enabled
                && explicit.is_some()
                && authorized
                && !has_signed_content(&raw);
            if let Err(error) = state.store.upsert_session(&session) {
                if mutation_requested || headers.contains_key("x-cw-dry-run") {
                    return (StatusCode::SERVICE_UNAVAILABLE, error.to_string()).into_response();
                }
                tracing::warn!(%error,"Observation recording unavailable; forwarding original request");
                break 'capture;
            }
            let seq = match state.store.reserve_turn_sequence(&session_id) {
                Ok(seq) => seq,
                Err(error) => {
                    if mutation_requested || headers.contains_key("x-cw-dry-run") {
                        return (StatusCode::SERVICE_UNAVAILABLE, error.to_string())
                            .into_response();
                    }
                    tracing::warn!(%error,"Observation recording unavailable; forwarding original request");
                    break 'capture;
                }
            };
            let previous = state.store.get_latest_turn(&session_id).ok().flatten();
            let previous_nodes = previous.as_ref().map(|t| t.nodes.clone());
            let active = !strict_shadow && mutation_requested;
            let mut outbound = raw.clone();
            let mut applied = Vec::new();
            let mut skipped_mutations = Vec::new();
            let pin_policy = state
                .store
                .get_pin_context_policy(&session_id)
                .unwrap_or_default();
            if active {
                let rules = match state.store.list_mutations(&session_id) {
                    Ok(rules) => rules,
                    Err(error) => {
                        return (StatusCode::SERVICE_UNAVAILABLE, error.to_string())
                            .into_response();
                    }
                };
                let report = apply_mutations_to_payload_with_policy(
                    &mut outbound,
                    provider,
                    &rules,
                    seq,
                    &pin_policy,
                );
                applied = report.applied_rule_ids;
                skipped_mutations = report.skipped_rules;
            }
            // Disk inspection belongs on the blocking pool, never on the forwarding executor.
            let input = outbound.clone();
            let sid = session_id.clone();
            let ws = workspace.clone();
            let prev = previous_nodes.clone();
            let parent = previous.as_ref().map(|t| t.id.clone());
            let analyzed = tokio::task::spawn_blocking(move || {
                analyze_turn_from_payload(
                    format!("turn_{}", Uuid::new_v4().simple()),
                    sid,
                    parent,
                    seq,
                    &input,
                    provider,
                    prev.as_deref(),
                    None,
                    ws.as_deref(),
                )
            })
            .await;
            if let Ok(Ok((mut turn, _))) = analyzed {
                if active || cfg.shadow_policy.enabled {
                    let mut eval_payload = outbound.clone();
                    let report = evaluate_autopilot_policies_with_policy(
                        &cfg,
                        &session_id,
                        &session.model,
                        provider,
                        seq,
                        &turn.nodes,
                        Some(&turn),
                        &mut eval_payload,
                        &pin_policy,
                    );
                    if active && !report.generated_mutations.is_empty() {
                        let creation = match state.store.save_generated_mutations_at_revision(
                            &session_id,
                            None,
                            &report.generated_mutations,
                        ) {
                            Ok(report) => report,
                            Err(error) => {
                                return (StatusCode::SERVICE_UNAVAILABLE, error.to_string())
                                    .into_response();
                            }
                        };
                        let application = apply_mutations_to_payload_with_policy(
                            &mut outbound,
                            provider,
                            &creation.created,
                            seq,
                            &state
                                .store
                                .get_pin_context_policy(&session_id)
                                .unwrap_or_default(),
                        );
                        applied.extend(application.applied_rule_ids);
                        skipped_mutations.extend(application.skipped_rules);
                    }
                    if !report.shadow_decisions.is_empty() {
                        let cache_obs = cw_core::policy::classify_cache_state(
                            &session_id,
                            provider,
                            &session.model,
                            seq,
                            None,
                            None,
                            Some(&turn.kv_cache_report),
                            false,
                        );
                        let pricing =
                            cw_core::policy::find_pricing_profile(provider, &session.model);
                        for dec in &report.shadow_decisions {
                            let mut evidence =
                                cw_core::calibration::ShadowDecisionEvidence::from_policy_decision(
                                    dec,
                                    &cache_obs,
                                    pricing.as_ref(),
                                    cfg.shadow_policy.horizon_turns,
                                    seq,
                                    seq,
                                );
                            let pfp = cw_core::calibration::compute_payload_fingerprint(&outbound);
                            evidence = evidence.with_payload_fingerprint(Some(pfp));
                            if let Err(error) = state.store.save_shadow_decision_evidence(&evidence)
                            {
                                tracing::warn!(%error, "Failed to persist shadow decision evidence");
                            }
                        }
                    }
                }
                let rules = match state.store.list_firewall_rules(Some(&session_id)) {
                    Ok(rules) => rules
                        .into_iter()
                        .filter(|rule| {
                            active
                                || matches!(
                                    rule.action,
                                    cw_core::ir::FirewallAction::PauseForHumanApproval { .. }
                                )
                        })
                        .collect::<Vec<_>>(),
                    Err(error) => {
                        return (StatusCode::SERVICE_UNAVAILABLE, error.to_string())
                            .into_response();
                    }
                };
                let verdict = evaluate_firewall(
                    &mut outbound,
                    provider,
                    &turn.nodes,
                    &turn.kv_cache_report,
                    &rules,
                    seq,
                );
                if let Some(rule) = verdict.paused_by_rule {
                    let intercept_id = format!("int_{}", Uuid::new_v4().simple());
                    let (tx, rx) = oneshot::channel();
                    let info = PendingInterceptInfo {
                        intercept_id: intercept_id.clone(),
                        session_id: session_id.clone(),
                        turn_seq: seq,
                        rule_name: rule.name,
                        payload_preview: outbound.clone(),
                        created_at: Utc::now(),
                    };
                    state.intercepts.lock().await.insert(
                        intercept_id.clone(),
                        ActiveIntercept {
                            info: info.clone(),
                            tx,
                        },
                    );
                    state.broadcast(WsEvent::FirewallIntercepted { intercept: info });
                    let seconds = match rule.action {
                        cw_core::ir::FirewallAction::PauseForHumanApproval { timeout_secs } => {
                            timeout_secs as u64
                        }
                        _ => 0,
                    };
                    let _cleanup = InterceptCleanup {
                        state: state.clone(),
                        id: intercept_id.clone(),
                    };
                    let result = tokio::time::timeout(Duration::from_secs(seconds), rx).await;
                    state.intercepts.lock().await.remove(&intercept_id);
                    state.broadcast(WsEvent::FirewallResolved { intercept_id });
                    match result {
                        Ok(Ok(InterceptResolution {
                            action: ApprovalAction::Approve,
                            edited_payload: None,
                        })) => {}
                        Ok(Ok(InterceptResolution {
                            action: ApprovalAction::Edit,
                            edited_payload: Some(edited),
                        })) if !has_signed_content(&raw)
                            && validate_protocol_invariants(&edited, provider).is_ok()
                            && !has_signed_content(&edited) =>
                        {
                            outbound = edited
                        }
                        _ => {
                            turn.outcome = Some("blocked".into());
                            if let Err(error) = state.store.save_turn(&turn, Some(&raw), None, None)
                            {
                                return (StatusCode::SERVICE_UNAVAILABLE, error.to_string())
                                    .into_response();
                            }
                            return (StatusCode::FORBIDDEN, "Request was not approved")
                                .into_response();
                        }
                    }
                }
                if active {
                    strip_internal_cw_fields(&mut outbound);
                }
                fallback_invalid_mutation(
                    &raw,
                    &mut outbound,
                    provider,
                    &mut applied,
                    &mut skipped_mutations,
                );
                if outbound != raw {
                    outbound_body = match serde_json::to_vec(&outbound) {
                        Ok(b) => b,
                        Err(_) => body.to_vec(),
                    };
                }
                // Reanalyze exactly what will be sent, including a validation fallback.
                if let Ok((actual, _)) = analyze_turn_from_payload(
                    turn.id.clone(),
                    session_id.clone(),
                    turn.parent_turn_id.clone(),
                    seq,
                    &outbound,
                    provider,
                    previous_nodes.as_deref(),
                    None,
                    None,
                ) {
                    turn = actual;
                }
                turn.applied_mutation_ids = applied;
                turn.skipped_mutations = skipped_mutations;
                turn.outcome = Some("pending".into());
                let raw_tokens = cw_core::normalizer::parse_request_to_nodes(&raw, provider, seq)
                    .map(|n| n.iter().map(|n| n.token_span.count).sum::<u32>())
                    .unwrap_or(0);
                turn.token_usage.saved_by_mutations =
                    raw_tokens.saturating_sub(turn.token_usage.input_tokens);
                if let Err(error) = state
                    .store
                    .save_turn(&turn, Some(&raw), Some(&outbound), None)
                {
                    if active || headers.contains_key("x-cw-dry-run") {
                        return (StatusCode::SERVICE_UNAVAILABLE, error.to_string())
                            .into_response();
                    }
                    tracing::warn!(%error,"Observation recording unavailable; forwarding original request");
                    break 'capture;
                }
                if let Err(error) = state.store.save_wire_request(&turn.id, &body) {
                    if active || headers.contains_key("x-cw-dry-run") {
                        return (StatusCode::SERVICE_UNAVAILABLE, error.to_string())
                            .into_response();
                    }
                    tracing::warn!(%error,"Wire recording unavailable; forwarding original request");
                    break 'capture;
                }
                #[cfg(feature = "qualification-fault-hooks")]
                if let Some(hook) = state.qualification_fault_hook.as_ref() {
                    let ingress = body.to_vec();
                    hook.pause(
                        "after_ingress_before_completed_egress",
                        serde_json::json!({
                            "phase":"proxy ingress persisted; model-bound egress not yet dispatched",
                            "session_id":session_id,
                            "turn_id":turn.id,
                            "accepted_admissions":0,
                            "rejected_admissions":0,
                            "upstream_dispatch_count":0,
                            "applied_mutation_ids":turn.applied_mutation_ids,
                            "skipped_mutations":turn.skipped_mutations,
                            "process_cleanup":"supervisor_verified",
                        }),
                        &[("ingress.bin", &ingress)],
                    )
                    .await;
                }
                if headers.contains_key("x-cw-dry-run") {
                    return render_dry_run_response(&state, turn, outbound);
                }
                let session_terminated = headers
                    .get("x-cw-session-end")
                    .or_else(|| headers.get("x-cw-session-terminated"))
                    .and_then(|v| v.to_str().ok())
                    .map(|v| v == "true" || v == "1")
                    .unwrap_or(false);
                if session_terminated {
                    let _ = state
                        .store
                        .mark_session_terminated(&session_id, Some("proxy_header_end"));
                }
                let mut rec =
                    Recorder::new(state.clone(), Some(turn), provider, previous_nodes, started);
                rec.session_terminated = session_terminated;
                recorder = Some(rec);
            }
        }
    }

    if recorder.is_none() {
        match state.store.list_firewall_rules(None) {
            Ok(rules)
                if rules.iter().any(|rule| {
                    rule.enabled
                        && matches!(
                            rule.action,
                            cw_core::ir::FirewallAction::PauseForHumanApproval { .. }
                        )
                }) =>
            {
                return (
                    StatusCode::FORBIDDEN,
                    "Cannot evaluate required approval for this request format",
                )
                    .into_response();
            }
            Err(error) => {
                return (StatusCode::SERVICE_UNAVAILABLE, error.to_string()).into_response();
            }
            _ => {}
        }
    }

    forward_to_upstream(&state, method, target, &headers, outbound_body, recorder).await
}

#[cfg(test)]
mod mutation_fallback_tests {
    use super::{fallback_invalid_mutation, validate_protocol_invariants};
    use cw_core::ir::ProviderKind;
    use serde_json::json;

    #[test]
    fn invalid_mutation_fallback_clears_applied_and_records_skip_reason() {
        let raw = json!({
            "model":"test",
            "messages":[{"role":"user","content":"continue"}]
        });
        let mut outbound = json!({
            "model":"test",
            "messages":[{"role":"user","content":[{"type":"tool_result","tool_use_id":"orphan","content":"bad"}]}]
        });
        assert!(validate_protocol_invariants(&outbound, ProviderKind::Anthropic).is_err());
        assert!(validate_protocol_invariants(&raw, ProviderKind::Anthropic).is_ok());

        let mut applied = vec!["pin-1".to_string()];
        let mut skipped = Vec::new();
        fallback_invalid_mutation(
            &raw,
            &mut outbound,
            ProviderKind::Anthropic,
            &mut applied,
            &mut skipped,
        );

        assert_eq!(outbound, raw);
        assert!(applied.is_empty());
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].rule_id, "pin-1");
        assert_eq!(skipped[0].reason, "PostMutationValidationFailed");
    }
}
