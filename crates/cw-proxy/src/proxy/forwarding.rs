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
                    if let Some(rec) = recorder.as_mut() {
                        let terminal = rec.terminal_outcome.clone();
                        let outcome = if !status.is_success() {
                            "upstream_error"
                        } else if let Some(ref term) = terminal {
                            term.as_str()
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
