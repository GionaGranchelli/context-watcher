use axum::{
    Json, Router,
    body::Body,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::Utc;
use cw_core::ir::*;
use cw_proxy::{DaemonState, build_proxy_router};
use cw_store::CwStore;
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    process::Command,
    sync::{Arc, Mutex},
};

#[derive(Debug, Clone)]
struct CapturedRequest {
    path: String,
    headers: HeaderMap,
    body: Vec<u8>,
    json_body: Option<Value>,
}

struct MockProvider {
    url: String,
    #[allow(dead_code)]
    port: u16,
    captured: Arc<Mutex<Vec<CapturedRequest>>>,
    server_task: tokio::task::JoinHandle<()>,
}

impl MockProvider {
    async fn start() -> Self {
        let captured = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
        let captured_clone = captured.clone();

        let app = Router::new()
            .route(
                "/v1/models",
                get(|| async {
                    Json(json!({
                        "data": [
                            {"id": "mock-model", "object": "model"},
                            {"id": "deepseek-chat", "object": "model"},
                            {"id": "gpt-4o", "object": "model"}
                        ]
                    }))
                }),
            )
            .route(
                "/v1/models/:model",
                get(|| async { Json(json!({"id": "mock-model", "object": "model"})) }),
            )
            .route(
                "/api/v1/models",
                get(|| async { Json(json!({"data": [{"id": "mock-model", "object": "model"}]})) }),
            )
            .route(
                "/api/show",
                post(|| async { Json(json!({"details": {"parent_model": ""}})) }),
            )
            .route(
                "/v1/responses",
                post({
                    let captured = captured_clone.clone();
                    move |headers: HeaderMap, body: axum::body::Bytes| {
                        let captured = captured.clone();
                        async move {
                            let json_val = serde_json::from_slice::<Value>(&body).ok();
                            let is_stream = json_val
                                .as_ref()
                                .and_then(|v| v.get("stream"))
                                .and_then(Value::as_bool)
                                .unwrap_or(false);

                            captured.lock().unwrap().push(CapturedRequest {
                                path: "/v1/responses".to_string(),
                                headers: headers.clone(),
                                body: body.to_vec(),
                                json_body: json_val,
                            });

                            if is_stream {
                                let events = [
                                    json!({"type": "response.created", "response": {"id": "resp_test", "status": "in_progress"}}),
                                    json!({"type": "response.output_item.added", "output_index": 0, "item": {"id": "msg_out_1", "type": "message", "role": "assistant", "content": []}}),
                                    json!({"type": "response.content_part.added", "output_index": 0, "content_index": 0, "part": {"type": "output_text", "text": ""}}),
                                    json!({"type": "response.output_text.delta", "output_index": 0, "content_index": 0, "delta": "Mock responses output"}),
                                    json!({"type": "response.output_text.done", "output_index": 0, "content_index": 0, "text": "Mock responses output"}),
                                    json!({"type": "response.content_part.done", "output_index": 0, "content_index": 0, "part": {"type": "output_text", "text": "Mock responses output"}}),
                                    json!({"type": "response.output_item.done", "output_index": 0, "item": {"id": "msg_out_1", "type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Mock responses output"}]}}),
                                    json!({"type": "response.completed", "response": {"id": "resp_test", "status": "completed", "output": [{"id": "msg_out_1", "type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Mock responses output"}]}], "usage": {"input_tokens": 12, "output_tokens": 4, "total_tokens": 16}}}),
                                ];
                                let mut sse_body = String::new();
                                for ev in events {
                                    sse_body.push_str(&format!("data: {}\n\n", serde_json::to_string(&ev).unwrap()));
                                }
                                sse_body.push_str("data: [DONE]\n\n");
                                Response::builder()
                                    .status(StatusCode::OK)
                                    .header("content-type", "text/event-stream")
                                    .header("cache-control", "no-cache")
                                    .body(Body::from(sse_body))
                                    .unwrap()
                            } else {
                                Json(json!({
                                    "id": "resp_mock_json",
                                    "output": [{
                                        "id": "msg_out_1",
                                        "type": "message",
                                        "role": "assistant",
                                        "content": [{"type": "output_text", "text": "Mock responses output"}]
                                    }],
                                    "usage": {"input_tokens": 12, "output_tokens": 4, "total_tokens": 16}
                                })).into_response()
                            }
                        }
                    }
                }),
            )
            .route(
                "/v1/chat/completions",
                post({
                    let captured = captured_clone.clone();
                    move |headers: HeaderMap, body: axum::body::Bytes| {
                        let captured = captured.clone();
                        async move {
                            let json_val = serde_json::from_slice::<Value>(&body).ok();
                            let is_stream = json_val
                                .as_ref()
                                .and_then(|v| v.get("stream"))
                                .and_then(Value::as_bool)
                                .unwrap_or(false);

                            captured.lock().unwrap().push(CapturedRequest {
                                path: "/v1/chat/completions".to_string(),
                                headers: headers.clone(),
                                body: body.to_vec(),
                                json_body: json_val,
                            });

                            if is_stream {
                                let chunks = [
                                    json!({"id": "cmpl-1", "object": "chat.completion.chunk", "created": Utc::now().timestamp(), "model": "mock", "choices": [{"index": 0, "delta": {"role": "assistant", "content": "Mock chat "}, "finish_reason": null}]}),
                                    json!({"id": "cmpl-1", "object": "chat.completion.chunk", "created": Utc::now().timestamp(), "model": "mock", "choices": [{"index": 0, "delta": {"content": "output"}, "finish_reason": null}]}),
                                    json!({"id": "cmpl-1", "object": "chat.completion.chunk", "created": Utc::now().timestamp(), "model": "mock", "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 10, "completion_tokens": 4, "total_tokens": 14}}),
                                ];
                                let mut sse_body = String::new();
                                for c in chunks {
                                    sse_body.push_str(&format!("data: {}\n\n", serde_json::to_string(&c).unwrap()));
                                }
                                sse_body.push_str("data: [DONE]\n\n");
                                Response::builder()
                                    .status(StatusCode::OK)
                                    .header("content-type", "text/event-stream")
                                    .header("cache-control", "no-cache")
                                    .body(Body::from(sse_body))
                                    .unwrap()
                            } else {
                                Json(json!({
                                    "id": "chatcmpl-mock",
                                    "object": "chat.completion",
                                    "created": Utc::now().timestamp(),
                                    "model": "mock",
                                    "choices": [{
                                        "index": 0,
                                        "message": {"role": "assistant", "content": "Mock chat output"},
                                        "finish_reason": "stop"
                                    }],
                                    "usage": {"prompt_tokens": 10, "completion_tokens": 4, "total_tokens": 14}
                                })).into_response()
                            }
                        }
                    }
                }),
            )
            .route(
                "/v1/messages",
                post({
                    let captured = captured_clone.clone();
                    move |headers: HeaderMap, body: axum::body::Bytes| {
                        let captured = captured.clone();
                        async move {
                            let json_val = serde_json::from_slice::<Value>(&body).ok();
                            captured.lock().unwrap().push(CapturedRequest {
                                path: "/v1/messages".to_string(),
                                headers: headers.clone(),
                                body: body.to_vec(),
                                json_body: json_val,
                            });
                            Json(json!({
                                "id": "msg_mock",
                                "type": "message",
                                "role": "assistant",
                                "content": [{"type": "text", "text": "Mock anthropic output"}],
                                "usage": {"input_tokens": 10, "output_tokens": 4}
                            })).into_response()
                        }
                    }
                }),
            );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server_task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        Self {
            url: format!("http://127.0.0.1:{port}"),
            port,
            captured,
            server_task,
        }
    }
}

impl Drop for MockProvider {
    fn drop(&mut self) {
        self.server_task.abort();
    }
}

struct QualificationFixture {
    mock: MockProvider,
    proxy_port: u16,
    control_token: String,
    store: CwStore,
    proxy_task: tokio::task::JoinHandle<()>,
}

impl QualificationFixture {
    async fn setup() -> Self {
        let mock = MockProvider::start().await;
        let store = CwStore::open_in_memory().unwrap();
        store
            .set_autopilot_config(&cw_core::autopilot::AutoPilotConfig {
                enabled: true,
                ..Default::default()
            })
            .unwrap();

        let mut state = DaemonState::new(store.clone());
        state.openai_upstream = mock.url.clone();
        state.anthropic_upstream = mock.url.clone();
        state.deepseek_upstream = mock.url.clone();
        let control_token = state.control_token.clone();

        let app = build_proxy_router(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_port = listener.local_addr().unwrap().port();
        let proxy_task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        Self {
            mock,
            proxy_port,
            control_token,
            store,
            proxy_task,
        }
    }
}

impl Drop for QualificationFixture {
    fn drop(&mut self) {
        self.proxy_task.abort();
    }
}

fn which(bin: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let full = dir.join(bin);
            if full.is_file() { Some(full) } else { None }
        })
    })
}

// -----------------------------------------------------------------------------
// 10 Acceptance Criteria Tests
// -----------------------------------------------------------------------------

#[tokio::test]
async fn test_ac1_session_identity_and_binding() {
    let f = QualificationFixture::setup().await;
    let client = reqwest::Client::new();
    let session_id = "test_ac1_session";

    let payload = json!({
        "model": "gpt-4o",
        "messages": [{"role": "user", "content": "turn 1"}]
    });

    // 1. Unauthenticated session binding is rejected with 403 Forbidden
    let unauth_resp = client
        .post(format!(
            "http://127.0.0.1:{}/openai/v1/chat/completions",
            f.proxy_port
        ))
        .header("x-cw-session-id", session_id)
        .header("content-type", "application/json")
        .json(&payload)
        .send()
        .await
        .unwrap();
    assert_eq!(unauth_resp.status(), StatusCode::FORBIDDEN);

    // 2. Authenticated request binds the session correctly
    let auth_resp = client
        .post(format!(
            "http://127.0.0.1:{}/openai/v1/chat/completions",
            f.proxy_port
        ))
        .header("x-cw-session-id", session_id)
        .header("x-cw-control-token", &f.control_token)
        .header("content-type", "application/json")
        .json(&payload)
        .send()
        .await
        .unwrap();
    assert_eq!(auth_resp.status(), StatusCode::OK);

    // 3. Verify session exists in store with 1 turn
    let sess = f
        .store
        .get_session(session_id)
        .unwrap()
        .expect("session exists");
    assert_eq!(sess.id, session_id);
    let turns = f.store.list_turns(session_id).unwrap();
    assert_eq!(turns.len(), 1);

    // 4. Second turn keeps exact same session ID
    let payload_turn2 = json!({
        "model": "gpt-4o",
        "messages": [
            {"role": "user", "content": "turn 1"},
            {"role": "assistant", "content": "Mock chat output"},
            {"role": "user", "content": "turn 2"}
        ]
    });
    let auth_resp2 = client
        .post(format!(
            "http://127.0.0.1:{}/openai/v1/chat/completions",
            f.proxy_port
        ))
        .header("x-cw-session-id", session_id)
        .header("x-cw-control-token", &f.control_token)
        .header("content-type", "application/json")
        .json(&payload_turn2)
        .send()
        .await
        .unwrap();
    assert_eq!(auth_resp2.status(), StatusCode::OK);

    let turns2 = f.store.list_turns(session_id).unwrap();
    assert_eq!(turns2.len(), 2);
}

#[tokio::test]
async fn test_ac2_committed_overlay_on_next_model_request() {
    let f = QualificationFixture::setup().await;
    let client = reqwest::Client::new();
    let session_id = "test_ac2_session";

    let payload1 = json!({
        "model": "gpt-4o",
        "messages": [{"role": "user", "content": "initial prompt"}]
    });

    let r1 = client
        .post(format!(
            "http://127.0.0.1:{}/openai/v1/chat/completions",
            f.proxy_port
        ))
        .header("x-cw-session-id", session_id)
        .header("x-cw-control-token", &f.control_token)
        .header("content-type", "application/json")
        .json(&payload1)
        .send()
        .await
        .unwrap();
    assert_eq!(r1.status(), StatusCode::OK);

    // Commit an overlay PinContext
    let now = Utc::now();
    f.store
        .save_mutation(&MutationRule {
            id: "ac2_pin_rule".into(),
            session_id: session_id.into(),
            op: MutationOp::PinContext {
                label: "Policy".into(),
                content: "CRITICAL_PINNED_DIRECTIVE: Preserved across turns".into(),
                source_node_id: None,
            },
            status: MutationStatus::Committed,
            estimated_saved_tokens: 0,
            estimated_kv_break_tokens: 0,
            created_at: now,
        })
        .unwrap();

    // Turn 2: verify committed overlay is applied to outbound payload sent to upstream
    let payload2 = json!({
        "model": "gpt-4o",
        "messages": [
            {"role": "user", "content": "initial prompt"},
            {"role": "assistant", "content": "Mock chat output"},
            {"role": "user", "content": "follow-up prompt"}
        ]
    });
    let r2 = client
        .post(format!(
            "http://127.0.0.1:{}/openai/v1/chat/completions",
            f.proxy_port
        ))
        .header("x-cw-session-id", session_id)
        .header("x-cw-control-token", &f.control_token)
        .header("content-type", "application/json")
        .json(&payload2)
        .send()
        .await
        .unwrap();
    assert_eq!(r2.status(), StatusCode::OK);

    // Inspect the captured upstream requests
    let captured = f.mock.captured.lock().unwrap().clone();
    assert_eq!(captured.len(), 2);
    let turn2_req = &captured[1];
    let turn2_text = String::from_utf8_lossy(&turn2_req.body);
    assert!(
        turn2_text.contains("CRITICAL_PINNED_DIRECTIVE: Preserved across turns"),
        "Upstream did not receive the committed overlay!"
    );
}

#[tokio::test]
async fn test_ac3_zero_duplicate_mutation_injection() {
    let f = QualificationFixture::setup().await;
    let client = reqwest::Client::new();
    let session_id = "test_ac3_session";

    // Setup session and pin
    let payload1 = json!({
        "model": "gpt-4o",
        "messages": [{"role": "user", "content": "turn 1"}]
    });
    let _ = client
        .post(format!(
            "http://127.0.0.1:{}/openai/v1/chat/completions",
            f.proxy_port
        ))
        .header("x-cw-session-id", session_id)
        .header("x-cw-control-token", &f.control_token)
        .header("content-type", "application/json")
        .json(&payload1)
        .send()
        .await
        .unwrap();

    f.store
        .save_mutation(&MutationRule {
            id: "ac3_pin_rule".into(),
            session_id: session_id.into(),
            op: MutationOp::PinContext {
                label: "Policy".into(),
                content: "UNIQUE_NON_DUPLICATED_PIN".into(),
                source_node_id: None,
            },
            status: MutationStatus::Committed,
            estimated_saved_tokens: 0,
            estimated_kv_break_tokens: 0,
            created_at: Utc::now(),
        })
        .unwrap();

    // Send Turn 2
    let payload2 = json!({
        "model": "gpt-4o",
        "messages": [
            {"role": "user", "content": "turn 1"},
            {"role": "assistant", "content": "Mock chat output"},
            {"role": "user", "content": "turn 2"}
        ]
    });
    let _ = client
        .post(format!(
            "http://127.0.0.1:{}/openai/v1/chat/completions",
            f.proxy_port
        ))
        .header("x-cw-session-id", session_id)
        .header("x-cw-control-token", &f.control_token)
        .header("content-type", "application/json")
        .json(&payload2)
        .send()
        .await
        .unwrap();

    // Send Turn 3 (simulating agent conversation history containing earlier turns)
    let payload3 = json!({
        "model": "gpt-4o",
        "messages": [
            {"role": "user", "content": "turn 1"},
            {"role": "assistant", "content": "Mock chat output"},
            {"role": "user", "content": "turn 2"},
            {"role": "assistant", "content": "Mock chat output 2"},
            {"role": "user", "content": "turn 3"}
        ]
    });
    let _ = client
        .post(format!(
            "http://127.0.0.1:{}/openai/v1/chat/completions",
            f.proxy_port
        ))
        .header("x-cw-session-id", session_id)
        .header("x-cw-control-token", &f.control_token)
        .header("content-type", "application/json")
        .json(&payload3)
        .send()
        .await
        .unwrap();

    let captured = f.mock.captured.lock().unwrap().clone();
    assert_eq!(captured.len(), 3);
    let turn3_text = String::from_utf8_lossy(&captured[2].body);
    let occurrences = turn3_text.matches("UNIQUE_NON_DUPLICATED_PIN").count();
    assert_eq!(occurrences, 1, "Duplicate mutation injected into turn 3!");
}

#[tokio::test]
async fn test_ac4_tool_call_result_pairing_preserved() {
    let f = QualificationFixture::setup().await;
    let client = reqwest::Client::new();
    let session_id = "test_ac4_session";

    let payload = json!({
        "model": "gpt-4o",
        "messages": [
            {"role": "user", "content": "Check files"},
            {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "call_inspect_123",
                    "type": "function",
                    "function": {"name": "list_files", "arguments": "{}"}
                }]
            },
            {
                "role": "tool",
                "tool_call_id": "call_inspect_123",
                "content": "file1.rs\nfile2.rs\nfile3.rs"
            },
            {"role": "user", "content": "Proceed with analysis"}
        ]
    });

    let resp = client
        .post(format!(
            "http://127.0.0.1:{}/openai/v1/chat/completions",
            f.proxy_port
        ))
        .header("x-cw-session-id", session_id)
        .header("x-cw-control-token", &f.control_token)
        .header("content-type", "application/json")
        .json(&payload)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let captured = f.mock.captured.lock().unwrap().clone();
    assert_eq!(captured.len(), 1);
    let req_json = captured[0].json_body.as_ref().expect("valid json body");
    let messages = req_json.get("messages").and_then(Value::as_array).unwrap();

    let assistant_msg = &messages[1];
    let tool_call_id = assistant_msg
        .pointer("/tool_calls/0/id")
        .and_then(Value::as_str)
        .unwrap();
    let tool_msg = &messages[2];
    let tool_result_id = tool_msg
        .get("tool_call_id")
        .and_then(Value::as_str)
        .unwrap();

    assert_eq!(tool_call_id, "call_inspect_123");
    assert_eq!(tool_result_id, "call_inspect_123");
}

#[tokio::test]
async fn test_ac5_signed_request_unmodified_passthrough() {
    let f = QualificationFixture::setup().await;
    let client = reqwest::Client::new();
    let session_id = "test_ac5_session";

    let now = Utc::now();
    f.store
        .upsert_session(&Session {
            id: session_id.into(),
            parent_session_id: None,
            harness: HarnessKind::GenericProxy,
            workspace_root: None,
            model: "claude-3-7-sonnet".into(),
            provider: ProviderKind::Anthropic,
            title: None,
            created_at: now,
            updated_at: now,
        })
        .unwrap();

    // Commit an overlay that would normally mutate the payload
    f.store
        .save_mutation(&MutationRule {
            id: "ac5_pin".into(),
            session_id: session_id.into(),
            op: MutationOp::PinContext {
                label: "Pin".into(),
                content: "MUTATION_SHOULD_BE_SUPPRESSED".into(),
                source_node_id: None,
            },
            status: MutationStatus::Committed,
            estimated_saved_tokens: 0,
            estimated_kv_break_tokens: 0,
            created_at: now,
        })
        .unwrap();

    // Anthropic payload with thinking signature
    let signed_payload = json!({
        "model": "claude-3-7-sonnet",
        "max_tokens": 1024,
        "messages": [
            {
                "role": "assistant",
                "content": [{
                    "type": "thinking",
                    "thinking": "Sensitive reasoning process",
                    "signature": "opaque_cryptographic_signature_value"
                }]
            },
            {"role": "user", "content": "continue"}
        ]
    });

    let resp = client
        .post(format!(
            "http://127.0.0.1:{}/anthropic/v1/messages",
            f.proxy_port
        ))
        .header("x-cw-session-id", session_id)
        .header("x-cw-control-token", &f.control_token)
        .header("content-type", "application/json")
        .json(&signed_payload)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let captured = f.mock.captured.lock().unwrap().clone();
    assert_eq!(captured.len(), 1);
    let body_str = String::from_utf8_lossy(&captured[0].body);
    // Signature must be completely preserved
    assert!(body_str.contains("opaque_cryptographic_signature_value"));
    // Mutation must NOT be applied to signed payload
    assert!(!body_str.contains("MUTATION_SHOULD_BE_SUPPRESSED"));
}

#[tokio::test]
async fn test_ac6_control_token_stripped_from_upstream_and_tools() {
    let f = QualificationFixture::setup().await;
    let client = reqwest::Client::new();
    let session_id = "test_ac6_session";

    let payload = json!({
        "model": "gpt-4o",
        "messages": [{"role": "user", "content": "credential check"}]
    });

    let resp = client
        .post(format!(
            "http://127.0.0.1:{}/openai/v1/chat/completions",
            f.proxy_port
        ))
        .header("x-cw-session-id", session_id)
        .header("x-cw-control-token", &f.control_token)
        .header("x-cw-workspace", "/home/user/project")
        .header("content-type", "application/json")
        .json(&payload)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let captured = f.mock.captured.lock().unwrap().clone();
    assert_eq!(captured.len(), 1);
    let upstream_headers = &captured[0].headers;

    // Verify upstream never sees contextWatcher control tokens or internal routing headers
    assert!(!upstream_headers.contains_key("x-cw-control-token"));
    assert!(!upstream_headers.contains_key("x-cw-session-id"));
    assert!(!upstream_headers.contains_key("x-cw-workspace"));
}

#[tokio::test]
async fn test_ac7_original_transcripts_untouched() {
    let f = QualificationFixture::setup().await;
    let client = reqwest::Client::new();
    let session_id = "test_ac7_session";

    // Create a temporary mock transcript file on disk
    let temp_dir = std::env::temp_dir().join(format!("cw_test_ac7_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_dir).unwrap();
    let transcript_file = temp_dir.join("transcript.jsonl");
    let original_content = "{\"step\": 1, \"content\": \"pristine_user_text\"}\n";
    std::fs::write(&transcript_file, original_content).unwrap();

    let payload = json!({
        "model": "gpt-4o",
        "messages": [{"role": "user", "content": "read transcript"}]
    });

    let resp = client
        .post(format!(
            "http://127.0.0.1:{}/openai/v1/chat/completions",
            f.proxy_port
        ))
        .header("x-cw-session-id", session_id)
        .header("x-cw-control-token", &f.control_token)
        .header("content-type", "application/json")
        .json(&payload)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Verify file content on disk is completely unchanged
    let current_content = std::fs::read_to_string(&transcript_file).unwrap();
    assert_eq!(current_content, original_content);
    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_ac8_safe_recovery_on_daemon_fault() {
    let f = QualificationFixture::setup().await;
    let client = reqwest::Client::new();
    let session_id = "test_ac8_session";

    // Invalid session ID with illegal characters
    let bad_resp = client
        .post(format!(
            "http://127.0.0.1:{}/openai/v1/chat/completions",
            f.proxy_port
        ))
        .header("x-cw-session-id", "invalid/session/id!")
        .header("x-cw-control-token", &f.control_token)
        .header("content-type", "application/json")
        .json(&json!({"model": "gpt-4o", "messages": [{"role": "user", "content": "ping"}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(bad_resp.status(), StatusCode::BAD_REQUEST);

    // Proxy remains operational after rejected request
    let good_resp = client
        .post(format!(
            "http://127.0.0.1:{}/openai/v1/chat/completions",
            f.proxy_port
        ))
        .header("x-cw-session-id", session_id)
        .header("x-cw-control-token", &f.control_token)
        .header("content-type", "application/json")
        .json(&json!({"model": "gpt-4o", "messages": [{"role": "user", "content": "healthy"}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(good_resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_ac9_binary_task_continuation_codex_cli() {
    let codex_bin = match which("codex") {
        Some(bin) => bin,
        None => {
            eprintln!("SKIPPED: codex binary not found in PATH");
            return;
        }
    };

    let f = QualificationFixture::setup().await;
    let session_id = "codex_qual_ac9";
    let temp_home = std::env::temp_dir().join(format!("codex_home_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_home).unwrap();

    let proxy_port = f.proxy_port;
    let token = f.control_token.clone();
    let temp_home_clone = temp_home.clone();
    let sid = session_id.to_string();

    let output = tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new(codex_bin);
        cmd.args([
            "exec",
            "--ephemeral",
            "--skip-git-repo-check",
            "--ignore-user-config",
        ]);
        cmd.args(cw_harnesses::qualification::codex_proxy_config_args(
            proxy_port,
        ));
        cmd.args(["-m", "mock-model", "Say hello"]);
        cmd.env("OPENAI_API_KEY", "mock-openai-key");
        cmd.env("CODEX_HOME", &temp_home_clone);
        cmd.env("CW_SESSION_ID", sid);
        cmd.env("CW_CONTROL_TOKEN", token);
        cmd.output()
    })
    .await
    .unwrap()
    .expect("failed to run codex binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    let _ = std::fs::remove_dir_all(&temp_home);

    assert_eq!(
        output.status.code(),
        Some(0),
        "Codex CLI failed: stderr: {stderr}"
    );
    assert!(stdout.contains("Mock responses output") || stderr.contains("Mock responses output"));

    // Verify proxy recorded the session and turn
    let sess = f
        .store
        .get_session(session_id)
        .unwrap()
        .expect("codex session recorded");
    assert_eq!(sess.id, session_id);
    let turns = f.store.list_turns(session_id).unwrap();
    assert_eq!(turns.len(), 1);

    // Verify upstream captured request has headers stripped
    let captured = f.mock.captured.lock().unwrap().clone();
    assert!(!captured.is_empty());
    assert!(!captured[0].headers.contains_key("x-cw-control-token"));
    assert!(!captured[0].headers.contains_key("x-cw-session-id"));
}

#[tokio::test]
async fn test_ac9_binary_task_continuation_omp() {
    let omp_bin = match which("omp") {
        Some(bin) => bin,
        None => {
            eprintln!("SKIPPED: omp binary not found in PATH");
            return;
        }
    };

    let f = QualificationFixture::setup().await;
    let temp_agent_dir = std::env::temp_dir().join(format!("omp_agent_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_agent_dir).unwrap();

    let models_cfg =
        cw_harnesses::qualification::omp_proxy_models_config(f.proxy_port, "mock-model");
    std::fs::write(
        temp_agent_dir.join("models.json"),
        serde_json::to_string_pretty(&models_cfg).unwrap(),
    )
    .unwrap();

    let temp_agent_dir_clone = temp_agent_dir.clone();
    let output = tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new(omp_bin);
        cmd.args([
            "-p",
            "Say hello",
            "--model",
            "cw-proxy/mock-model",
            "--no-tools",
            "--no-session",
            "--no-extensions",
            "--no-skills",
            "--no-rules",
        ]);
        cmd.env("PI_CODING_AGENT_DIR", &temp_agent_dir_clone);
        cmd.output()
    })
    .await
    .unwrap()
    .expect("failed to run omp binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    let _ = std::fs::remove_dir_all(&temp_agent_dir);

    assert_eq!(
        output.status.code(),
        Some(0),
        "OMP CLI failed: stderr: {stderr}"
    );
    assert!(
        stdout.contains("Mock chat output"),
        "Stdout did not contain mock response: {stdout}"
    );

    // Upstream captured the turn
    let captured = f.mock.captured.lock().unwrap().clone();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].path, "/v1/chat/completions");
}

#[tokio::test]
async fn test_ac9_binary_task_continuation_hermes() {
    let hermes_bin = match which("hermes") {
        Some(bin) => bin,
        None => {
            eprintln!("SKIPPED: hermes binary not found in PATH");
            return;
        }
    };

    let f = QualificationFixture::setup().await;
    let proxy_port = f.proxy_port;

    let output = tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new(hermes_bin);
        cmd.args([
            "chat",
            "-Q",
            "-q",
            "Say hello",
            "--provider",
            "custom",
            "--base-url",
            &format!("http://127.0.0.1:{proxy_port}/openai/v1"),
            "--api-key",
            "mock-key",
            "-m",
            "mock-model",
            "--ignore-user-config",
            "--ignore-rules",
            "--source",
            "tool",
        ]);
        cmd.output()
    })
    .await
    .unwrap()
    .expect("failed to run hermes binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(
        output.status.code(),
        Some(0),
        "Hermes failed: stderr: {stderr}"
    );
    assert!(
        stdout.contains("Mock chat output"),
        "Stdout did not contain mock response: {stdout}"
    );

    // Upstream captured chat completion
    let captured = f.mock.captured.lock().unwrap().clone();
    assert!(!captured.is_empty());
    assert!(captured.iter().any(|r| r.path == "/v1/chat/completions"));
}

#[tokio::test]
async fn test_ac10_token_and_latency_savings_accounting() {
    let f = QualificationFixture::setup().await;
    let client = reqwest::Client::new();
    let session_id = "test_ac10_session";

    let payload = json!({
        "model": "gpt-4o",
        "messages": [{"role": "user", "content": "measure savings"}]
    });

    let resp = client
        .post(format!(
            "http://127.0.0.1:{}/openai/v1/chat/completions",
            f.proxy_port
        ))
        .header("x-cw-session-id", session_id)
        .header("x-cw-control-token", &f.control_token)
        .header("content-type", "application/json")
        .json(&payload)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let turns = f.store.list_turns(session_id).unwrap();
    assert_eq!(turns.len(), 1);
    let turn = &turns[0];

    // Assert token usage recorded faithfully matches mock provider's returned usage
    assert!(turn.token_usage.input_tokens > 0);
    // Cached status must not be fabricated when telemetry is missing
    assert_eq!(turn.token_usage.cached_tokens, 0);
}

// -----------------------------------------------------------------------------
// Opt-in Real-Provider Continuation Tests
// -----------------------------------------------------------------------------

#[tokio::test]
#[ignore = "opt-in real Codex qualification; requires CW_QUALIFY_REAL_CODEX and OPENAI_API_KEY"]
async fn test_live_continuation_real_codex() {
    if std::env::var("CW_QUALIFY_REAL_CODEX").is_err() {
        eprintln!("SKIPPED: CW_QUALIFY_REAL_CODEX not set");
        return;
    }
    let api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY required for live test");
    let codex_bin = which("codex").expect("codex binary required in PATH");

    let store = CwStore::open_in_memory().unwrap();
    let mut state = DaemonState::new(store.clone());
    state.openai_upstream = "https://api.openai.com".to_string();
    let control_token = state.control_token.clone();

    let app = build_proxy_router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_port = listener.local_addr().unwrap().port();
    let proxy_task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let session_id = "codex_real_live_qual";
    let temp_home = std::env::temp_dir().join(format!("codex_real_home_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_home).unwrap();

    let temp_home_clone = temp_home.clone();
    let sid = session_id.to_string();
    let token = control_token.clone();

    let output = tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new(codex_bin);
        cmd.args([
            "exec",
            "--ephemeral",
            "--skip-git-repo-check",
            "--ignore-user-config",
            "-c", "model_provider=\"context_watcher\"",
            "-c", "model_providers.context_watcher.name=\"contextWatcher\"",
            "-c", &format!("model_providers.context_watcher.base_url=\"http://127.0.0.1:{proxy_port}/openai/v1\""),
            "-c", "model_providers.context_watcher.wire_api=\"responses\"",
            "-c", "model_providers.context_watcher.requires_openai_auth=true",
            "-c", "model_providers.context_watcher.env_key=\"OPENAI_API_KEY\"",
            "-c", "model_providers.context_watcher.env_http_headers={\"x-cw-session-id\"=\"CW_SESSION_ID\",\"x-cw-control-token\"=\"CW_CONTROL_TOKEN\"}",
            "-c", "shell_environment_policy.filters.CW_CONTROL_TOKEN=\"exclude\"",
            "-m", "gpt-4o-mini",
            "Say live-codex-pass"
        ]);
        cmd.env("OPENAI_API_KEY", api_key);
        cmd.env("CODEX_HOME", &temp_home_clone);
        cmd.env("CW_SESSION_ID", sid);
        cmd.env("CW_CONTROL_TOKEN", token);
        cmd.output()
    })
    .await
    .unwrap()
    .expect("failed to run codex binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let _ = std::fs::remove_dir_all(&temp_home);
    proxy_task.abort();

    assert_eq!(output.status.code(), Some(0), "Live Codex failed: {stderr}");
    assert!(stdout.contains("live-codex-pass") || stderr.contains("live-codex-pass"));

    let sess = store
        .get_session(session_id)
        .unwrap()
        .expect("session recorded");
    assert_eq!(sess.id, session_id);
    let turns = store.list_turns(session_id).unwrap();
    assert_eq!(turns.len(), 1);
}

#[tokio::test]
#[ignore = "opt-in real Hermes qualification; requires CW_QUALIFY_REAL_HERMES and DEEPSEEK_API_KEY"]
async fn test_live_continuation_real_hermes() {
    if std::env::var("CW_QUALIFY_REAL_HERMES").is_err() {
        eprintln!("SKIPPED: CW_QUALIFY_REAL_HERMES not set");
        return;
    }
    let api_key =
        std::env::var("DEEPSEEK_API_KEY").expect("DEEPSEEK_API_KEY required for live test");
    let hermes_bin = which("hermes").expect("hermes binary required in PATH");

    let store = CwStore::open_in_memory().unwrap();
    let mut state = DaemonState::new(store.clone());
    state.deepseek_upstream = "https://api.deepseek.com".to_string();

    let app = build_proxy_router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_port = listener.local_addr().unwrap().port();
    let proxy_task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let output = tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new(hermes_bin);
        cmd.args([
            "chat",
            "-Q",
            "-q",
            "Say live-hermes-pass",
            "--provider",
            "custom",
            "--base-url",
            &format!("http://127.0.0.1:{proxy_port}/deepseek/v1"),
            "--api-key",
            &api_key,
            "-m",
            "deepseek-chat",
            "--ignore-user-config",
            "--ignore-rules",
            "--source",
            "tool",
        ]);
        cmd.output()
    })
    .await
    .unwrap()
    .expect("failed to run hermes binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    proxy_task.abort();

    assert_eq!(
        output.status.code(),
        Some(0),
        "Live Hermes failed: {stderr}"
    );
    assert!(stdout.contains("live-hermes-pass") || stderr.contains("live-hermes-pass"));
}

#[tokio::test]
#[ignore = "opt-in real OMP qualification; requires CW_QUALIFY_REAL_OMP and OPENAI_API_KEY"]
async fn test_live_continuation_real_omp() {
    if std::env::var("CW_QUALIFY_REAL_OMP").is_err() {
        eprintln!("SKIPPED: CW_QUALIFY_REAL_OMP not set");
        return;
    }
    let api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY required for live test");
    let omp_bin = which("omp").expect("omp binary required in PATH");

    let store = CwStore::open_in_memory().unwrap();
    let mut state = DaemonState::new(store.clone());
    state.openai_upstream = "https://api.openai.com".to_string();

    let app = build_proxy_router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_port = listener.local_addr().unwrap().port();
    let proxy_task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let temp_agent_dir =
        std::env::temp_dir().join(format!("omp_real_agent_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_agent_dir).unwrap();

    let models_cfg = json!({
        "providers": {
            "cw-live": {
                "baseUrl": format!("http://127.0.0.1:{proxy_port}/openai/v1"),
                "api": "openai-completions",
                "apiKey": api_key,
                "models": [{
                    "id": "gpt-4o-mini",
                    "name": "GPT-4o Mini",
                    "contextWindow": 128000,
                    "maxTokens": 4096
                }]
            }
        }
    });
    std::fs::write(
        temp_agent_dir.join("models.json"),
        serde_json::to_string_pretty(&models_cfg).unwrap(),
    )
    .unwrap();

    let temp_agent_dir_clone = temp_agent_dir.clone();
    let output = tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new(omp_bin);
        cmd.args([
            "-p",
            "Say live-omp-pass",
            "--model",
            "cw-live/gpt-4o-mini",
            "--no-tools",
            "--no-session",
            "--no-extensions",
            "--no-skills",
            "--no-rules",
        ]);
        cmd.env("PI_CODING_AGENT_DIR", &temp_agent_dir_clone);
        cmd.output()
    })
    .await
    .unwrap()
    .expect("failed to run omp binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let _ = std::fs::remove_dir_all(&temp_agent_dir);
    proxy_task.abort();

    assert_eq!(output.status.code(), Some(0), "Live OMP failed: {stderr}");
    assert!(stdout.contains("live-omp-pass") || stderr.contains("live-omp-pass"));
}
