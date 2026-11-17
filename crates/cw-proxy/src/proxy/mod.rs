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
    mutation::{
        MutationSchedulingConfig, apply_mutations_with_scheduler, detect_stable_prefix,
        evaluate_firewall, has_signed_content,
    },
    normalizer::detect_provider,
    validator::validate_protocol_invariants,
};
use serde_json::{Value, json};
use std::sync::Arc;
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

const MAX_KV_BREAK_TOKENS: u32 = 512;

/// Token budget for the rendered Zone 1 symbol map prefix (opt-in).
const ZONE1_MAX_TOKENS: usize = 512;

/// Display-only context budget for the Zone 3 budget HUD (opt-in symbol-map mode). The proxy
/// does not enforce a provider-specific window here; this constant is the reference denominator
/// shown in `[cw: active/budget tok | ...]` telemetry.
const HUD_CONTEXT_BUDGET_TOKENS: usize = 128_000;

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

fn require_standard_service_tier(payload: &mut Value) -> Result<bool, ()> {
    if !payload.is_object() {
        return Err(());
    }
    match payload.get("service_tier") {
        None => {
            payload["service_tier"] = Value::String("default".into());
            Ok(true)
        }
        Some(tier) if tier.as_str() == Some("default") => Ok(false),
        Some(_) => Err(()),
    }
}

/// Renders the Zone 1 symbol map prefix for an enabled opt-in run. The workspace root is taken
/// from `CW_WORKSPACE_ROOT` (falling back to the daemon's working directory, mirroring the CLI's
/// transcript-watcher default) and indexed once at proxy startup.
fn zone1_block_from_env() -> Option<String> {
    let root = match std::env::var_os("CW_WORKSPACE_ROOT") {
        Some(root) if !root.is_empty() => std::path::PathBuf::from(root),
        _ => std::env::current_dir().ok()?,
    };
    cw_core::symbol_index::SymbolIndex::build(&root)
        .ok()
        .map(|index| index.render_zone1_prefix(ZONE1_MAX_TOKENS))
}

/// Builds the Intercepting Proxy Router (`127.0.0.1:4401`).
pub fn build_proxy_router(state: DaemonState) -> Router {
    let mut state = state;
    // Zone 1 symbol map is opt-in and parsed exactly once at proxy startup: only the exact
    // values `1`/`true` enable it (case-sensitive). When enabled, the index is built over
    // `CW_WORKSPACE_ROOT` (or the daemon's working directory) and rendered into a stable
    // prefix block cached in the state for the lifetime of this process. Workspace changes
    // after startup are NOT picked up — restart the daemon to re-index. Build failure keeps
    // injection disabled with a warning rather than failing proxy startup.
    if std::env::var_os("CW_ENABLE_SYMBOL_MAP").is_some_and(|value| value == "1" || value == "true")
    {
        match zone1_block_from_env() {
            Some(block) => state.zone1_block = Some(Arc::from(block)),
            None => tracing::warn!(
                "CW_ENABLE_SYMBOL_MAP is set but the workspace symbol index could not be built; Zone 1 injection disabled"
            ),
        }
    }
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
    let detected_provider = serde_json::from_slice::<Value>(&body)
        .ok()
        .map(|raw| detect_provider(uri.path(), &raw))
        .unwrap_or_else(|| detect_provider(uri.path(), &Value::Null));
    let upstream_provider = routing::provider_for_route(uri.path(), detected_provider);
    if state.live_allowed_session_id.is_some() {
        let expected_origin = format!("{}/", state.openai_upstream.trim_end_matches('/'));
        let authorized = headers
            .get("x-cw-control-token")
            .and_then(|value| value.to_str().ok())
            == Some(state.control_token.as_str());
        let bound_session = headers
            .get("x-cw-session-id")
            .and_then(|value| value.to_str().ok())
            == state.live_allowed_session_id.as_deref();
        let live_route = uri.path().starts_with("/openai/")
            && target.starts_with(&expected_origin)
            && upstream_provider == ProviderKind::OpenAI;
        let authorization_active = state
            .live_authorization_expires_at
            .is_some_and(|expires| Utc::now() < expires);
        let mut authorized_payload = serde_json::from_slice::<Value>(&body).ok();
        let tier_was_defaulted = authorized_payload
            .as_mut()
            .and_then(|payload| require_standard_service_tier(payload).ok());
        let authorized_tier = tier_was_defaulted.is_some();
        if !live_route || !authorized || !bound_session || !authorization_active || !authorized_tier
        {
            tracing::warn!(
                live_route,
                authorized_control_token = authorized,
                bound_session,
                authorization_active,
                authorized_service_tier = authorized_tier,
                "L0 smoke request rejected before SpendGuard admission"
            );
            return (
                StatusCode::FORBIDDEN,
                "Smoke authorization route, session, expiry, or pricing tier mismatch",
            )
                .into_response();
        }
    }
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
            // Zones 1+2 (opt-in symbol-map mode): inject the deterministic symbol map prefix and
            // session registers into the system prompt before mutation scheduling runs, so
            // stable-prefix detection hashes them naturally. The working set is loaded per
            // request; nothing here writes it back (Milestone C's cw_mount owns register writes).
            let zone1_active = active && state.zone1_block.is_some();
            let working_set = if zone1_active {
                state.store.get_working_set(&session_id).unwrap_or_default()
            } else {
                cw_core::working_set::WorkingSetState::default()
            };
            if zone1_active && let Some(block) = state.zone1_block.as_deref() {
                // Zone 1: byte-stable prefix; Zone 2 lands immediately after it, and duplicate
                // raw file reads in the Zone 3 history are evicted before any mutation runs.
                cw_core::symbol_index::inject_zone1_prefix(&mut outbound, provider, block);
                if let Some(zone2) = working_set.render_zone2_block() {
                    cw_core::working_set::inject_zone2_registers(&mut outbound, provider, &zone2);
                    cw_core::working_set::stub_mounted_reads_in_zone3(
                        &mut outbound,
                        provider,
                        &working_set,
                    );
                }
            }
            let mut applied = Vec::new();
            let mut skipped_mutations = Vec::new();
            let mut mutation_telemetry: Option<cw_core::ir::MutationTurnTelemetry> = None;
            let mut recent_turns = if active {
                match state.store.list_turns_page(&session_id, 12, 0) {
                    Ok(turns) => turns,
                    Err(error) => {
                        return (StatusCode::SERVICE_UNAVAILABLE, error.to_string())
                            .into_response();
                    }
                }
            } else {
                Vec::new()
            };
            // Keep order deterministic for stable-prefix detection, regardless of store paging order.
            recent_turns.sort_by_key(|turn| turn.sequence_number);
            let cache_confidence_known = recent_turns
                .last()
                .map(|turn| turn.kv_cache_report.telemetry_available)
                .unwrap_or(false);
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
                let scheduling = MutationSchedulingConfig {
                    max_kv_break_tokens: MAX_KV_BREAK_TOKENS,
                    ..MutationSchedulingConfig::default()
                };
                let stable_prefix = detect_stable_prefix(&recent_turns, &scheduling);
                let report = apply_mutations_with_scheduler(
                    &mut outbound,
                    provider,
                    &rules,
                    seq,
                    &pin_policy,
                    &scheduling,
                    cache_confidence_known,
                    stable_prefix,
                );
                applied = report.application.applied_rule_ids;
                skipped_mutations = report.application.skipped_rules;
                mutation_telemetry = Some(report.telemetry);
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
                        let prior_used = mutation_telemetry
                            .as_ref()
                            .map(|t| t.kv_budget_used_tokens)
                            .unwrap_or(0);
                        let scheduling = MutationSchedulingConfig {
                            max_kv_break_tokens: MAX_KV_BREAK_TOKENS.saturating_sub(prior_used),
                            ..MutationSchedulingConfig::default()
                        };
                        let stable_prefix = detect_stable_prefix(&recent_turns, &scheduling);
                        let application = apply_mutations_with_scheduler(
                            &mut outbound,
                            provider,
                            &creation.created,
                            seq,
                            &pin_policy,
                            &scheduling,
                            cache_confidence_known,
                            stable_prefix,
                        );
                        applied.extend(application.application.applied_rule_ids);
                        skipped_mutations.extend(application.application.skipped_rules);
                        if let Some(telemetry) = &mut mutation_telemetry {
                            telemetry.kv_budget_used_tokens = telemetry
                                .kv_budget_used_tokens
                                .saturating_add(application.telemetry.kv_budget_used_tokens);
                            telemetry.predicted_saved_tokens +=
                                application.telemetry.predicted_saved_tokens;
                            telemetry.predicted_kv_break_tokens = telemetry
                                .predicted_kv_break_tokens
                                .saturating_add(application.telemetry.predicted_kv_break_tokens);
                            telemetry.observed_saved_tokens +=
                                application.telemetry.observed_saved_tokens;
                            telemetry
                                .selected_rule_ids
                                .extend(application.telemetry.selected_rule_ids);
                            telemetry
                                .queued_rule_ids
                                .extend(application.telemetry.queued_rule_ids);
                            telemetry
                                .skipped_rule_ids
                                .extend(application.telemetry.skipped_rule_ids);
                            telemetry.decisions.extend(application.telemetry.decisions);
                            telemetry.stable_prefix = application.telemetry.stable_prefix;
                        } else {
                            mutation_telemetry = Some(application.telemetry);
                        }
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
                // Zone 3 telemetry (opt-in symbol-map mode): the one-line budget HUD lands at
                // the tail of the active turn after every mutation pass. It is only appended
                // when the zone-modified payload survived validation: a fallback revert above
                // restored `raw` exactly, in which case no zones — and no HUD — go out.
                if zone1_active && outbound != raw {
                    // Token estimate reuses one serialization pass: no intermediate String, the
                    // UTF-8 slice is borrowed straight from the vec. The egress body below must
                    // re-serialize after append_budget_hud mutates a message in place — a single
                    // pass cannot cover both, because the HUD line itself references the count.
                    let active_tokens = serde_json::to_vec(&outbound)
                        .ok()
                        .and_then(|bytes| {
                            std::str::from_utf8(&bytes)
                                .ok()
                                .map(cw_core::tokenizer::estimate_tokens)
                        })
                        .unwrap_or(0) as usize;
                    let zone1_bytes = state.zone1_block.as_deref().map(str::len).unwrap_or(0);
                    cw_core::working_set::append_budget_hud(
                        &mut outbound,
                        provider,
                        active_tokens,
                        HUD_CONTEXT_BUDGET_TOKENS,
                        &working_set,
                        zone1_bytes,
                    );
                }
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
                if let Some(telemetry) = &mutation_telemetry {
                    tracing::info!(
                        session_id = %session_id,
                        turn_seq = seq,
                        cache_confidence = %telemetry.cache_confidence,
                        kv_budget_tokens = telemetry.kv_budget_tokens,
                        kv_budget_used_tokens = telemetry.kv_budget_used_tokens,
                        selected = telemetry.selected_rule_ids.len(),
                        queued = telemetry.queued_rule_ids.len(),
                        skipped = telemetry.skipped_rule_ids.len(),
                        predicted_saved_tokens = telemetry.predicted_saved_tokens,
                        observed_saved_tokens = telemetry.observed_saved_tokens,
                        predicted_kv_break_tokens = telemetry.predicted_kv_break_tokens,
                        stable_prefix_confidence = telemetry.stable_prefix.confidence,
                        stable_prefix_repeat_count = telemetry.stable_prefix.repeat_count,
                        "mutation scheduling telemetry"
                    );
                }
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

    if state.live_allowed_session_id.is_some() {
        let mut payload = match serde_json::from_slice::<Value>(&outbound_body) {
            Ok(payload) => payload,
            Err(error) => return (StatusCode::BAD_REQUEST, error.to_string()).into_response(),
        };
        if require_standard_service_tier(&mut payload).is_err() {
            return (
                StatusCode::FORBIDDEN,
                "Smoke request no longer qualifies for standard pricing",
            )
                .into_response();
        }
        outbound_body = match serde_json::to_vec(&payload) {
            Ok(normalized) => normalized,
            Err(error) => return (StatusCode::BAD_REQUEST, error.to_string()).into_response(),
        };
    }

    forward_to_upstream(
        &state,
        upstream_provider,
        method,
        target,
        &headers,
        outbound_body,
        recorder,
    )
    .await
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

#[cfg(test)]
mod l0_route_boundary_tests {
    use super::{handle_proxy_request, require_standard_service_tier};
    use crate::state::DaemonState;
    use axum::{
        Json, Router,
        body::Bytes,
        extract::State,
        http::{HeaderMap, Method, StatusCode, Uri},
        routing::post,
    };
    use chrono::{Duration, Utc};
    use cw_store::CwStore;

    #[test]
    fn l0_defaults_an_omitted_tier_and_rejects_other_explicit_tiers() {
        let mut omitted = serde_json::json!({"model":"gpt-6-luna"});
        assert_eq!(require_standard_service_tier(&mut omitted), Ok(true));
        assert_eq!(omitted["service_tier"], "default");

        let mut explicit = serde_json::json!({"model":"gpt-6-luna","service_tier":"default"});
        assert_eq!(require_standard_service_tier(&mut explicit), Ok(false));
        assert_eq!(explicit["service_tier"], "default");

        for tier in ["auto", "priority", "flex"] {
            let mut payload = serde_json::json!({"service_tier":tier});
            assert_eq!(require_standard_service_tier(&mut payload), Err(()));
        }
        let mut null_tier = serde_json::json!({"service_tier":null});
        assert_eq!(require_standard_service_tier(&mut null_tier), Err(()));
    }

    #[tokio::test]
    async fn l0_forwards_an_explicit_standard_tier_when_codex_omits_it() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let app = Router::new().route(
            "/v1/responses",
            post(move |body: Bytes| {
                let tx = tx.clone();
                async move {
                    tx.send(body).unwrap();
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({"status":"completed"})),
                    )
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let mut state = DaemonState::new(CwStore::open_in_memory().unwrap());
        state.openai_upstream = upstream;
        state.live_allowed_session_id = Some("cw122b2l0-test-auth".into());
        state.live_authorization_expires_at = Some(Utc::now() + Duration::minutes(1));
        state.capture_wire_egress = true;
        let mut headers = HeaderMap::new();
        headers.insert("x-cw-control-token", state.control_token.parse().unwrap());
        headers.insert("x-cw-session-id", "cw122b2l0-test-auth".parse().unwrap());
        let response = handle_proxy_request(
            State(state),
            Method::POST,
            Uri::try_from("/openai/v1/responses").unwrap(),
            headers,
            Bytes::from_static(br#"{"model":"gpt-6-luna","input":"smoke"}"#),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let _ = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        let egress: serde_json::Value = serde_json::from_slice(&rx.recv().await.unwrap()).unwrap();
        assert_eq!(egress["service_tier"], "default");
        server.abort();
    }

    #[tokio::test]
    async fn l0_rejects_alternate_routes_and_unbound_openai_requests() {
        let mut state = DaemonState::new(CwStore::open_in_memory().unwrap());
        state.live_allowed_session_id = Some("cw122b2l0-test-auth".into());
        state.live_authorization_expires_at = Some(Utc::now() + Duration::minutes(1));
        state.live_provider_model = Some((cw_core::ir::ProviderKind::OpenAI, "gpt-6-luna".into()));
        state.spend_guard = Some(
            cw_core::spend_guard::SpendGuard::new_with_campaign_request_limit(
                cw_core::spend_guard::SpendGuardConfig::default(),
                1,
            ),
        );

        for (path, bound, expires_at, tier) in [
            (
                "/deepseek/v1/chat/completions",
                false,
                Utc::now() + Duration::minutes(1),
                None,
            ),
            (
                "/openai/v1/responses",
                false,
                Utc::now() + Duration::minutes(1),
                None,
            ),
            (
                "/openai/v1/responses",
                true,
                Utc::now() - Duration::seconds(1),
                Some("default"),
            ),
            (
                "/openai/v1/responses",
                true,
                Utc::now() + Duration::minutes(1),
                None,
            ),
            (
                "/openai/v1/responses",
                true,
                Utc::now() + Duration::minutes(1),
                Some("priority"),
            ),
        ] {
            let mut request_state = state.clone();
            request_state.live_authorization_expires_at = Some(expires_at);
            let mut headers = HeaderMap::new();
            if bound {
                headers.insert(
                    "x-cw-control-token",
                    request_state.control_token.parse().unwrap(),
                );
                headers.insert("x-cw-session-id", "cw122b2l0-test-auth".parse().unwrap());
            }
            let mut payload = serde_json::json!({"model":"gpt-6-luna"});
            if let Some(tier) = tier {
                payload["service_tier"] = serde_json::Value::String(tier.into());
            }
            let response = handle_proxy_request(
                State(request_state),
                Method::POST,
                Uri::try_from(path).unwrap(),
                headers,
                Bytes::from(serde_json::to_vec(&payload).unwrap()),
            )
            .await;
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "route {path}");
        }
    }
}

#[cfg(test)]
mod zone1_injection_tests {
    use super::{HUD_CONTEXT_BUDGET_TOKENS, ZONE1_MAX_TOKENS, handle_proxy_request};
    use crate::state::DaemonState;
    use axum::{
        Json, body::Bytes, extract::State, http::HeaderMap, response::Response, routing::post,
    };
    use std::sync::Arc;

    const SESSION_ID: &str = "cw-zone1-test";

    /// Builds a deterministic Zone 1 block without touching the real workspace.
    fn test_zone1_block() -> String {
        let index = cw_core::symbol_index::SymbolIndex::build_from_sources(&[
            ("src/app.rs", "pub fn run() {}\n"),
            (
                "lib/main.ts",
                "export function start(): void {\n  console.log(\"go\");\n}\n",
            ),
        ]);
        index.render_zone1_prefix(ZONE1_MAX_TOKENS)
    }

    #[tokio::test]
    async fn zone1_prefix_is_byte_identical_across_turns_and_stable_on_second() {
        // Mock OpenAI upstream that captures every egress body.
        let (egress_tx, mut egress_rx) = tokio::sync::mpsc::unbounded_channel::<Bytes>();
        let app = axum::Router::new().route(
            "/v1/chat/completions",
            post(move |body: Bytes| {
                let tx = egress_tx.clone();
                async move {
                    let _ = tx.send(body);
                    (
                        axum::http::StatusCode::OK,
                        Json(serde_json::json!({
                            "id": "chatcmpl-zone1",
                            "object": "chat.completion",
                            "model": "gpt-test",
                            "choices": [{
                                "index": 0,
                                "message": {"role": "assistant", "content": "ok"},
                                "finish_reason": "stop"
                            }],
                            "usage": {"prompt_tokens": 12, "completion_tokens": 3, "total_tokens": 15}
                        })),
                    )
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_url = format!("http://{}", listener.local_addr().unwrap());
        let _server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        // Shared in-memory store (cloned DaemonStates share the connection), with
        // autopilot enabled so mutation_requested -> active on both turns.
        let base_state = DaemonState::new(cw_store::CwStore::open_in_memory().unwrap());
        let autopilot = cw_core::autopilot::AutoPilotConfig {
            enabled: true,
            ..Default::default()
        };
        base_state.store.set_autopilot_config(&autopilot).unwrap();

        // Harnesses re-send the full unmutated conversation on every turn, so both turns
        // carry byte-identical payloads (a continuation/retry of the same request). The
        // proxy must therefore inject an identical Zone 1 block each time.
        let payload = serde_json::json!({
            "model": "gpt-test",
            "messages": [
                {"role": "system", "content": "base system prompt"},
                {"role": "user", "content": "hello"}
            ]
        });

        async fn post_turn(state: DaemonState, payload: &serde_json::Value) -> Response {
            let mut headers = HeaderMap::new();
            headers.insert(
                "x-cw-control-token",
                state.control_token.as_str().parse().unwrap(),
            );
            headers.insert("x-cw-session-id", SESSION_ID.parse().unwrap());
            handle_proxy_request(
                State(state),
                axum::http::Method::POST,
                axum::http::Uri::try_from("/openai/v1/chat/completions").unwrap(),
                headers,
                Bytes::from(serde_json::to_vec(payload).unwrap()),
            )
            .await
        }

        let mut turn1_state = base_state.clone();
        turn1_state.openai_upstream = upstream_url.clone();
        turn1_state.zone1_block = Some(Arc::from(test_zone1_block()));
        let response_1 = post_turn(turn1_state, &payload).await;
        assert_eq!(response_1.status(), axum::http::StatusCode::OK);

        let mut turn2_state = base_state.clone();
        turn2_state.openai_upstream = upstream_url.clone();
        turn2_state.zone1_block = Some(Arc::from(test_zone1_block()));
        let response_2 = post_turn(turn2_state, &payload).await;
        assert_eq!(response_2.status(), axum::http::StatusCode::OK);

        // Both egress bodies carry the Zone 1 block, byte-identical across turns.
        let body_1: serde_json::Value =
            serde_json::from_slice(&egress_rx.recv().await.unwrap()).unwrap();
        let body_2: serde_json::Value =
            serde_json::from_slice(&egress_rx.recv().await.unwrap()).unwrap();
        let system_1 = body_1["messages"][0]["content"]
            .as_str()
            .expect("injected system message")
            .to_owned();
        let system_2 = body_2["messages"][0]["content"]
            .as_str()
            .expect("injected system message")
            .to_owned();
        assert_eq!(
            system_1, system_2,
            "Zone 1 bytes must be identical across turns"
        );
        assert!(
            system_1.starts_with("base system prompt"),
            "injection appends to the base prompt: {system_1}"
        );
        assert!(system_1.contains("<cw_memory_protocol>"));
        assert!(system_1.contains("<workspace_symbol_map>"));
        assert!(system_1.contains("</workspace_symbol_map>"));
        assert!(system_1.contains("repo://src/app.rs"));

        // The persisted turns expose the same prefix: on turn 2 the detector reports stable.
        let turns = base_state.store.list_turns_page(SESSION_ID, 10, 0).unwrap();
        assert_eq!(turns.len(), 2);
        let telemetry = cw_core::mutation::detect_stable_prefix(
            &turns,
            &cw_core::mutation::MutationSchedulingConfig::default(),
        );
        assert_eq!(
            telemetry.repeat_count, 2,
            "both turns must share the prefix fingerprint"
        );
        assert!(
            telemetry.stable,
            "expected stable prefix on turn 2: {telemetry:?}"
        );
    }

    #[tokio::test]
    async fn zone1_injection_is_off_by_default_and_leaves_payloads_untouched() {
        let (egress_tx, mut egress_rx) = tokio::sync::mpsc::unbounded_channel::<Bytes>();
        let app = axum::Router::new().route(
            "/v1/chat/completions",
            post(move |body: Bytes| {
                let tx = egress_tx.clone();
                async move {
                    let _ = tx.send(body);
                    (
                        axum::http::StatusCode::OK,
                        Json(serde_json::json!({"status": "ok"})),
                    )
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_url = format!("http://{}", listener.local_addr().unwrap());
        let _server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        // No zone1_block set (CW_ENABLE_SYMBOL_MAP unset) => byte-exact passthrough.
        let state = DaemonState::new(cw_store::CwStore::open_in_memory().unwrap());
        let mut request_state = state.clone();
        request_state.openai_upstream = upstream_url;

        let payload = serde_json::json!({
            "model": "gpt-test",
            "messages": [
                {"role": "system", "content": "base system prompt"},
                {"role": "user", "content": "hello"}
            ]
        });
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-cw-control-token",
            request_state.control_token.as_str().parse().unwrap(),
        );
        headers.insert("x-cw-session-id", SESSION_ID.parse().unwrap());
        // Keep the exact serialized request bytes: passthrough must be verified at byte
        // level, not via re-parsed JSON values (which would also pass a re-serialization).
        let original = serde_json::to_vec(&payload).unwrap();
        let response = handle_proxy_request(
            State(request_state),
            axum::http::Method::POST,
            axum::http::Uri::try_from("/openai/v1/chat/completions").unwrap(),
            headers,
            Bytes::from(original.clone()),
        )
        .await;
        assert_eq!(response.status(), axum::http::StatusCode::OK);

        let egress = egress_rx.recv().await.unwrap();
        assert_eq!(
            egress.as_ref(),
            original.as_slice(),
            "default-off proxy must forward the original bytes"
        );
    }

    const THREE_ZONE_DUMP: &str = "pub fn foo() {\n    let answer = 42;\n    answer\n}\n";

    #[tokio::test]
    async fn three_zone_pipeline_renders_zones_in_order_with_stable_prefix() {
        // Mock OpenAI upstream that captures every egress body.
        let (egress_tx, mut egress_rx) = tokio::sync::mpsc::unbounded_channel::<Bytes>();
        let app = axum::Router::new().route(
            "/v1/chat/completions",
            post(move |body: Bytes| {
                let tx = egress_tx.clone();
                async move {
                    let _ = tx.send(body);
                    (
                        axum::http::StatusCode::OK,
                        Json(serde_json::json!({
                            "id": "chatcmpl-3zone",
                            "object": "chat.completion",
                            "model": "gpt-test",
                            "choices": [{
                                "index": 0,
                                "message": {"role": "assistant", "content": "ok"},
                                "finish_reason": "stop"
                            }],
                            "usage": {"prompt_tokens": 12, "completion_tokens": 3, "total_tokens": 15}
                        })),
                    )
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_url = format!("http://{}", listener.local_addr().unwrap());
        let _server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        // Shared in-memory store with autopilot enabled (so active == true), and a
        // pre-populated session working set: one mounted symbol + one pinned invariant.
        let base_state = DaemonState::new(cw_store::CwStore::open_in_memory().unwrap());
        let autopilot = cw_core::autopilot::AutoPilotConfig {
            enabled: true,
            ..Default::default()
        };
        base_state.store.set_autopilot_config(&autopilot).unwrap();

        use cw_core::{
            symbol_index::SymbolUri,
            working_set::{MountMode, MountedSlice},
        };
        let mut ws = cw_core::working_set::WorkingSetState::default();
        ws.mounted.insert(
            SymbolUri("repo://src/lib.rs#foo".into()),
            MountedSlice {
                uri: SymbolUri("repo://src/lib.rs#foo".into()),
                rel_path: "src/lib.rs".to_string(),
                mode: MountMode::Full,
                start_line: 1,
                end_line: 4,
                content: THREE_ZONE_DUMP.to_string(),
                token_estimate: cw_core::tokenizer::estimate_tokens(THREE_ZONE_DUMP) as usize,
            },
        );
        ws.pin("kv-rule", "keep Zone 1 static");
        base_state.store.save_working_set(SESSION_ID, &ws).unwrap();

        // Turn 1 payload carries a duplicate raw file read in the tool history.
        let turn1_payload = serde_json::json!({
            "model": "gpt-test",
            "messages": [
                {"role": "system", "content": "base system prompt"},
                {"role": "user", "content": "inspect foo"},
                {"role": "assistant", "content": null, "tool_calls": [{"id": "c1"}]},
                {"role": "tool", "tool_call_id": "c1", "content": format!("src/lib.rs:\n{THREE_ZONE_DUMP}")}
            ]
        });
        // Turn 2 resends the full turn-1 history plus the assistant reply and a new user
        // message — proving the HUD lands on the fresh final message, not on accumulated state.
        let turn2_payload = serde_json::json!({
            "model": "gpt-test",
            "messages": [
                {"role": "system", "content": "base system prompt"},
                {"role": "user", "content": "inspect foo"},
                {"role": "assistant", "content": null, "tool_calls": [{"id": "c1"}]},
                {"role": "tool", "tool_call_id": "c1", "content": format!("src/lib.rs:\n{THREE_ZONE_DUMP}")},
                {"role": "assistant", "content": "the answer is 42"},
                {"role": "user", "content": "verify it"}
            ]
        });

        async fn post_turn(state: DaemonState, payload: &serde_json::Value) -> Response {
            let mut headers = HeaderMap::new();
            headers.insert(
                "x-cw-control-token",
                state.control_token.as_str().parse().unwrap(),
            );
            headers.insert("x-cw-session-id", SESSION_ID.parse().unwrap());
            handle_proxy_request(
                State(state),
                axum::http::Method::POST,
                axum::http::Uri::try_from("/openai/v1/chat/completions").unwrap(),
                headers,
                Bytes::from(serde_json::to_vec(payload).unwrap()),
            )
            .await
        }

        let mut turn1_state = base_state.clone();
        turn1_state.openai_upstream = upstream_url.clone();
        turn1_state.zone1_block = Some(Arc::from(test_zone1_block()));
        assert_eq!(
            post_turn(turn1_state, &turn1_payload).await.status(),
            axum::http::StatusCode::OK
        );

        let mut turn2_state = base_state.clone();
        turn2_state.openai_upstream = upstream_url.clone();
        turn2_state.zone1_block = Some(Arc::from(test_zone1_block()));
        assert_eq!(
            post_turn(turn2_state, &turn2_payload).await.status(),
            axum::http::StatusCode::OK
        );

        let body_1: serde_json::Value =
            serde_json::from_slice(&egress_rx.recv().await.unwrap()).unwrap();
        let body_2: serde_json::Value =
            serde_json::from_slice(&egress_rx.recv().await.unwrap()).unwrap();

        // Zone 3 stub prefix, expected HUD line shape (F1: assert the full line, not substrings),
        // and per-turn index of the final user/tool message each HUD must land on.
        let stub_prefix = "[Mounted in Zone 2: repo://src/lib.rs#foo — raw duplicate evicted]";
        let hud_shape = format!(
            "/{HUD_CONTEXT_BUDGET_TOKENS} tok | mounted: 1 | pinned: 1 | zone1_prefix_bytes: {}",
            test_zone1_block().len()
        );

        for (turn, body, hud_index) in [(1usize, &body_1, 3usize), (2, &body_2, 5)] {
            // Zone 1 -> Zone 2 inside the system prompt, in order.
            let system = body["messages"][0]["content"]
                .as_str()
                .unwrap_or("")
                .to_owned();
            assert!(system.starts_with("base system prompt"), "turn {turn}");
            let z1_close = "</workspace_symbol_map>";
            let z1_end = system.find(z1_close).expect("zone 1 frame missing") + z1_close.len();
            let mounted_pos = system
                .find("<mounted_context>\n")
                .expect("zone 2 <mounted_context> missing");
            let pinned_pos = system
                .find("<pinned_invariants>")
                .expect("invariants frame missing");
            assert!(
                mounted_pos > z1_end,
                "Zone 2 must follow Zone 1 (turn {turn})"
            );
            assert!(
                pinned_pos > mounted_pos,
                "invariants frame follows mounts (turn {turn})"
            );
            assert!(system.contains("### [repo://src/lib.rs#foo] (mode: full, lines: 1-4)"));
            assert!(system.contains("- [kv-rule]: keep Zone 1 static"));

            // Zone 3: the duplicate raw file read is evicted to a one-line stub.
            let tool_content = body["messages"][3]["content"].as_str().unwrap();
            assert!(
                tool_content.starts_with(stub_prefix),
                "raw dump must be stubbed (turn {turn}): {tool_content}"
            );

            // HUD at the tail of the final user/tool message, full line shape.
            let hud_content = body["messages"][hud_index]["content"].as_str().unwrap();
            let hud_lines: Vec<&str> = hud_content
                .lines()
                .filter(|l| l.starts_with("[cw:"))
                .collect();
            assert_eq!(hud_lines.len(), 1, "exactly one HUD line (turn {turn})");
            assert!(
                hud_lines[0].starts_with("[cw: "),
                "HUD must open with '[cw: ' (turn {turn}): {}",
                hud_lines[0]
            );
            assert!(
                hud_lines[0].contains(&hud_shape),
                "full HUD line shape (turn {turn}): {}",
                hud_lines[0]
            );
            assert!(
                hud_lines[0].ends_with("]"),
                "HUD must close with ']' (turn {turn})"
            );
        }

        // Zone 1 (and the whole deterministic system prefix) is byte-identical across turns,
        // even though turn 2 carries extra history.
        let sys_1 = body_1["messages"][0]["content"].as_str().unwrap();
        let sys_2 = body_2["messages"][0]["content"].as_str().unwrap();
        assert_eq!(
            sys_1, sys_2,
            "system prompt must be byte-stable across turns"
        );
    }
}
