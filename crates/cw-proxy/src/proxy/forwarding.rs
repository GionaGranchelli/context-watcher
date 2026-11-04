use axum::{
    body::Body,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use futures::StreamExt;

use crate::{proxy::recorder::Recorder, state::DaemonState};

/// Forwards an outbound HTTP request to the upstream LLM provider,
/// filtering internal and hop-by-hop headers, and streams the response
/// while tapping telemetry and usage metrics through the `Recorder`.
pub async fn forward_to_upstream(
    state: &DaemonState,
    method: axum::http::Method,
    target: String,
    headers: &HeaderMap,
    outbound_body: Vec<u8>,
    mut recorder: Option<Recorder>,
) -> Response {
    // GET (e.g. model listing) is not a billable generation request.
    if let Some(guard) = state.spend_guard.as_ref()
        && method != axum::http::Method::GET
    {
        // Live spend control: admit before dispatch, never forward unmetered requests.
        let Some(session_id) = recorder
            .as_ref()
            .and_then(|r| r.turn.as_ref())
            .map(|t| t.session_id.clone())
        else {
            return (
                StatusCode::FORBIDDEN,
                "SpendGuard: unmetered provider request rejected",
            )
                .into_response();
        };
        match guard.admit_request(&session_id) {
            Ok(permit) => {
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
                if let Some(mut rec) = recorder {
                    rec.finish("blocked");
                }
                return (
                    StatusCode::TOO_MANY_REQUESTS,
                    format!("SpendGuard: {error}"),
                )
                    .into_response();
            }
        }
    }
    if state.capture_wire_egress
        && let Some(ref rec) = recorder
        && let Some(ref turn) = rec.turn
    {
        let _ = state.store.save_wire_egress(&turn.id, &outbound_body);
    }

    let mut request = state.http_client.request(method, target);
    for (key, value) in headers {
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
