use axum::{
    body::Body,
    extract::State,
    http::{HeaderValue, Request, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::state::DaemonState;

/// Returns true when `host` (a `Host` header value, optionally with a port) names the
/// loopback interface. Used to reject DNS-rebinding requests, where a hostile domain is
/// re-pointed at 127.0.0.1 and the browser sends `Host: evil.example`.
pub fn is_loopback_host(host: &str) -> bool {
    let host = host.trim();
    let name = if let Some(rest) = host.strip_prefix('[') {
        // IPv6 literal: "[::1]" or "[::1]:4400"
        match rest.split_once(']') {
            Some((addr, tail)) if tail.is_empty() || tail.starts_with(':') => addr,
            _ => return false,
        }
    } else {
        match host.rsplit_once(':') {
            Some((name, port)) if port.chars().all(|c| c.is_ascii_digit()) => name,
            Some(_) => return false,
            None => host,
        }
    };
    let name = name.to_ascii_lowercase();
    name == "localhost" || name == "127.0.0.1" || name == "::1"
}

/// Returns true when an `Origin` header value is a page served from the local machine
/// (the embedded web UI, or the Vite dev server). `null` and every remote origin are refused.
pub fn is_loopback_origin(origin: &str) -> bool {
    match origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
    {
        Some(rest) if !rest.contains('/') => is_loopback_host(rest),
        _ => false,
    }
}

/// CORS policy for the daemon: only pages served from loopback may read responses.
/// Replaces the previous `CorsLayer::permissive()`, which let any website read session
/// transcripts and drive the control API from the user's browser.
pub fn local_cors_layer() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(|origin: &HeaderValue, _| {
            origin.to_str().map(is_loopback_origin).unwrap_or(false)
        }))
        .allow_methods(tower_http::cors::Any)
        .allow_headers(tower_http::cors::Any)
}

/// Rejects requests that did not come from the local machine's own pages or tools:
/// * a `Host` header that is not loopback (blocks DNS rebinding), and
/// * an `Origin` header that is not loopback (blocks cross-site requests, including
///   "simple" form POSTs and cross-origin WebSocket upgrades, which CORS does not stop).
///
/// CLI, MCP and harness clients send no `Origin` header and a loopback `Host`, so they are
/// unaffected.
pub async fn local_only_guard(req: Request<Body>, next: Next) -> Response {
    let headers = req.headers();
    if headers
        .get(header::HOST)
        .is_some_and(|h| !h.to_str().map(is_loopback_host).unwrap_or(false))
    {
        return (
            StatusCode::FORBIDDEN,
            "contextWatcher: non-loopback Host refused",
        )
            .into_response();
    }
    if headers.get(header::ORIGIN).is_some_and(|o| {
        let origin = o.to_str().unwrap_or("");
        let host = headers
            .get(header::HOST)
            .and_then(|h| h.to_str().ok())
            .unwrap_or("");
        !is_loopback_origin(origin)
            || (origin != format!("http://{host}")
                && origin != format!("https://{host}")
                && std::env::var("CW_UI_ORIGIN").ok().as_deref() != Some(origin))
    }) {
        return (
            StatusCode::FORBIDDEN,
            "contextWatcher: cross-origin request refused",
        )
            .into_response();
    }
    next.run(req).await
}

pub async fn control_guard(
    State(state): State<DaemonState>,
    req: Request<Body>,
    next: Next,
) -> Response {
    if req.uri().path() == "/api/v1/health" {
        return next.run(req).await;
    }
    let bearer = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let control_header = req
        .headers()
        .get("x-cw-control-token")
        .and_then(|v| v.to_str().ok());
    let ws_token = req
        .headers()
        .get(header::SEC_WEBSOCKET_PROTOCOL)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            v.split(',')
                .map(str::trim)
                .find_map(|v| v.strip_prefix("cw-token."))
        });
    if bearer.or(control_header).or(ws_token) != Some(state.control_token.as_str()) {
        return (
            StatusCode::UNAUTHORIZED,
            "Use cw auth-token to connect a local client",
        )
            .into_response();
    }
    next.run(req).await
}
