use axum::{
    Json, Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    response::Response,
    routing::{get, post},
};
use chrono::Utc;
use cw_core::ir::*;
use cw_proxy::{DaemonState, build_api_router, build_proxy_router};
use cw_store::CwStore;
use serde_json::json;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use tower::ServiceExt;
async fn mock(app: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{address}"), task)
}
fn request(body: &str, id: Option<&str>, token: &str) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/openai/v1/chat/completions")
        .header("host", "127.0.0.1")
        .header("content-type", "application/json");
    if let Some(id) = id {
        builder = builder
            .header("x-cw-session-id", id)
            .header("x-cw-control-token", token);
    }
    builder.body(Body::from(body.to_string())).unwrap()
}
#[tokio::test]
async fn observer_forwards_original_bytes_and_keeps_unrelated_sessions_separate() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let copy = captured.clone();
    let (url, task) = mock(Router::new().route(
        "/v1/chat/completions",
        post(move |body: axum::body::Bytes| {
            let copy = copy.clone();
            async move {
                copy.lock().unwrap().push(body.to_vec());
                Json(json!({"usage":{"prompt_tokens":2,"completion_tokens":1}}))
            }
        }),
    ))
    .await;
    let store = CwStore::open_in_memory().unwrap();
    let mut state = DaemonState::new(store.clone());
    state.openai_upstream = url;
    let app = build_proxy_router(state);
    let raw = "{ \"model\":\"test\", \"messages\":[{\"role\":\"user\",\"content\":\"literal <cw_pinned_context>keep</cw_pinned_context>\"}],\"cw_customer_id\":42 }";
    for _ in 0..2 {
        let response = app.clone().oneshot(request(raw, None, "")).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        to_bytes(response.into_body(), 10000).await.unwrap();
    }
    assert_eq!(
        *captured.lock().unwrap(),
        vec![raw.as_bytes().to_vec(), raw.as_bytes().to_vec()]
    );
    assert_eq!(store.list_sessions().unwrap().len(), 2);
    task.abort();
}

#[tokio::test]
async fn deepseek_route_uses_its_upstream_and_preserves_chat_endpoint() {
    let captured = Arc::new(Mutex::new(None));
    let copy = captured.clone();
    let (url, task) = mock(Router::new().route(
        "/v1/chat/completions",
        post(
            move |headers: axum::http::HeaderMap, body: axum::body::Bytes| {
                let copy = copy.clone();
                async move {
                    *copy.lock().unwrap() =
                        Some((body.to_vec(), headers.contains_key("x-cw-session-id")));
                    Json(json!({"choices":[],"usage":{"prompt_tokens":1,"completion_tokens":1}}))
                }
            },
        ),
    ))
    .await;
    let mut state = DaemonState::new(CwStore::open_in_memory().unwrap());
    state.deepseek_upstream = url;
    let app = build_proxy_router(state);
    let request = Request::builder()
        .method("POST")
        .uri("/deepseek/v1/chat/completions")
        .header("host", "127.0.0.1")
        .header("content-type", "application/json")
        .body(Body::from(
            r#"{"model":"deepseek-v4-flash","messages":[{"role":"user","content":"ping"}]}"#,
        ))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    to_bytes(response.into_body(), 10000).await.unwrap();
    let (body, session_header_forwarded) = captured.lock().unwrap().clone().unwrap();
    assert!(
        String::from_utf8(body)
            .unwrap()
            .contains("deepseek-v4-flash")
    );
    assert!(!session_header_forwarded);
    task.abort();
}

#[tokio::test]
async fn authenticated_session_headers_apply_overlay_and_are_not_forwarded_upstream() {
    let captured = Arc::new(Mutex::new(None));
    let copy = captured.clone();
    let (url, task) = mock(Router::new().route(
        "/v1/responses",
        post(move |headers: axum::http::HeaderMap, body: axum::body::Bytes| {
            let copy = copy.clone();
            async move {
                *copy.lock().unwrap() = Some((
                    body.to_vec(),
                    headers.contains_key("x-cw-session-id"),
                    headers.contains_key("x-cw-control-token"),
                ));
                Json(json!({"id":"resp_fixture","output":[],"usage":{"input_tokens":1,"output_tokens":1}}))
            }
        }),
    ))
    .await;
    let store = CwStore::open_in_memory().unwrap();
    store
        .set_autopilot_config(&cw_core::autopilot::AutoPilotConfig {
            enabled: true,
            ..Default::default()
        })
        .unwrap();
    let now = Utc::now();
    store
        .upsert_session(&Session {
            id: "r7_native_session".into(),
            parent_session_id: None,
            harness: HarnessKind::GenericProxy,
            workspace_root: None,
            model: "test-model".into(),
            provider: ProviderKind::OpenAI,
            title: None,
            created_at: now,
            updated_at: now,
        })
        .unwrap();
    store
        .save_mutation(&MutationRule {
            id: "pin_r7_session".into(),
            session_id: "r7_native_session".into(),
            op: MutationOp::PinContext {
                label: "R7 pin".into(),
                content: "overlay applied only to authenticated session".into(),
                source_node_id: None,
            },
            status: MutationStatus::Committed,
            estimated_saved_tokens: 0,
            estimated_kv_break_tokens: 0,
            created_at: now,
        })
        .unwrap();
    let mut state = DaemonState::new(store.clone());
    state.openai_upstream = url;
    let token = state.control_token.clone();
    let request = Request::builder()
        .method("POST")
        .uri("/openai/v1/responses")
        .header("content-type", "application/json")
        .header("x-cw-session-id", "r7_native_session")
        .header("x-cw-control-token", token)
        .body(Body::from(
            r#"{"model":"test-model","input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"continue"}]}]}"#,
        ))
        .unwrap();
    let response = build_proxy_router(state).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    to_bytes(response.into_body(), 10000).await.unwrap();

    let (forwarded_body, session_header_forwarded, token_header_forwarded) =
        captured.lock().unwrap().clone().unwrap();
    let forwarded: serde_json::Value = serde_json::from_slice(&forwarded_body).unwrap();
    assert!(
        forwarded
            .to_string()
            .contains("overlay applied only to authenticated session")
    );
    assert!(!session_header_forwarded);
    assert!(!token_header_forwarded);
    assert_eq!(store.list_turns("r7_native_session").unwrap().len(), 1);
    task.abort();
}

#[tokio::test]
async fn signed_anthropic_payload_is_forwarded_unchanged_with_committed_overlay() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let copy = captured.clone();
    let (url, task) = mock(Router::new().route(
        "/v1/messages",
        post(move |body: axum::body::Bytes| {
            let copy = copy.clone();
            async move {
                copy.lock().unwrap().push(body.to_vec());
                Json(json!({"type":"message","content":[],"usage":{"input_tokens":1,"output_tokens":1}}))
            }
        }),
    ))
    .await;
    let store = CwStore::open_in_memory().unwrap();
    store
        .set_autopilot_config(&cw_core::autopilot::AutoPilotConfig {
            enabled: true,
            ..Default::default()
        })
        .unwrap();
    let now = Utc::now();
    store
        .upsert_session(&Session {
            id: "signed_session".into(),
            parent_session_id: None,
            harness: HarnessKind::GenericProxy,
            workspace_root: None,
            model: "claude-test".into(),
            provider: ProviderKind::Anthropic,
            title: None,
            created_at: now,
            updated_at: now,
        })
        .unwrap();
    store
        .save_mutation(&MutationRule {
            id: "pin_signed_session".into(),
            session_id: "signed_session".into(),
            op: MutationOp::PinContext {
                label: "keep".into(),
                content: "important context".into(),
                source_node_id: None,
            },
            status: MutationStatus::Committed,
            estimated_saved_tokens: 0,
            estimated_kv_break_tokens: 0,
            created_at: now,
        })
        .unwrap();
    let mut state = DaemonState::new(store.clone());
    state.anthropic_upstream = url;
    state.control_token = "r7-test-control-token-000000000000000000000".into();
    let body = r#"{"model":"claude-test","max_tokens":32,"messages":[{"role":"assistant","content":[{"type":"thinking","thinking":"preserve this","signature":"fixture-signature-value"}]},{"role":"user","content":"continue"}]}"#;
    let request = Request::builder()
        .method("POST")
        .uri("/anthropic/v1/messages")
        .header("content-type", "application/json")
        .header("x-cw-session-id", "signed_session")
        .header(
            "x-cw-control-token",
            "r7-test-control-token-000000000000000000000",
        )
        .body(Body::from(body))
        .unwrap();
    let response = build_proxy_router(state).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    to_bytes(response.into_body(), 10000).await.unwrap();
    assert_eq!(*captured.lock().unwrap(), vec![body.as_bytes().to_vec()]);
    assert!(
        store
            .list_mutations("signed_session")
            .unwrap()
            .iter()
            .any(|m| m.id == "pin_signed_session")
    );
    task.abort();
}

#[tokio::test]
async fn signed_anthropic_sse_response_is_forwarded_unchanged() {
    let signed_events: &'static str = concat!(
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_fixture\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"claude-test\",\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
        "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"private thought\"}}\n\n",
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"fixture-provider-signature\"}}\n\n",
        "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"
    );
    let expected_bytes = signed_events.as_bytes().to_vec();
    let (url, task) = mock(Router::new().route(
        "/v1/messages",
        post(move || async move {
            Response::builder()
                .header("content-type", "text/event-stream")
                .body(Body::from(signed_events))
                .unwrap()
        }),
    ))
    .await;
    let mut state = DaemonState::new(CwStore::open_in_memory().unwrap());
    state.anthropic_upstream = url;
    let request = Request::builder()
        .method("POST")
        .uri("/anthropic/v1/messages")
        .header("content-type", "application/json")
        .body(Body::from(
            r#"{"model":"claude-test","max_tokens":32,"messages":[{"role":"user","content":"continue"}]}"#,
        ))
        .unwrap();
    let response = build_proxy_router(state).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "text/event-stream"
    );
    let response_bytes = to_bytes(response.into_body(), 10000).await.unwrap();
    assert_eq!(response_bytes.as_ref(), expected_bytes.as_slice());
    task.abort();
}

#[tokio::test]
async fn approval_timeout_blocks_signed_requests_without_enabling_mutation() {
    let calls = Arc::new(AtomicUsize::new(0));
    let copy = calls.clone();
    let (url, task) = mock(Router::new().route(
        "/v1/chat/completions",
        post(move || {
            let copy = copy.clone();
            async move {
                copy.fetch_add(1, Ordering::SeqCst);
                Json(json!({}))
            }
        }),
    ))
    .await;
    let store = CwStore::open_in_memory().unwrap();
    store
        .upsert_firewall_rule(&FirewallRule {
            id: "approval".into(),
            session_id: None,
            name: "Require approval".into(),
            trigger: FirewallTrigger::ManualStepMode,
            action: FirewallAction::PauseForHumanApproval { timeout_secs: 0 },
            enabled: true,
        })
        .unwrap();
    let mut state = DaemonState::new(store.clone());
    state.openai_upstream = url;
    let response=build_proxy_router(state).oneshot(request(&json!({"model":"test","messages":[{"role":"user","content":"hello"},{"role":"assistant","content":[{"type":"thinking","thinking":"opaque","signature":"signed"}]}]}).to_string(),None,"")).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    task.abort();
}
#[tokio::test]
async fn malformed_preview_never_calls_upstream() {
    let store = CwStore::open_in_memory().unwrap();
    let state = DaemonState::new(store);
    let app = build_proxy_router(state);
    let mut req = request("not json", None, "");
    req.headers_mut()
        .insert("x-cw-dry-run", "1".parse().unwrap());
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
#[tokio::test]
async fn concurrent_requests_keep_both_records_and_sse_final_usage() {
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let copy = barrier.clone();
    let (url,task)=mock(Router::new().route("/v1/chat/completions",post(move||{let barrier=copy.clone();async move{barrier.wait().await;
        let chunks=vec![Ok::<_,std::io::Error>("data: {\"usage\":null}\n\ndata: {\"usage\":{\"prompt_tokens\":20,\"completion_tokens\":3,\"prompt_tokens_det"),Ok("ails\":{\"cached_tokens\":8}}}\n\ndata: [DONE]\n\n")];
        ([("content-type","text/event-stream")],Body::from_stream(futures::stream::iter(chunks)))}}))).await;
    let store = CwStore::open_in_memory().unwrap();
    let mut state = DaemonState::new(store.clone());
    state.openai_upstream = url;
    let token = state.control_token.clone();
    let app = build_proxy_router(state);
    let body = json!({"model":"test","messages":[{"role":"user","content":"hello"}]}).to_string();
    let (a, b) = tokio::join!(
        app.clone().oneshot(request(&body, Some("s"), &token)),
        app.oneshot(request(&body, Some("s"), &token))
    );
    for response in [a.unwrap(), b.unwrap()] {
        assert_eq!(response.status(), StatusCode::OK);
        to_bytes(response.into_body(), 10000).await.unwrap();
    }
    let turns = store.list_turns("s").unwrap();
    assert_eq!(turns.len(), 2);
    for turn in turns {
        assert_eq!(turn.outcome.as_deref(), Some("completed"));
        assert_eq!(
            turn.observed_usage.as_ref().unwrap()["completion_tokens"],
            3
        );
        assert_eq!(turn.token_usage.cached_tokens, 8);
        assert!(turn.first_body_byte_ms.is_some());
    }
    task.abort();
}
#[tokio::test]
async fn control_requires_auth_and_override_never_receives_engine_key() {
    let captured = Arc::new(Mutex::new(None));
    let copy = captured.clone();
    let (url, task) = mock(Router::new().route(
        "/slots",
        get(move |headers: axum::http::HeaderMap| {
            let copy = copy.clone();
            async move {
                *copy.lock().unwrap() = headers.get("authorization").cloned();
                Json(json!([]))
            }
        }),
    ))
    .await;
    let mut state = DaemonState::new(CwStore::open_in_memory().unwrap());
    state.llamacpp_api_key = Some("fake-key".into());
    let token = state.control_token.clone();
    let app = build_api_router(state);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/sessions")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/engine/slots?engine_url={url}"))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(captured.lock().unwrap().is_none());
    task.abort();
}
#[tokio::test]
async fn preview_does_not_rewrite_actual_turn_and_stale_plan_is_rejected() {
    let store = CwStore::open_in_memory().unwrap();
    let state = DaemonState::new(store.clone());
    let token = state.control_token.clone();
    let mut req = request(
        &json!({"model":"test","messages":[{"role":"user","content":"original"}]}).to_string(),
        Some("s"),
        &token,
    );
    req.headers_mut()
        .insert("x-cw-dry-run", "1".parse().unwrap());
    build_proxy_router(state.clone())
        .oneshot(req)
        .await
        .unwrap();
    let original = store.get_latest_turn("s").unwrap().unwrap();
    let revision = store.session_revision("s").unwrap();
    let api = build_api_router(state);
    let make = || {
        Request::builder().method("POST").uri("/api/v1/sessions/s/mutations").header("authorization",format!("Bearer {token}")).header("if-match",&revision).header("content-type","application/json").body(Body::from(json!({"op":{"op":"pin_context","label":"reminder","content":"important","source_node_id":null},"status":"committed"}).to_string())).unwrap()
    };
    assert_eq!(
        api.clone().oneshot(make()).await.unwrap().status(),
        StatusCode::OK
    );
    assert_eq!(
        api.oneshot(make()).await.unwrap().status(),
        StatusCode::PRECONDITION_FAILED
    );
    let actual = store.get_latest_turn("s").unwrap().unwrap();
    assert_eq!(
        serde_json::to_value(actual).unwrap(),
        serde_json::to_value(original).unwrap()
    );
}
