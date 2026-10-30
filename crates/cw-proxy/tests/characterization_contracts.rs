use axum::{
    Json, Router,
    body::{Body, to_bytes},
    http::{HeaderMap, Request, StatusCode},
    routing::post,
};
use chrono::Utc;
use cw_core::ir::*;
use cw_proxy::{
    DaemonState, SseChunkAccumulator, build_api_router, build_proxy_router,
    extract_cached_tokens_from_sse,
};
use cw_store::CwStore;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

#[allow(dead_code)]
#[derive(Clone, Debug)]
struct CapturedRecord {
    path: String,
    headers: HeaderMap,
    body: Vec<u8>,
}

struct MockUpstream {
    url: String,
    captured: Arc<Mutex<Vec<CapturedRecord>>>,
    _handle: tokio::task::JoinHandle<()>,
}

impl MockUpstream {
    async fn new(routes: &[&str]) -> Self {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let mut router = Router::new();

        for &route in routes {
            let captured_clone = captured.clone();
            let path_str = route.to_string();
            router = router.route(
                route,
                post(move |headers: HeaderMap, body: axum::body::Bytes| {
                    let cap = captured_clone.clone();
                    let p = path_str.clone();
                    async move {
                        cap.lock().unwrap().push(CapturedRecord {
                            path: p,
                            headers,
                            body: body.to_vec(),
                        });
                        Json(json!({
                            "id": "mock_resp_1",
                            "choices": [{"message": {"role": "assistant", "content": "mock output"}}],
                            "usage": {"prompt_tokens": 10, "completion_tokens": 5}
                        }))
                    }
                }),
            );
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });

        Self {
            url: format!("http://{addr}"),
            captured,
            _handle: handle,
        }
    }
}

// =============================================================================
// Contract 1: Context Transform API (/api/v1/sessions/:id/transform)
// =============================================================================

#[tokio::test]
async fn contract_transform_context_api_auth_validation_and_fail_open() {
    let store = CwStore::open_in_memory().unwrap();
    let mut state = DaemonState::new(store.clone());
    let control_token = "valid_control_token_32_chars_long_1234".to_string();
    state.control_token = control_token.clone();

    let session_id = "test_char_session_transform";
    store
        .upsert_session(&Session {
            id: session_id.into(),
            parent_session_id: None,
            harness: HarnessKind::Omp,
            workspace_root: None,
            model: "mock-model".into(),
            provider: ProviderKind::OpenAI,
            title: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        })
        .unwrap();

    let api_app = build_api_router(state);

    // 1. Missing control token -> 401 Unauthorized (via control_guard)
    let req_unauth = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/sessions/{session_id}/transform"))
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "messages": [{"role": "user", "content": "hi"}]
            })
            .to_string(),
        ))
        .unwrap();

    let resp = api_app.clone().oneshot(req_unauth).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // 2. Wrong control token -> 401 Unauthorized (via control_guard)
    let req_wrong_token = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/sessions/{session_id}/transform"))
        .header("content-type", "application/json")
        .header("x-cw-control-token", "wrong-token-value")
        .body(Body::from(
            json!({
                "messages": [{"role": "user", "content": "hi"}]
            })
            .to_string(),
        ))
        .unwrap();

    let resp = api_app.clone().oneshot(req_wrong_token).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // 3. Malformed payload: messages is not an array -> 400 Bad Request
    let req_malformed = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/sessions/{session_id}/transform"))
        .header("content-type", "application/json")
        .header("x-cw-control-token", &control_token)
        .body(Body::from(
            json!({
                "messages": "not an array"
            })
            .to_string(),
        ))
        .unwrap();

    let resp = api_app.clone().oneshot(req_malformed).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // 4. Authenticated call with no active mutations -> 200 OK, exact same messages returned
    let original_messages = json!([
        {"role": "user", "content": "hello world", "custom_meta": 42}
    ]);
    let req_empty_rules = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/sessions/{session_id}/transform"))
        .header("content-type", "application/json")
        .header("x-cw-control-token", &control_token)
        .body(Body::from(
            json!({
                "messages": original_messages.clone(),
                "provider": "openai"
            })
            .to_string(),
        ))
        .unwrap();

    let resp = api_app.clone().oneshot(req_empty_rules).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), 10000).await.unwrap();
    let body_json: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body_json["status"], "ok");
    assert_eq!(body_json["messages"], original_messages);
    assert_eq!(body_json["applied_rules"], json!([]));

    // 5. Active committed PinContext mutation -> 200 OK, pin injected into outbound prompt, custom metadata preserved
    let pin_rule = MutationRule {
        id: "mut_contract_pin_01".into(),
        session_id: session_id.into(),
        op: MutationOp::PinContext {
            label: "TEST_PIN".into(),
            content: "Enforce strict typing".into(),
            source_node_id: None,
        },
        status: MutationStatus::Committed,
        estimated_saved_tokens: 0,
        estimated_kv_break_tokens: 0,
        created_at: Utc::now(),
    };
    store.save_mutation(&pin_rule).unwrap();

    let req_with_pin = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/sessions/{session_id}/transform"))
        .header("content-type", "application/json")
        .header("x-cw-control-token", &control_token)
        .body(Body::from(
            json!({
                "messages": original_messages.clone(),
                "provider": "openai"
            })
            .to_string(),
        ))
        .unwrap();

    let resp = api_app.clone().oneshot(req_with_pin).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), 10000).await.unwrap();
    let body_json: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body_json["status"], "ok");
    assert_eq!(body_json["applied_rules"], json!(["mut_contract_pin_01"]));

    let transformed_messages = body_json["messages"].as_array().unwrap();
    assert_eq!(transformed_messages.len(), 1);
    let content = transformed_messages[0]["content"].as_str().unwrap();
    assert!(content.contains("<cw_pinned_context"));
    assert!(content.contains("Enforce strict typing"));
    // Custom metadata property must be strictly preserved
    assert_eq!(transformed_messages[0]["custom_meta"], 42);

    // 6. Pre-Forward Validation Gate Fail-Open:
    // If messages have a protocol invariant violation (e.g. an orphan tool result without tool call),
    // the gate catches it and fails open to raw messages with status "fallback_raw" instead of breaking the session.
    let orphan_tool_messages = json!([
        {"role": "user", "content": "run test"},
        {"role": "tool", "tool_call_id": "orphan_call_id_999", "content": "error output"}
    ]);
    let req_orphan = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/sessions/{session_id}/transform"))
        .header("content-type", "application/json")
        .header("x-cw-control-token", &control_token)
        .body(Body::from(
            json!({
                "messages": orphan_tool_messages.clone(),
                "provider": "openai"
            })
            .to_string(),
        ))
        .unwrap();

    let resp = api_app.clone().oneshot(req_orphan).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), 10000).await.unwrap();
    let body_json: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        body_json["status"], "fallback_raw",
        "Protocol invariant violation must fail open to fallback_raw"
    );
    assert_eq!(body_json["messages"], orphan_tool_messages);
}

// =============================================================================
// Contract 2: Proxy Routing & Fallback Provider Selection
// =============================================================================

#[tokio::test]
async fn contract_proxy_routing_and_fallback_provider_selection() {
    let mock_anthropic = MockUpstream::new(&["/v1/messages"]).await;
    let mock_openai = MockUpstream::new(&["/v1/chat/completions"]).await;
    let mock_deepseek = MockUpstream::new(&["/chat/completions"]).await;
    let mock_gemini = MockUpstream::new(&["/v1beta/models/gemini-1.5:generateContent"]).await;

    let store = CwStore::open_in_memory().unwrap();
    let mut state = DaemonState::new(store);
    state.anthropic_upstream = mock_anthropic.url.clone();
    state.openai_upstream = mock_openai.url.clone();
    state.deepseek_upstream = mock_deepseek.url.clone();
    state.gemini_upstream = mock_gemini.url.clone();

    let proxy_app = build_proxy_router(state);

    // 1. Prefix routing: /deepseek routes to deepseek upstream
    let req = Request::builder()
        .method("POST")
        .uri("/deepseek/chat/completions")
        .header("host", "127.0.0.1")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"model": "deepseek-chat", "messages": []}).to_string(),
        ))
        .unwrap();
    let resp = proxy_app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(mock_deepseek.captured.lock().unwrap().len(), 1);

    // 2. Prefix routing: /openai/v1 routes to openai upstream
    let req = Request::builder()
        .method("POST")
        .uri("/openai/v1/chat/completions")
        .header("host", "127.0.0.1")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"model": "gpt-4o", "messages": []}).to_string(),
        ))
        .unwrap();
    let resp = proxy_app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(mock_openai.captured.lock().unwrap().len(), 1);

    // 3. Fallback routing: unprefixed /v1/messages routes to anthropic upstream
    let req = Request::builder()
        .method("POST")
        .uri("/v1/messages")
        .header("host", "127.0.0.1")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"model": "claude-3-5-sonnet", "messages": []}).to_string(),
        ))
        .unwrap();
    let resp = proxy_app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(mock_anthropic.captured.lock().unwrap().len(), 1);

    // 4. Fallback routing: unprefixed path containing generateContent routes to gemini upstream
    let req = Request::builder()
        .method("POST")
        .uri("/v1beta/models/gemini-1.5:generateContent")
        .header("host", "127.0.0.1")
        .header("content-type", "application/json")
        .body(Body::from(json!({"contents": []}).to_string()))
        .unwrap();
    let resp = proxy_app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(mock_gemini.captured.lock().unwrap().len(), 1);
}

// =============================================================================
// Contract 3: Credential & Internal Header Stripping Before Upstream Forwarding
// =============================================================================

#[tokio::test]
async fn contract_proxy_strips_internal_cw_headers_before_forwarding() {
    let mock = MockUpstream::new(&["/v1/chat/completions"]).await;
    let store = CwStore::open_in_memory().unwrap();
    let mut state = DaemonState::new(store.clone());
    let token = "test_auth_control_token_32_characters".to_string();
    state.control_token = token.clone();
    state.openai_upstream = mock.url.clone();

    let proxy_app = build_proxy_router(state);
    let session_id = "test_strip_headers_sess";

    let req = Request::builder()
        .method("POST")
        .uri("/openai/v1/chat/completions")
        .header("host", "127.0.0.1")
        .header("content-type", "application/json")
        .header("authorization", "Bearer client-secret-key")
        .header("x-client-trace-id", "trace-98765")
        .header("x-cw-session-id", session_id)
        .header("x-cw-control-token", &token)
        .header("x-cw-workspace", "/home/user/project")
        .body(Body::from(
            json!({
                "model": "gpt-4o",
                "messages": [{"role": "user", "content": "test headers"}]
            })
            .to_string(),
        ))
        .unwrap();

    let resp = proxy_app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let captured = mock.captured.lock().unwrap();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].path, "/v1/chat/completions");
    assert!(!captured[0].body.is_empty());
    let upstream_headers = &captured[0].headers;

    // Client application headers must be preserved
    assert_eq!(
        upstream_headers.get("authorization").unwrap(),
        "Bearer client-secret-key"
    );
    assert_eq!(
        upstream_headers.get("x-client-trace-id").unwrap(),
        "trace-98765"
    );

    // All internal x-cw-* headers must be completely stripped before reaching upstream
    assert!(
        upstream_headers.get("x-cw-session-id").is_none(),
        "x-cw-session-id leaked to upstream"
    );
    assert!(
        upstream_headers.get("x-cw-control-token").is_none(),
        "x-cw-control-token leaked to upstream"
    );
    assert!(
        upstream_headers.get("x-cw-workspace").is_none(),
        "x-cw-workspace leaked to upstream"
    );
}

// =============================================================================
// Contract 3b: Dry-Run Preview Never Calls Upstream
// =============================================================================

#[tokio::test]
async fn contract_proxy_dry_run_preview_never_calls_upstream() {
    let mock = MockUpstream::new(&["/v1/chat/completions"]).await;
    let store = CwStore::open_in_memory().unwrap();
    let mut state = DaemonState::new(store);
    let token = "test_auth_control_token_32_characters".to_string();
    state.control_token = token.clone();
    state.openai_upstream = mock.url.clone();

    let proxy_app = build_proxy_router(state);
    let session_id = "test_dry_run_sess";

    let req = Request::builder()
        .method("POST")
        .uri("/openai/v1/chat/completions")
        .header("host", "127.0.0.1")
        .header("content-type", "application/json")
        .header("x-cw-session-id", session_id)
        .header("x-cw-control-token", &token)
        .header("x-cw-dry-run", "1")
        .body(Body::from(
            json!({
                "model": "gpt-4o",
                "messages": [{"role": "user", "content": "preview this turn"}]
            })
            .to_string(),
        ))
        .unwrap();

    let resp = proxy_app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = to_bytes(resp.into_body(), 10000).await.unwrap();
    let body_json: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body_json["cw_dry_run"], true);
    assert_eq!(body_json["turn"]["outcome"], "preview");

    // Invariant: Dry-run preview MUST NEVER call upstream inference endpoints
    assert_eq!(
        mock.captured.lock().unwrap().len(),
        0,
        "Dry-run preview must never reach upstream network provider"
    );
}

// =============================================================================
// Contract 4: Unauthenticated Request Generates Synthetic Observation Session
// =============================================================================

#[tokio::test]
async fn contract_unauthenticated_request_generates_synthetic_session() {
    let mock = MockUpstream::new(&["/v1/chat/completions"]).await;
    let store = CwStore::open_in_memory().unwrap();
    let mut state = DaemonState::new(store.clone());
    state.openai_upstream = mock.url.clone();

    let proxy_app = build_proxy_router(state);

    // Send unauthenticated request without session or token headers
    let payload = json!({
        "model": "gpt-4o",
        "messages": [{"role": "user", "content": "unauthenticated observation prompt"}]
    });
    let req = Request::builder()
        .method("POST")
        .uri("/openai/v1/chat/completions")
        .header("host", "127.0.0.1")
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))
        .unwrap();

    let resp = proxy_app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Verify synthetic session was created in CwStore starting with 'sess_'
    let sessions = store.list_sessions().unwrap();
    assert_eq!(
        sessions.len(),
        1,
        "A synthetic observation session must be created"
    );
    assert!(
        sessions[0].id.starts_with("sess_"),
        "Synthetic session must start with 'sess_': {}",
        sessions[0].id
    );

    // Verify turn record was created under the synthetic session
    let turns = store.list_turns(&sessions[0].id).unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].sequence_number, 1);
}

// =============================================================================
// Contract 5: SSE Streaming Chunk Accumulator & Cached Tokens Extraction
// =============================================================================

#[tokio::test]
async fn contract_sse_streaming_chunk_accumulator_and_cached_tokens() {
    let mut accumulator = SseChunkAccumulator::default();

    // Chunk 1: Content delta without usage
    let chunk1 = b"data: {\"id\":\"gen_1\",\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n";
    let cached1 = accumulator.push_chunk(chunk1);
    assert_eq!(cached1, None);

    // Chunk 2: Fragmented chunk containing prompt_tokens_details.cached_tokens
    let chunk2 = b"data: {\"id\":\"gen_1\",\"choices\":[],\"usage\":{\"prompt_tokens\":120,\"completion_tokens\":25,\"prompt_tokens_details\":{\"cached_tokens\":80}}}\n\n";
    let cached2 = accumulator.push_chunk(chunk2);
    assert_eq!(cached2, Some(80));

    // Chunk 3: DONE sentinel
    let chunk3 = b"data: [DONE]\n\n";
    let cached3 = accumulator.push_chunk(chunk3);
    assert_eq!(cached3, None);

    // Verify extract_cached_tokens_from_sse helper directly
    let sse_text = "data: {\"usage\":{\"prompt_tokens_details\":{\"cached_tokens\":150}}}\n\n";
    let extracted = extract_cached_tokens_from_sse(sse_text);
    assert_eq!(extracted, Some(150));

    let no_cached = extract_cached_tokens_from_sse("data: {\"delta\":{\"text\":\"abc\"}}\n\n");
    assert_eq!(no_cached, None);
}
