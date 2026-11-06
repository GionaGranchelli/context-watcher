use axum::{
    body::Body,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use cw_core::ir::ProviderKind;
use futures::StreamExt;

use crate::{proxy::recorder::Recorder, state::DaemonState};

/// Forwards an outbound HTTP request to the upstream LLM provider,
/// filtering internal and hop-by-hop headers, and streams the response
/// while tapping telemetry and usage metrics through the `Recorder`.
pub async fn forward_to_upstream(
    state: &DaemonState,
    upstream_provider: ProviderKind,
    method: axum::http::Method,
    target: String,
    headers: &HeaderMap,
    outbound_body: Vec<u8>,
    mut recorder: Option<Recorder>,
) -> Response {
    if let Some((expected_provider, expected_model)) = state.live_provider_model.as_ref() {
        let observed_model = recorder
            .as_ref()
            .and_then(|rec| rec.turn.as_ref())
            .and_then(|turn| turn.model.as_deref());
        if *expected_provider != upstream_provider || observed_model != Some(expected_model) {
            if let Some(mut rec) = recorder
                && let Some(task) = rec.finish("blocked_model_mismatch")
            {
                let _ = task.await;
            }
            return (
                StatusCode::FORBIDDEN,
                "Smoke authorization provider/model mismatch",
            )
                .into_response();
        }
    }
    if upstream_provider == ProviderKind::OpenAI
        && state.openai_api_key_configured
        && state.openai_api_key.is_none()
    {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "OpenAI upstream credential is configured but unavailable or invalid",
        )
            .into_response();
    }
    // GET (e.g. model listing) is not a billable generation request.
    if let Some(guard) = state.spend_guard.as_ref()
        && method != axum::http::Method::GET
    {
        // Live spend control: admit before dispatch, never forward unmetered requests.
        let Some(turn) = recorder.as_ref().and_then(|r| r.turn.as_ref()) else {
            return (
                StatusCode::FORBIDDEN,
                "SpendGuard: unmetered provider request rejected",
            )
                .into_response();
        };
        let session_id = turn.session_id.clone();
        let turn_id = turn.id.clone();
        let model = turn.model.clone().unwrap_or_default();
        match guard.admit_request(&session_id) {
            Ok(permit) => {
                if state
                    .store
                    .record_spend_admission(cw_store::SpendGuardAdmission {
                        turn_id: &turn_id,
                        session_id: &session_id,
                        provider: &format!("{upstream_provider:?}"),
                        model: &model,
                        admitted: true,
                        reservation_usd: Some(guard.config().request_reservation_usd),
                        error: None,
                    })
                    .is_err()
                {
                    drop(permit);
                    if let Some(mut rec) = recorder
                        && let Some(task) = rec.finish("spend_evidence_error")
                    {
                        let _ = task.await;
                    }
                    return (
                        StatusCode::INSUFFICIENT_STORAGE,
                        "Unable to persist SpendGuard admission evidence",
                    )
                        .into_response();
                }
                if let Some(rec) = recorder.as_mut() {
                    rec.spend_permit = Some(permit);
                }
                #[cfg(feature = "qualification-fault-hooks")]
                if let Some(hook) = state.qualification_fault_hook.as_ref()
                    && let Some(turn) = recorder.as_ref().and_then(|rec| rec.turn.as_ref())
                {
                    let ingress = state
                        .store
                        .get_wire_request(&turn.id)
                        .ok()
                        .flatten()
                        .unwrap_or_default();
                    hook.pause(
                        "after_admission_before_dispatch",
                        serde_json::json!({
                            "phase":"SpendGuard admission accepted; provider send not started",
                            "session_id":session_id,
                            "turn_id":turn.id,
                            "accepted_admissions":guard.session_request_count(&session_id),
                            "rejected_admissions":0,
                            "upstream_dispatch_count":0,
                            "reservation_usd":guard.total_reserved_usd(),
                            "settlement":"NOT_SETTLED_AT_INTERRUPTION",
                            "applied_mutation_ids":turn.applied_mutation_ids,
                            "skipped_mutations":turn.skipped_mutations,
                        }),
                        &[("ingress.bin", ingress.as_slice())],
                    )
                    .await;
                }
            }
            Err(error) => {
                let error_text = error.to_string();
                let evidence_saved = state
                    .store
                    .record_spend_admission(cw_store::SpendGuardAdmission {
                        turn_id: &turn_id,
                        session_id: &session_id,
                        provider: &format!("{upstream_provider:?}"),
                        model: &model,
                        admitted: false,
                        reservation_usd: None,
                        error: Some(&error_text),
                    })
                    .is_ok();
                if let Some(mut rec) = recorder
                    && let Some(task) = rec.finish("blocked")
                {
                    let _ = task.await;
                }
                return if evidence_saved {
                    (
                        StatusCode::TOO_MANY_REQUESTS,
                        format!("SpendGuard: {error}"),
                    )
                        .into_response()
                } else {
                    (
                        StatusCode::INSUFFICIENT_STORAGE,
                        "Unable to persist SpendGuard rejection evidence",
                    )
                        .into_response()
                };
            }
        }
    }
    if state.capture_wire_egress
        && let Some(ref rec) = recorder
        && let Some(ref turn) = rec.turn
        && state
            .store
            .save_wire_egress(&turn.id, &outbound_body)
            .is_err()
    {
        if let Some(mut rec) = recorder
            && let Some(task) = rec.finish("wire_evidence_error")
        {
            let _ = task.await;
        }
        return (
            StatusCode::INSUFFICIENT_STORAGE,
            "Unable to persist outbound wire evidence",
        )
            .into_response();
    }

    if state.live_allowed_session_id.is_some()
        && state
            .live_authorization_expires_at
            .is_none_or(|expires| chrono::Utc::now() >= expires)
    {
        if let Some(mut rec) = recorder
            && let Some(task) = rec.finish("authorization_expired")
        {
            let _ = task.await;
        }
        return (
            StatusCode::FORBIDDEN,
            "Smoke authorization expired before dispatch",
        )
            .into_response();
    }

    let client = if state.live_allowed_session_id.is_some() {
        &state.http_client_no_redirect
    } else {
        &state.http_client
    };
    let mut request = client.request(method, target);
    let replace_openai_auth =
        upstream_provider == ProviderKind::OpenAI && state.openai_api_key.is_some();
    for (key, value) in headers {
        if matches!(
            key.as_str(),
            "host" | "content-length" | "connection" | "transfer-encoding" | "accept-encoding"
        ) || key.as_str().starts_with("x-cw-")
            || (replace_openai_auth && key == header::AUTHORIZATION)
        {
            continue;
        }
        request = request.header(key, value);
    }
    if upstream_provider == ProviderKind::OpenAI
        && let Some(key) = state.openai_api_key.as_deref()
    {
        request = request.header(header::AUTHORIZATION, format!("Bearer {key}"));
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
    // The response body is re-streamed; keep EOF-driven recorder finalization reliable.
    response_headers.remove(header::CONTENT_LENGTH);

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
                    #[cfg(feature = "qualification-fault-hooks")]
                    if let Some(rec) = recorder.as_ref()
                        && let Some(hook) = rec.state.qualification_fault_hook.as_ref()
                        && let Some(turn) = rec.turn.as_ref()
                    {
                        let ingress = rec
                            .state
                            .store
                            .get_wire_request(&turn.id)
                            .ok()
                            .flatten()
                            .unwrap_or_default();
                        let egress = rec
                            .state
                            .store
                            .get_wire_egress(&turn.id)
                            .ok()
                            .flatten()
                            .unwrap_or_default();
                        let response = rec.buffer.clone();
                        let guard = rec.state.spend_guard.as_ref();
                        hook.pause(
                            "after_response_before_usage_settlement",
                            serde_json::json!({
                                "phase":"complete mock response body received; recorder has not settled the SpendGuard permit",
                                "session_id":turn.session_id,
                                "turn_id":turn.id,
                                "accepted_admissions":guard.map(|g|g.session_request_count(&turn.session_id)).unwrap_or(0),
                                "rejected_admissions":0,
                                "upstream_dispatch_count":1,
                                "reservation_usd":guard.map(|g|g.total_reserved_usd()),
                                "settlement":"NOT_SETTLED_AT_INTERRUPTION",
                                "observed_usage_before_settlement":rec.usage,
                                "applied_mutation_ids":turn.applied_mutation_ids,
                                "skipped_mutations":turn.skipped_mutations,
                            }),
                            &[
                                ("ingress.bin", ingress.as_slice()),
                                ("egress.bin", egress.as_slice()),
                                ("provider_response.bin", response.as_slice()),
                            ],
                        )
                        .await;
                    }
                    if let Some(rec) = recorder.as_mut() {
                        rec.response_body_complete = true;
                        let terminal = rec.terminal_outcome.clone();
                        let outcome = if !status.is_success() {
                            "upstream_error"
                        } else if let Some(ref term) = terminal {
                            term.as_str()
                        } else if rec.sse {
                            "stream_error"
                        } else {
                            "completed"
                        };
                        if let Some(task) = rec.finish(outcome) {
                            let _ = task.await;
                        }
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

#[cfg(test)]
mod redirect_tests {
    use super::forward_to_upstream;
    use crate::state::DaemonState;
    use axum::{
        Router,
        http::{HeaderMap, Method, StatusCode},
        response::Redirect,
        routing::post,
    };
    use chrono::{Duration, Utc};
    use cw_core::ir::ProviderKind;
    use cw_store::CwStore;

    async fn redirect() -> Redirect {
        Redirect::temporary("/ok")
    }

    async fn ok() -> &'static str {
        "ok"
    }

    #[tokio::test]
    async fn redirect_blocking_is_l0_only() {
        let app = Router::new()
            .route("/redirect", post(redirect))
            .route("/ok", post(ok));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut state = DaemonState::new(CwStore::open_in_memory().unwrap());
        state.openai_upstream = url.clone();
        let followed = forward_to_upstream(
            &state,
            ProviderKind::OpenAI,
            Method::POST,
            format!("{url}/redirect"),
            &HeaderMap::new(),
            b"{}".to_vec(),
            None,
        )
        .await;
        assert_eq!(followed.status(), StatusCode::OK);
        state.live_allowed_session_id = Some("cw122b2l0-test".into());
        state.live_authorization_expires_at = Some(Utc::now() + Duration::minutes(1));
        let refused = forward_to_upstream(
            &state,
            ProviderKind::OpenAI,
            Method::POST,
            format!("{url}/redirect"),
            &HeaderMap::new(),
            b"{}".to_vec(),
            None,
        )
        .await;
        assert_eq!(refused.status(), StatusCode::TEMPORARY_REDIRECT);
        server.abort();
    }
}
