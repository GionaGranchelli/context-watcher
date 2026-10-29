use super::*;
use cw_core::mutation::has_signed_content;

pub(super) async fn handle_proxy_request(
    State(state): State<DaemonState>,
    method: axum::http::Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let started = Instant::now();
    if headers.contains_key("x-cw-dry-run")
        && !serde_json::from_slice::<Value>(&body).is_ok_and(|value| value.is_object())
    {
        return (
            StatusCode::BAD_REQUEST,
            "Preview requires a supported JSON object",
        )
            .into_response();
    }
    let path = uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
    let routes = [
        ("/anthropic", &state.anthropic_upstream),
        ("/openai", &state.openai_upstream),
        ("/deepseek", &state.deepseek_upstream),
        ("/gemini", &state.gemini_upstream),
        ("/ollama", &state.ollama_upstream),
        ("/llamacpp", &state.llamacpp_upstream),
        ("/vllm", &state.vllm_upstream),
    ];
    let fallback = if uri.path().ends_with("/v1/messages") {
        &state.anthropic_upstream
    } else if path.contains("generateContent") {
        &state.gemini_upstream
    } else {
        &state.openai_upstream
    };
    let (base, suffix) = routes
        .iter()
        .find_map(|(prefix, base)| {
            path.strip_prefix(prefix)
                .filter(|rest| rest.is_empty() || rest.starts_with('/') || rest.starts_with('?'))
                .map(|rest| (*base, if rest.is_empty() { "/" } else { rest }))
        })
        .unwrap_or((fallback, path));
    let target = format!("{}{suffix}", base.trim_end_matches('/'));
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
            if explicit.is_some_and(|id| {
                id.is_empty()
                    || id.len() > 128
                    || !id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            }) {
                return (StatusCode::BAD_REQUEST, "Invalid cw session ID").into_response();
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
            if let Ok(Some(existing)) = state.store.get_session(&session_id)
                && (existing.provider != provider
                    || existing.workspace_root != workspace
                    || existing.model != model)
            {
                return (
                    StatusCode::CONFLICT,
                    "Session metadata changed; start a new session",
                )
                    .into_response();
            }
            let session = Session {
                id: session_id.clone(),
                parent_session_id: None,
                harness: HarnessKind::GenericProxy,
                workspace_root: workspace.clone(),
                model,
                provider,
                title: None,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            };
            let cfg = state.store.get_autopilot_config().unwrap_or_default();
            let mutation_requested =
                cfg.enabled && explicit.is_some() && authorized && !has_signed_content(&raw);
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
            let active = mutation_requested
                && cw_core::normalizer::supports_mutation_payload(&raw, provider);
            let mut outbound = raw.clone();
            let mut applied = Vec::new();
            if active {
                let rules = match state.store.list_mutations(&session_id) {
                    Ok(rules) => rules,
                    Err(error) => {
                        return (StatusCode::SERVICE_UNAVAILABLE, error.to_string())
                            .into_response();
                    }
                };
                let report = apply_mutations_to_payload(&mut outbound, provider, &rules, seq);
                applied = report.applied_rule_ids;
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
                if active {
                    let report = evaluate_autopilot_policies(
                        &cfg,
                        &session_id,
                        &session.model,
                        provider,
                        seq,
                        &turn.nodes,
                        Some(&turn),
                        &mut outbound,
                    );
                    for rule in report.generated_mutations {
                        if let Err(error) = state.store.save_mutation(&rule) {
                            return (StatusCode::SERVICE_UNAVAILABLE, error.to_string())
                                .into_response();
                        }
                        applied.push(rule.id);
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
                if validate_protocol_invariants(&outbound, provider).is_err()
                    && validate_protocol_invariants(&raw, provider).is_ok()
                {
                    outbound = raw.clone();
                    applied.clear();
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
                if headers.contains_key("x-cw-dry-run") {
                    turn.outcome = Some("preview".into());
                    let _ = state.store.save_turn(&turn, None, Some(&outbound), None);
                    return Json(json!({"cw_dry_run":true,"turn":turn,"mutated_payload":outbound}))
                        .into_response();
                }
                recorder = Some(Recorder {
                    state: state.clone(),
                    turn: Some(turn),
                    provider,
                    previous: previous_nodes,
                    started,
                    usage: None,
                    sse: false,
                    buffer: Vec::new(),
                });
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
    let mut request = state.http_client.request(method, target);
    for (key, value) in &headers {
        if !matches!(
            key.as_str(),
            "host" | "content-length" | "connection" | "transfer-encoding" | "accept-encoding"
        ) && !key.as_str().starts_with("x-cw-")
        {
            request = request.header(key, value);
        }
    }
    let upstream = match request.body(outbound_body).send().await {
        Ok(response) => response,
        Err(error) => {
            if let Some(mut rec) = recorder {
                rec.finish("upstream_error");
            }
            return (StatusCode::BAD_GATEWAY, error.without_url().to_string()).into_response();
        }
    };
    let status = upstream.status();
    let mut response_headers = upstream.headers().clone();
    response_headers.remove(header::CONNECTION);
    response_headers.remove(header::TRANSFER_ENCODING);
    if let Some(rec) = recorder.as_mut() {
        rec.sse = response_headers
            .get(header::CONTENT_TYPE)
            .is_some_and(|v| v.as_bytes().starts_with(b"text/event-stream"));
    }
    let stream = futures::stream::unfold(
        (upstream.bytes_stream(), recorder),
        move |(mut source, mut recorder)| async move {
            match source.next().await {
                Some(Ok(bytes)) => {
                    if let Some(rec) = recorder.as_mut() {
                        rec.tap(&bytes);
                    }
                    Some((Ok::<_, reqwest::Error>(bytes), (source, recorder)))
                }
                Some(Err(error)) => {
                    if let Some(rec) = recorder.as_mut()
                        && let Some(task) = rec.finish("stream_error")
                    {
                        let _ = task.await;
                    }
                    Some((Err(error), (source, recorder)))
                }
                None => {
                    if let Some(rec) = recorder.as_mut()
                        && let Some(task) = rec.finish(if status.is_success() {
                            "completed"
                        } else {
                            "upstream_error"
                        })
                    {
                        let _ = task.await;
                    }
                    None
                }
            }
        },
    );
    let mut response = Response::new(Body::from_stream(stream));
    *response.status_mut() = status;
    *response.headers_mut() = response_headers;
    response
}

struct Recorder {
    state: DaemonState,
    turn: Option<ContextTurn>,
    provider: ProviderKind,
    previous: Option<Vec<cw_core::ir::ContextNode>>,
    started: Instant,
    usage: Option<Value>,
    sse: bool,
    buffer: Vec<u8>,
}
impl Recorder {
    fn tap(&mut self, bytes: &[u8]) {
        if let Some(turn) = self.turn.as_mut()
            && turn.first_body_byte_ms.is_none()
            && !bytes.is_empty()
        {
            turn.first_body_byte_ms = Some(self.started.elapsed().as_millis() as u64);
        }
        if self.buffer.len().saturating_add(bytes.len()) > 1_048_576 {
            self.buffer.clear();
            return;
        }
        self.buffer.extend_from_slice(bytes);
        if self.sse {
            while let Some(end) = self.buffer.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = self.buffer.drain(..=end).collect();
                if let Some(data) = line.strip_prefix(b"data:")
                    && let Ok(value) = serde_json::from_slice::<Value>(data)
                {
                    self.read_usage(&value);
                }
            }
        }
    }
    fn read_usage(&mut self, value: &Value) {
        let usage = value
            .get("usage")
            .or_else(|| value.get("usageMetadata"))
            .or_else(|| value.pointer("/message/usage"))
            .or_else(|| value.pointer("/response/usage"));
        if let Some(usage) = usage.filter(|u| u.is_object()) {
            let merged = self.usage.get_or_insert_with(|| json!({}));
            if let (Some(target), Some(source)) = (merged.as_object_mut(), usage.as_object()) {
                for (k, v) in source {
                    target.insert(k.clone(), v.clone());
                }
            }
        }
    }
    fn finish(&mut self, outcome: &str) -> Option<tokio::task::JoinHandle<()>> {
        if !self.sse
            && let Ok(value) = serde_json::from_slice::<Value>(&self.buffer)
        {
            self.read_usage(&value);
        }
        let mut turn = self.turn.take()?;
        turn.outcome = Some(outcome.into());
        turn.latency_ms = Some(self.started.elapsed().as_millis() as u64);
        turn.observed_usage = self.usage.take();
        let cached = turn
            .observed_usage
            .as_ref()
            .and_then(|u| extract_cached_tokens_from_json(&json!({"usage":u})));
        turn.kv_cache_report = cw_core::radix::analyze_kv_cache(
            self.previous.as_deref(),
            &mut turn.nodes,
            self.provider,
            cached,
        );
        turn.token_usage.cached_tokens = cached.unwrap_or(0);
        turn.token_usage.output_tokens = turn
            .observed_usage
            .as_ref()
            .and_then(|u| {
                u.get("output_tokens")
                    .or_else(|| u.get("completion_tokens"))
                    .or_else(|| u.get("candidatesTokenCount"))
            })
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .min(u32::MAX as u64) as u32;
        let state = self.state.clone();
        Some(tokio::task::spawn_blocking(move || {
            if let Err(error) = state.store.save_turn(&turn, None, None, None) {
                tracing::error!(%error,"Unable to finalize provider attempt");
                return;
            }
            for id in &turn.applied_mutation_ids {
                let record = MutationEffectivenessRecord {
                    mutation_id: id.clone(),
                    session_id: turn.session_id.clone(),
                    turn_id: turn.id.clone(),
                    tokens_saved: 0,
                    kv_cache_hit_rate: turn.kv_cache_report.hit_ratio as f64,
                    ttft_ms: None,
                    created_at: Utc::now(),
                };
                if let Err(error) = state.store.record_mutation_effectiveness(&record) {
                    tracing::error!(%error,"Unable to record mutation evidence");
                }
            }
            state.broadcast(WsEvent::TurnCompleted {
                session_id: turn.session_id.clone(),
                turn: Box::new(turn),
            });
        }))
    }
}
impl Drop for Recorder {
    fn drop(&mut self) {
        if self.turn.is_some() {
            self.finish("cancelled");
        }
    }
}

struct InterceptCleanup {
    state: DaemonState,
    id: String,
}
impl Drop for InterceptCleanup {
    fn drop(&mut self) {
        let state = self.state.clone();
        let id = self.id.clone();
        tokio::spawn(async move {
            if state.intercepts.lock().await.remove(&id).is_some() {
                state.broadcast(WsEvent::FirewallResolved { intercept_id: id });
            }
        });
    }
}
