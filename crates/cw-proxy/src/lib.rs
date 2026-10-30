pub mod api;
pub mod proxy;
pub mod security;
pub mod sse;
pub mod state;

pub use api::{
    EngineSlotsQuery, SpliceRequest, SwarmQueryParams, TransformContextRequest, build_api_router,
    recompute_latest_turn_in_store,
};
pub use proxy::build_proxy_router;
pub use sse::{SseChunkAccumulator, extract_cached_tokens_from_sse};
pub use state::{ApprovalAction, DaemonState, InterceptResolution, PendingInterceptInfo, WsEvent};

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use cw_core::ir::ProviderKind;
    use cw_store::CwStore;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    #[test]
    fn test_loopback_host_and_origin_checks() {
        use security::{is_loopback_host, is_loopback_origin};

        for ok in [
            "127.0.0.1",
            "127.0.0.1:4400",
            "localhost:4401",
            "LOCALHOST",
            "[::1]:4400",
            "[::1]",
        ] {
            assert!(is_loopback_host(ok), "{ok} should be accepted");
        }
        for bad in [
            "evil.example",
            "evil.example:4400",
            "127.0.0.1.evil.example",
            "localhost.evil:80",
            "[::1]x",
            "127.0.0.1:44a0",
            "",
        ] {
            assert!(!is_loopback_host(bad), "{bad} should be refused");
        }
        assert!(is_loopback_origin("http://127.0.0.1:4400"));
        assert!(is_loopback_origin("http://localhost:5173"));
        assert!(!is_loopback_origin("null"));
        assert!(!is_loopback_origin("https://evil.example"));
        assert!(!is_loopback_origin("http://localhost.evil.example"));
    }

    #[tokio::test]
    async fn test_api_refuses_cross_origin_and_rebinding() {
        let state = DaemonState::new(CwStore::open_in_memory().unwrap());
        let token = state.control_token.clone();
        let api_app = build_api_router(state);

        let cross_origin = Request::builder()
            .uri("/api/v1/sessions")
            .header("host", "127.0.0.1:4400")
            .header("origin", "https://evil.example")
            .body(Body::empty())
            .unwrap();
        let res = api_app.clone().oneshot(cross_origin).await.unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);

        let rebinding = Request::builder()
            .uri("/api/v1/sessions")
            .header("host", "evil.example:4400")
            .body(Body::empty())
            .unwrap();
        let res = api_app.clone().oneshot(rebinding).await.unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);

        let local_ui = Request::builder()
            .uri("/api/v1/sessions")
            .header("host", "127.0.0.1:4400")
            .header("origin", "http://127.0.0.1:4400")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let res = api_app.clone().oneshot(local_ui).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers().get("access-control-allow-origin").unwrap(),
            "http://127.0.0.1:4400"
        );

        let cli = Request::builder()
            .uri("/api/v1/health")
            .header("host", "127.0.0.1:4400")
            .body(Body::empty())
            .unwrap();
        let res = api_app.oneshot(cli).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_proxy_dry_run_and_api_mutations_and_replay() {
        let store = CwStore::open_in_memory().unwrap();
        let state = DaemonState::new(store);
        let store = state.store.clone();
        let token = state.control_token.clone();
        let proxy_app = build_proxy_router(state.clone());
        let api_app = build_api_router(state);

        // 1. Send dry-run Anthropic request through proxy
        let req_payload = json!({
            "model": "claude-sonnet-4",
            "max_tokens": 1024,
            "messages": [
                {
                    "role": "assistant",
                    "content": [
                        { "type": "tool_use", "id": "c1", "name": "read_file", "input": { "Path": "src/main.rs" } }
                    ]
                },
                {
                    "role": "user",
                    "content": [
                        { "type": "tool_result", "tool_use_id": "c1", "content": "fn main() { println!(\"v1\"); }" }
                    ]
                },
                {
                    "role": "assistant",
                    "content": [
                        { "type": "tool_use", "id": "c2", "name": "read_file", "input": { "Path": "src/main.rs" } }
                    ]
                },
                {
                    "role": "user",
                    "content": [
                        { "type": "tool_result", "tool_use_id": "c2", "content": "fn main() { println!(\"v1\"); }" }
                    ]
                }
            ]
        });

        let req = Request::builder()
            .method("POST")
            .uri("/anthropic/v1/messages")
            .header("content-type", "application/json")
            .header("x-cw-session-id", "test_sess")
            .header("x-cw-control-token", &token)
            .header("x-cw-dry-run", "1")
            .body(Body::from(serde_json::to_vec(&req_payload).unwrap()))
            .unwrap();

        let resp = proxy_app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // 2. Call 1-Click Optimize on the control API
        let opt_req = Request::builder()
            .method("POST")
            .uri("/api/v1/sessions/test_sess/optimize")
            .header("if-match", store.session_revision("test_sess").unwrap())
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let opt_resp = api_app.clone().oneshot(opt_req).await.unwrap();
        assert_eq!(opt_resp.status(), StatusCode::OK);

        // 3. Call Time-Travel Fork & Replay on the control API
        let replay_req = Request::builder()
            .method("POST")
            .uri("/api/v1/sessions/test_sess/replay")
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({ "from_turn_id": null })).unwrap(),
            ))
            .unwrap();
        let replay_resp = api_app.oneshot(replay_req).await.unwrap();
        assert_eq!(replay_resp.status(), StatusCode::OK);
        let bytes = to_bytes(replay_resp.into_body(), 1_000_000).await.unwrap();
        let val: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(val.get("forked_turn").is_some());
    }

    #[tokio::test]
    async fn test_pre_forward_validation_gate_fail_open_fallback() {
        use cw_core::validator::validate_protocol_invariants;

        let valid_raw = json!({
            "model": "claude-3-7-sonnet",
            "messages": [
                { "role": "user", "content": "Hello" },
                { "role": "assistant", "content": [{ "type": "text", "text": "Hi" }] }
            ]
        });

        // Corrupted mutation payload with orphan tool result
        let invalid_mutated = json!({
            "model": "claude-3-7-sonnet",
            "messages": [
                { "role": "user", "content": "Hello" },
                { "role": "assistant", "content": [{ "type": "text", "text": "Hi" }] },
                {
                    "role": "user",
                    "content": [
                        { "type": "tool_result", "tool_use_id": "orphan_c99", "content": "bad" }
                    ]
                }
            ]
        });

        assert!(validate_protocol_invariants(&valid_raw, ProviderKind::Anthropic).is_ok());
        assert!(validate_protocol_invariants(&invalid_mutated, ProviderKind::Anthropic).is_err());

        // In cw-proxy, the fail-open fallback replaces invalid_mutated with valid_raw
        let mut outbound = invalid_mutated.clone();
        if validate_protocol_invariants(&outbound, ProviderKind::Anthropic).is_err()
            && validate_protocol_invariants(&valid_raw, ProviderKind::Anthropic).is_ok()
        {
            outbound = valid_raw.clone();
        }

        assert_eq!(outbound, valid_raw);
        assert!(validate_protocol_invariants(&outbound, ProviderKind::Anthropic).is_ok());
    }

    #[test]
    fn test_split_chunk_sse_accumulator() {
        let mut acc = SseChunkAccumulator::default();

        // Chunk 1: Partial SSE event split right in the middle of json key
        let chunk1 =
            b"event: message_delta\ndata: {\"type\": \"message_delta\", \"usage\": {\"cache_";
        let res1 = acc.push_chunk(chunk1);
        assert_eq!(res1, None); // Not ready yet

        // Chunk 2: The rest of the key and values, followed by newline
        let chunk2 = b"read_input_tokens\": 2048}}\n\n";
        let res2 = acc.push_chunk(chunk2);
        assert_eq!(res2, Some(2048)); // Successfully reassembled and parsed!

        // Chunk 3: Normal complete chunk
        let chunk3 = b"data: {\"usage\": {\"input_tokens_details\": {\"cached_tokens\": 512}}}\n";
        let res3 = acc.push_chunk(chunk3);
        assert_eq!(res3, Some(512));
    }
}
