use axum::{
    Json, Router,
    body::Body,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::Utc;
use cw_core::{ir::*, validator::validate_protocol_invariants};
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
                "/models",
                get(|| async {
                    Json(json!({
                        "data": [
                            {"id": "mock-model", "object": "model"}
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
                "/deepseek/api/show",
                post(|| async { Json(json!({"details": {"parent_model": ""}})) }),
            )
            .route(
                "/deepseek/api/v1/models",
                get(|| async { Json(json!({"data": [{"id": "mock-model", "object": "model"}]})) }),
            )
            .route(
                "/deepseek/v1/models",
                get(|| async { Json(json!({"data": [{"id": "mock-model", "object": "model"}]})) }),
            )
            .route(
                "/deepseek/v1/models/:model",
                get(|| async { Json(json!({"id": "mock-model", "object": "model"})) }),
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
                            handle_mock_chat_completion(headers, body, "/v1/chat/completions", captured).await
                        }
                    }
                }),
            )
            .route(
                "/chat/completions",
                post({
                    let captured = captured_clone.clone();
                    move |headers: HeaderMap, body: axum::body::Bytes| {
                        let captured = captured.clone();
                        async move {
                            handle_mock_chat_completion(headers, body, "/chat/completions", captured).await
                        }
                    }
                }),
            )
            .route(
                "/deepseek/chat/completions",
                post({
                    let captured = captured_clone.clone();
                    move |headers: HeaderMap, body: axum::body::Bytes| {
                        let captured = captured.clone();
                        async move {
                            handle_mock_chat_completion(headers, body, "/deepseek/chat/completions", captured).await
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

async fn handle_mock_chat_completion(
    headers: HeaderMap,
    body: axum::body::Bytes,
    path: &str,
    captured: Arc<Mutex<Vec<CapturedRequest>>>,
) -> Response {
    let json_val = serde_json::from_slice::<Value>(&body).ok();
    let is_stream = json_val
        .as_ref()
        .and_then(|v| v.get("stream"))
        .and_then(Value::as_bool)
        .unwrap_or(false);

    captured.lock().unwrap().push(CapturedRequest {
        path: path.to_string(),
        headers: headers.clone(),
        body: body.to_vec(),
        json_body: json_val,
    });

    if is_stream {
        let chunks = [
            json!({"id": "cmpl-1", "object": "chat.completion.chunk", "created": Utc::now().timestamp(), "model": "mock", "choices": [{"index": 0, "delta": {"role": "assistant", "content": "Mock chat "}, "finish_reason": null}]}),
            json!({"id": "cmpl-1", "object": "chat.completion.chunk", "created": Utc::now().timestamp(), "model": "mock", "choices": [{"index": 0, "delta": {"content": "output"}, "finish_reason": null}]}),
            json!({"id": "cmpl-1", "object": "chat.completion.chunk", "created": Utc::now().timestamp(), "model": "mock", "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 1250, "completion_tokens": 180, "total_tokens": 1430}}),
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
            "usage": {"prompt_tokens": 1250, "completion_tokens": 180, "total_tokens": 1430}
        }))
        .into_response()
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
        if let Ok(tok) = cw_core::control::control_token(false) {
            state.control_token = tok;
        }
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

fn require_installed_binary(bin: &str) -> PathBuf {
    which(bin).unwrap_or_else(|| {
        panic!(
            "Required harness binary '{bin}' was not found in PATH. \
             Ensure '{bin}' is installed before running this harness qualification test."
        );
    })
}

// =============================================================================
// Suite A: Core Proxy & Protocol Contract Tests (Portable / Runs in CI)
// =============================================================================

#[tokio::test]
async fn test_core_contract_ac1_session_identity_and_binding() {
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
async fn test_core_contract_ac2_committed_overlay_on_next_model_request() {
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
async fn test_core_contract_ac2_context_transform_api() {
    let f = QualificationFixture::setup().await;
    let client = reqwest::Client::new();
    let session_id = "test_ac2_transform_session";

    let sess = Session {
        id: session_id.to_string(),
        parent_session_id: None,
        harness: HarnessKind::Omp,
        workspace_root: None,
        model: "mock-model".to_string(),
        provider: ProviderKind::OpenAI,
        title: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };
    f.store.upsert_session(&sess).unwrap();

    let pin = MutationRule {
        id: "mut_omp_pin_01".to_string(),
        session_id: session_id.to_string(),
        op: MutationOp::PinContext {
            label: "CRITICAL_OMP_PINNED_POLICY".to_string(),
            content: "Zero hallucination context hook active".to_string(),
            source_node_id: None,
        },
        status: MutationStatus::Committed,
        estimated_saved_tokens: 0,
        estimated_kv_break_tokens: 0,
        created_at: Utc::now(),
    };
    f.store
        .save_mutations_at_revision(session_id, None, &[pin])
        .unwrap();

    let transform_url = format!(
        "http://127.0.0.1:{}/api/v1/sessions/{session_id}/transform",
        f.proxy_port
    );

    // 1. Authorized transform request succeeds
    let req_body = json!({
        "messages": [
            {"role": "user", "content": "Hello prompt"}
        ],
        "provider": "openai"
    });
    let res = client
        .post(&transform_url)
        .header("x-cw-control-token", &f.control_token)
        .header("content-type", "application/json")
        .json(&req_body)
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["status"], "ok");
    let msgs = body["messages"].as_array().expect("messages must be array");
    let msgs_str = serde_json::to_string(msgs).unwrap();
    assert!(msgs_str.contains("CRITICAL_OMP_PINNED_POLICY"));
    assert!(msgs_str.contains("Zero hallucination context hook active"));

    // 2. Unauthorized request is rejected
    let res_unauth = client
        .post(&transform_url)
        .header("x-cw-control-token", "invalid_token_12345678901234567890")
        .header("content-type", "application/json")
        .json(&req_body)
        .send()
        .await
        .unwrap();
    assert_eq!(res_unauth.status(), StatusCode::FORBIDDEN);

    // 3. Bad request (messages is not an array)
    let res_bad = client
        .post(&transform_url)
        .header("x-cw-control-token", &f.control_token)
        .header("content-type", "application/json")
        .json(&json!({"messages": "not an array"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res_bad.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_core_contract_ac3_zero_duplicate_mutation_injection() {
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
async fn test_core_contract_ac4_tool_call_result_pairing_preserved_under_active_mutation() {
    let f = QualificationFixture::setup().await;
    let client = reqwest::Client::new();
    let session_id = "test_ac4_pairing_session";

    f.store
        .upsert_session(&Session {
            id: session_id.into(),
            parent_session_id: None,
            harness: HarnessKind::GenericProxy,
            workspace_root: Some("/tmp".into()),
            model: "gpt-4o".into(),
            provider: ProviderKind::OpenAI,
            title: Some("ac4 session".into()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        })
        .unwrap();

    // Commit an active mutation rule targeting turn 1
    f.store
        .save_mutation(&MutationRule {
            id: "ac4_tail_pin".into(),
            session_id: session_id.into(),
            op: MutationOp::PinContext {
                label: "Safety".into(),
                content: "ENFORCE_TOOL_INTEGRITY".into(),
                source_node_id: None,
            },
            status: MutationStatus::Committed,
            estimated_saved_tokens: 0,
            estimated_kv_break_tokens: 0,
            created_at: Utc::now(),
        })
        .unwrap();

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

    // 1. Invariant validator must strictly accept the mutated outbound payload
    assert_eq!(
        validate_protocol_invariants(req_json, ProviderKind::OpenAI),
        Ok(())
    );

    // 2. Outbound JSON must retain identical 1:1 call_id <-> tool_call_id matching
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

    // 3. PinContext overlay must also be present without breaking pairing
    let last_user_content = messages
        .last()
        .unwrap()
        .get("content")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(last_user_content.contains("ENFORCE_TOOL_INTEGRITY"));
}

#[tokio::test]
async fn test_core_contract_ac5_signed_request_byte_for_byte_fidelity() {
    let f = QualificationFixture::setup().await;
    let client = reqwest::Client::new();
    let session_id = "test_ac5_signed_session";

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

    // Commit a mutation rule that would ordinarily modify text
    f.store
        .save_mutation(&MutationRule {
            id: "ac5_pin".into(),
            session_id: session_id.into(),
            op: MutationOp::PinContext {
                label: "Pin".into(),
                content: "MUTATION_MUST_BE_SUPPRESSED_ON_SIGNED".into(),
                source_node_id: None,
            },
            status: MutationStatus::Committed,
            estimated_saved_tokens: 0,
            estimated_kv_break_tokens: 0,
            created_at: now,
        })
        .unwrap();

    // Anthropic payload with cryptographic thinking signature
    let signed_payload = json!({
        "model": "claude-3-7-sonnet",
        "max_tokens": 1024,
        "messages": [
            {
                "role": "assistant",
                "content": [{
                    "type": "thinking",
                    "thinking": "Cryptographic reasoning trace",
                    "signature": "opaque_cryptographic_signature_value_ed25519"
                }]
            },
            {"role": "user", "content": "continue"}
        ]
    });
    let raw_input_bytes = serde_json::to_vec(&signed_payload).unwrap();

    let resp = client
        .post(format!(
            "http://127.0.0.1:{}/anthropic/v1/messages",
            f.proxy_port
        ))
        .header("x-cw-session-id", session_id)
        .header("x-cw-control-token", &f.control_token)
        .header("content-type", "application/json")
        .body(raw_input_bytes.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let captured = f.mock.captured.lock().unwrap().clone();
    assert_eq!(captured.len(), 1);

    // Assert exact byte-for-byte and semantic JSON equality
    assert_eq!(
        captured[0].body, raw_input_bytes,
        "Signed payload bytes modified in transit!"
    );
    assert_eq!(
        captured[0].json_body.as_ref().unwrap(),
        &signed_payload,
        "Signed payload JSON structure modified in transit!"
    );
    assert!(
        !String::from_utf8_lossy(&captured[0].body)
            .contains("MUTATION_MUST_BE_SUPPRESSED_ON_SIGNED")
    );
}

#[tokio::test]
async fn test_core_contract_ac6_control_token_stripped_and_tool_sandboxing() {
    let f = QualificationFixture::setup().await;
    let client = reqwest::Client::new();
    let session_id = "test_ac6_token_session";

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

    // 1. Verify upstream never receives contextWatcher internal headers
    assert!(!upstream_headers.contains_key("x-cw-control-token"));
    assert!(!upstream_headers.contains_key("x-cw-session-id"));
    assert!(!upstream_headers.contains_key("x-cw-workspace"));

    // 2. Verify Codex CLI configuration helper enforces shell policy exclusion
    let codex_args = cw_harnesses::qualification::codex_proxy_config_args(f.proxy_port);
    let has_exclusion_filter = codex_args
        .iter()
        .any(|arg| arg.contains("shell_environment_policy.filters.CW_CONTROL_TOKEN=\"exclude\""));
    assert!(
        has_exclusion_filter,
        "Codex proxy config args missing shell_environment_policy CW_CONTROL_TOKEN exclusion!"
    );
}

#[tokio::test]
async fn test_core_contract_ac7_original_transcripts_untouched_bit_for_bit() {
    let f = QualificationFixture::setup().await;
    let client = reqwest::Client::new();

    let temp_dir = std::env::temp_dir().join(format!("cw_test_ac7_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_dir).unwrap();

    let agy_fixture = std::path::Path::new("../cw-core/tests/fixtures/agy/transcript.jsonl");
    let agy_transcript = temp_dir.join("transcript.jsonl");
    if agy_fixture.exists() {
        std::fs::copy(agy_fixture, &agy_transcript).unwrap();
    } else {
        std::fs::write(
            &agy_transcript,
            b"{\"step_index\": 1, \"type\": \"USER_INPUT\", \"content\": \"pristine_agy\"}\n",
        )
        .unwrap();
    }

    let omp_transcript = temp_dir.join("omp_session.jsonl");
    let hermes_db = temp_dir.join("hermes_state.db");
    let omp_data = b"{\"type\": \"message\", \"id\": \"msg_0\", \"message\": {\"role\": \"user\", \"content\": \"pristine_omp\"}}\n";
    let hermes_data = b"PRISTINE_SQLITE_HERMES_HEADER_AND_BODY_DATA_BLOCK";

    std::fs::write(&omp_transcript, omp_data).unwrap();
    std::fs::write(&hermes_db, hermes_data).unwrap();

    let agy_initial = std::fs::read(&agy_transcript).unwrap();
    let hash_agy_before = blake3::hash(&agy_initial);
    let hash_omp_before = blake3::hash(omp_data);
    let hash_hermes_before = blake3::hash(hermes_data);

    // 1. Ingest AGY transcript via harness adapter into CwStore
    let agy_session_id =
        cw_harnesses::import_agy_transcript(&agy_transcript, &f.store, None).unwrap();
    assert!(!agy_session_id.is_empty());

    // 2. Commit active overlay mutation on the ingested session
    f.store
        .save_mutation(&MutationRule {
            id: "ac7_test_overlay".into(),
            session_id: agy_session_id.clone(),
            op: MutationOp::PinContext {
                label: "ImmutabilityPolicy".into(),
                content: "PRESERVE_SOURCE_TRANSCRIPT".into(),
                source_node_id: None,
            },
            status: MutationStatus::Committed,
            estimated_saved_tokens: 0,
            estimated_kv_break_tokens: 0,
            created_at: Utc::now(),
        })
        .unwrap();

    // 3. Perform proxy requests with committed overlay
    let sess = f.store.get_session(&agy_session_id).unwrap().unwrap();
    let payload = json!({
        "model": sess.model,
        "max_tokens": 1024,
        "messages": [{"role": "user", "content": "read transcript"}]
    });

    let mut req = client
        .post(format!(
            "http://127.0.0.1:{}/anthropic/v1/messages",
            f.proxy_port
        ))
        .header("x-cw-session-id", &agy_session_id)
        .header("x-cw-control-token", &f.control_token)
        .header("content-type", "application/json");
    if let Some(ws) = &sess.workspace_root {
        req = req.header("x-cw-workspace", ws);
    }
    let resp = req.json(&payload).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // 4. Verify all on-disk harness transcript files are bit-for-bit unchanged
    let agy_after = std::fs::read(&agy_transcript).unwrap();
    let omp_after = std::fs::read(&omp_transcript).unwrap();
    let hermes_after = std::fs::read(&hermes_db).unwrap();

    assert_eq!(blake3::hash(&agy_after), hash_agy_before);
    assert_eq!(blake3::hash(&omp_after), hash_omp_before);
    assert_eq!(blake3::hash(&hermes_after), hash_hermes_before);

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_core_contract_ac8_safe_recovery_and_validation_gate_fail_open() {
    let f = QualificationFixture::setup().await;
    let client = reqwest::Client::new();
    let session_id = "test_ac8_recovery_session";

    // 1. Invalid session ID with illegal characters is rejected with 400 Bad Request
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

    // 2. Pre-Forward Validation Gate: commit a mutation that corrupts the tool call ID
    // which would create an orphan tool result. The proxy must catch this invariant violation
    // via validate_protocol_invariants, revert outbound to raw unmutated payload, and forward safely.
    f.store
        .upsert_session(&Session {
            id: session_id.into(),
            parent_session_id: None,
            harness: HarnessKind::GenericProxy,
            workspace_root: Some("/tmp".into()),
            model: "gpt-4o".into(),
            provider: ProviderKind::OpenAI,
            title: Some("ac8 recovery session".into()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        })
        .unwrap();

    // Commit an active mutation rule that targets the assistant tool call ID directly,
    // mutating it to "corrupted_call_id" which breaks 1:1 pairing with the tool result.
    f.store
        .save_mutation(&MutationRule {
            id: "ac8_corrupting_mutation".into(),
            session_id: session_id.into(),
            op: MutationOp::PruneOrStub {
                target_fingerprint: None,
                target_call_id: None,
                target_json_pointer: Some("/messages/1/tool_calls/0/id".into()),
                stub_text: "corrupted_call_id".into(),
            },
            status: MutationStatus::Committed,
            estimated_saved_tokens: 0,
            estimated_kv_break_tokens: 0,
            created_at: Utc::now(),
        })
        .unwrap();

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
                "content": "file1.rs\nfile2.rs"
            }
        ]
    });

    // Verify raw input is structurally valid before proxy submission
    assert!(validate_protocol_invariants(&payload, ProviderKind::OpenAI).is_ok());

    let good_resp = client
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
    assert_eq!(good_resp.status(), StatusCode::OK);

    // Verify upstream received the raw unmutated payload with pairing intact
    let captured = f.mock.captured.lock().unwrap().clone();
    assert!(!captured.is_empty());
    let last_captured = captured.last().unwrap();
    let req_json = last_captured.json_body.as_ref().expect("captured JSON");
    let messages = req_json.get("messages").and_then(Value::as_array).unwrap();
    let assistant_msg = &messages[1];
    let tool_call_id = assistant_msg
        .pointer("/tool_calls/0/id")
        .and_then(Value::as_str)
        .unwrap();
    let tool_result_id = messages[2]
        .get("tool_call_id")
        .and_then(Value::as_str)
        .unwrap();

    // The corrupting mutation was caught and dropped; raw tool ID "call_inspect_123" was forwarded!
    assert_eq!(tool_call_id, "call_inspect_123");
    assert_eq!(tool_result_id, "call_inspect_123");
    assert!(validate_protocol_invariants(req_json, ProviderKind::OpenAI).is_ok());

    // Verify turn record in CwStore cleared the applied mutation IDs due to validation fallback
    let turns = f.store.list_turns(session_id).unwrap();
    assert_eq!(turns.len(), 1);
    assert!(
        turns[0].applied_mutation_ids.is_empty(),
        "Corrupted mutation rule must be excluded from applied_mutation_ids"
    );
}

#[tokio::test]
async fn test_core_contract_ac10_token_accounting_and_telemetry_reconciliation() {
    let f = QualificationFixture::setup().await;
    let client = reqwest::Client::new();
    let session_id = "test_ac10_accounting_session";

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

    // Give background response processing a moment to persist observed telemetry
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    let turns = f.store.list_turns(session_id).unwrap();
    assert_eq!(turns.len(), 1);
    let turn = &turns[0];

    // Reconcile observed provider usage vs normalized input tokens
    let obs = turn
        .observed_usage
        .as_ref()
        .expect("observed_usage present");
    assert_eq!(obs.get("prompt_tokens").and_then(Value::as_u64), Some(1250));
    assert_eq!(
        obs.get("completion_tokens").and_then(Value::as_u64),
        Some(180)
    );
    assert_eq!(turn.token_usage.output_tokens, 180);
    assert_eq!(turn.token_usage.input_tokens, 4);
    // Cached status must be strictly un-fabricated when telemetry is missing
    assert_eq!(turn.token_usage.cached_tokens, 0);
    assert!(!turn.kv_cache_report.telemetry_available);
    assert_eq!(turn.kv_cache_report.hit_ratio, 0.0);
}

// =============================================================================
// Suite B: Harness Binary Integration Tests (Requires Installed Binaries)
// =============================================================================

#[tokio::test]
#[ignore = "harness binary integration test; requires installed 'codex'; run with cargo test --test qualification harness_binary -- --ignored"]
async fn test_harness_binary_codex_cli_multiturn_overlay_reapplication() {
    let codex_bin = require_installed_binary("codex");

    let f = QualificationFixture::setup().await;
    let session_id = "codex_qual_multiturn_session";
    let temp_home = std::env::temp_dir().join(format!("codex_home_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_home).unwrap();

    let proxy_port = f.proxy_port;
    let token = f.control_token.clone();
    let temp_home_clone = temp_home.clone();
    let sid = session_id.to_string();

    // Turn 1 Execution
    let output1 = tokio::task::spawn_blocking({
        let codex_bin = codex_bin.clone();
        let sid = sid.clone();
        let token = token.clone();
        let temp_home_clone = temp_home_clone.clone();
        move || {
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
            cmd.args(["-m", "mock-model", "Turn 1 prompt"]);
            cmd.env("OPENAI_API_KEY", "mock-openai-key");
            cmd.env("CODEX_HOME", &temp_home_clone);
            cmd.env("CW_SESSION_ID", &sid);
            cmd.env("CW_CONTROL_TOKEN", &token);
            cmd.output()
        }
    })
    .await
    .unwrap()
    .expect("failed to run codex binary on turn 1");

    let stderr1 = String::from_utf8_lossy(&output1.stderr);
    assert_eq!(
        output1.status.code(),
        Some(0),
        "Codex Turn 1 failed: {stderr1}"
    );

    // Commit overlay mutation on the session after Turn 1
    f.store
        .save_mutation(&MutationRule {
            id: "codex_pin_rule".into(),
            session_id: sid.clone(),
            op: MutationOp::PinContext {
                label: "Policy".into(),
                content: "CRITICAL_POLICY_REAPPLIED_ON_TURN_2".into(),
                source_node_id: None,
            },
            status: MutationStatus::Committed,
            estimated_saved_tokens: 0,
            estimated_kv_break_tokens: 0,
            created_at: Utc::now(),
        })
        .unwrap();

    // Turn 2 Execution with identical session ID
    let output2 = tokio::task::spawn_blocking({
        let codex_bin = codex_bin.clone();
        let sid = sid.clone();
        let token = token.clone();
        let temp_home_clone = temp_home_clone.clone();
        move || {
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
            cmd.args(["-m", "mock-model", "Turn 2 prompt"]);
            cmd.env("OPENAI_API_KEY", "mock-openai-key");
            cmd.env("CODEX_HOME", &temp_home_clone);
            cmd.env("CW_SESSION_ID", &sid);
            cmd.env("CW_CONTROL_TOKEN", &token);
            cmd.output()
        }
    })
    .await
    .unwrap()
    .expect("failed to run codex binary on turn 2");

    let stderr2 = String::from_utf8_lossy(&output2.stderr);
    assert_eq!(
        output2.status.code(),
        Some(0),
        "Codex Turn 2 failed: {stderr2}"
    );

    let _ = std::fs::remove_dir_all(&temp_home);

    // Verify proxy recorded 2 turns under the exact same session ID
    let sess = f
        .store
        .get_session(&sid)
        .unwrap()
        .expect("codex session recorded");
    assert_eq!(sess.id, sid);
    let turns = f.store.list_turns(&sid).unwrap();
    assert_eq!(
        turns.len(),
        2,
        "Expected 2 turns recorded for codex session"
    );

    // Inspect captured requests forwarded to mock provider
    let captured = f.mock.captured.lock().unwrap().clone();
    assert_eq!(captured.len(), 2, "Expected 2 requests forwarded upstream");

    let body_turn1 = String::from_utf8_lossy(&captured[0].body);
    let body_turn2 = String::from_utf8_lossy(&captured[1].body);

    // Turn 1 must NOT contain the pin (it was committed after turn 1)
    assert!(!body_turn1.contains("CRITICAL_POLICY_REAPPLIED_ON_TURN_2"));

    // Turn 2 MUST contain the pin injected into the Responses payload
    assert!(
        body_turn2.contains("CRITICAL_POLICY_REAPPLIED_ON_TURN_2"),
        "Turn 2 request did not contain committed overlay mutation!"
    );

    // Neither request must leak contextWatcher control token or session ID headers
    for req in &captured {
        assert!(!req.headers.contains_key("x-cw-control-token"));
        assert!(!req.headers.contains_key("x-cw-session-id"));
    }
}

#[tokio::test]
#[ignore = "harness binary integration test; requires installed 'hermes'; run with cargo test --test qualification harness_binary -- --ignored"]
async fn test_harness_binary_hermes_middleware_session_binding() {
    let hermes_bin = require_installed_binary("hermes");

    let f = QualificationFixture::setup().await;
    let proxy_port = f.proxy_port;
    let control_token = f.control_token.clone();
    let session_id = "hermes_bound_qual_sess";

    // Create temporary control token file with 0600 mode
    let temp_token_file = std::env::temp_dir().join(format!("cw_token_{}", uuid::Uuid::new_v4()));
    std::fs::write(&temp_token_file, &control_token).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temp_token_file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    let token_path_str = temp_token_file.to_string_lossy().to_string();

    // Turn 1: Hermes executes prompt and binds to configured CW_SESSION_ID
    let hermes_bin_clone1 = hermes_bin.clone();
    let token_path_clone1 = token_path_str.clone();
    let output1 = tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new(hermes_bin_clone1);
        cmd.args([
            "chat",
            "-Q",
            "-q",
            "Say turn 1 prompt",
            "--provider",
            "custom",
            "--base-url",
            &format!("http://127.0.0.1:{proxy_port}/deepseek"),
            "--api-key",
            "mock-key",
            "-m",
            "mock-model",
            "--source",
            "tool",
        ]);
        cmd.env("CW_CONTROL_TOKEN_FILE", token_path_clone1);
        cmd.env("CW_SESSION_ID", session_id);
        cmd.output()
    })
    .await
    .unwrap()
    .expect("failed to run hermes binary on turn 1");

    let stdout1 = String::from_utf8_lossy(&output1.stdout);
    let stderr1 = String::from_utf8_lossy(&output1.stderr);

    assert_eq!(
        output1.status.code(),
        Some(0),
        "Hermes turn 1 failed: stdout: {stdout1}\nstderr: {stderr1}"
    );
    assert!(
        stdout1.contains("Mock chat output"),
        "Stdout did not contain mock response: {stdout1}"
    );

    // Extract Hermes's authenticated native session ID
    let sid = stderr1
        .lines()
        .find_map(|l| l.strip_prefix("session_id: "))
        .map(str::trim)
        .expect("Hermes must output its session_id in stderr");
    assert!(
        sid.starts_with("local-"),
        "Unexpected session ID format: {sid}"
    );

    // Verify proxy bound and recorded Turn 1 under that exact authenticated session ID
    let bound_session = f
        .store
        .get_session(sid)
        .unwrap()
        .expect("Expected Hermes session to be bound and recorded in CwStore");
    assert_eq!(bound_session.id, sid);
    let turns1 = f.store.list_turns(sid).unwrap();
    assert_eq!(
        turns1.len(),
        1,
        "Expected 1 turn recorded for bound session"
    );

    // Commit active overlay mutation on the bound session
    f.store
        .save_mutation(&MutationRule {
            id: "hermes_pin_overlay".into(),
            session_id: sid.into(),
            op: MutationOp::PinContext {
                label: "SafetyDirective".into(),
                content: "CRITICAL_HERMES_PINNED_POLICY".into(),
                source_node_id: None,
            },
            status: MutationStatus::Committed,
            estimated_saved_tokens: 0,
            estimated_kv_break_tokens: 0,
            created_at: Utc::now(),
        })
        .unwrap();

    // Turn 2: Hermes resumes the session (--resume <sid>); middleware preserves session ID
    let hermes_bin_clone2 = hermes_bin.clone();
    let token_path_clone2 = token_path_str.clone();
    let sid_string = sid.to_string();
    let output2 = tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new(hermes_bin_clone2);
        cmd.args([
            "chat",
            "-Q",
            "-r",
            &sid_string,
            "-q",
            "Say turn 2 follow-up",
            "--provider",
            "custom",
            "--base-url",
            &format!("http://127.0.0.1:{proxy_port}/deepseek"),
            "--api-key",
            "mock-key",
            "-m",
            "mock-model",
            "--source",
            "tool",
        ]);
        cmd.env("CW_CONTROL_TOKEN_FILE", token_path_clone2);
        cmd.output()
    })
    .await
    .unwrap()
    .expect("failed to run hermes binary on turn 2");

    let stdout2 = String::from_utf8_lossy(&output2.stdout);
    let stderr2 = String::from_utf8_lossy(&output2.stderr);
    let _ = std::fs::remove_file(&temp_token_file);

    assert_eq!(
        output2.status.code(),
        Some(0),
        "Hermes turn 2 failed: stdout: {stdout2}\nstderr: {stderr2}"
    );

    // Verify Turn 2 outbound payload intercepted and received the committed overlay
    let captured = f.mock.captured.lock().unwrap().clone();
    let turn2_req = captured.last().expect("Turn 2 request captured");
    let turn2_body = String::from_utf8_lossy(&turn2_req.body);
    assert!(
        turn2_body.contains("CRITICAL_HERMES_PINNED_POLICY"),
        "Turn 2 request did not contain committed overlay in wire body: {turn2_body}"
    );

    // Verify session recorded both turns under the same bound session ID
    let turns2 = f.store.list_turns(sid).unwrap();
    assert_eq!(
        turns2.len(),
        2,
        "Expected 2 turns recorded for Hermes session"
    );

    // Verify upstream received requests with contextWatcher headers stripped
    for req in &captured {
        assert!(!req.headers.contains_key("x-cw-control-token"));
        assert!(!req.headers.contains_key("x-cw-session-id"));
    }
}

fn find_jsonl_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(find_jsonl_files(&path));
            } else if path.extension().and_then(|s| s.to_str()) == Some("jsonl") {
                files.push(path);
            }
        }
    }
    files
}

#[tokio::test]
#[ignore = "harness binary integration test; requires installed 'omp'; run with cargo test --test qualification harness_binary -- --ignored"]
async fn test_harness_binary_omp_native_runtime_overlay_reapplication() {
    let omp_bin = require_installed_binary("omp");

    let f = QualificationFixture::setup().await;
    let temp_agent_dir = std::env::temp_dir().join(format!("omp_agent_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_agent_dir).unwrap();

    // Create private control token file for the extension
    let token_dir = std::env::temp_dir().join(format!("omp_token_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&token_dir).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&token_dir, std::fs::Permissions::from_mode(0o700));
    }
    let token_file = token_dir.join("control-token");
    std::fs::write(&token_file, format!("{}\n", f.control_token)).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&token_file, std::fs::Permissions::from_mode(0o600));
    }

    // AC1 / Security: models.json MUST NOT contain privileged CW control tokens
    let models_cfg =
        cw_harnesses::qualification::omp_proxy_models_config(f.proxy_port, "mock-model");
    let models_json_str = serde_json::to_string_pretty(&models_cfg).unwrap();
    assert!(
        !models_json_str.contains("control-token"),
        "models.json must never contain CW control credentials"
    );
    assert!(
        !models_json_str.contains(&f.control_token),
        "models.json must never contain CW control token"
    );
    std::fs::write(temp_agent_dir.join("models.json"), &models_json_str).unwrap();

    let ext_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("plugins/pi-opencode/index.js");
    assert!(
        ext_path.exists(),
        "plugins/pi-opencode/index.js must exist at {}",
        ext_path.display()
    );

    // Turn 1: Initial prompt executes with native extension loaded; OMP creates native persisted session
    let omp_bin_clone = omp_bin.clone();
    let temp_agent_dir_clone = temp_agent_dir.clone();
    let token_file_clone = token_file.clone();
    let ext_path_clone = ext_path.clone();
    let proxy_port = f.proxy_port;

    let output1 = tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new(omp_bin_clone);
        cmd.args([
            "-p",
            "Turn 1 pristine prompt text",
            "--model",
            "cw-proxy/mock-model",
            "-e",
            &ext_path_clone.to_string_lossy(),
        ]);
        cmd.env("PI_CODING_AGENT_DIR", &temp_agent_dir_clone);
        cmd.env("CW_CONTROL_TOKEN_FILE", &token_file_clone);
        cmd.env("CW_DAEMON_URL", format!("http://127.0.0.1:{proxy_port}"));
        cmd.output()
    })
    .await
    .unwrap()
    .expect("failed to run omp binary Turn 1");

    let stdout1 = String::from_utf8_lossy(&output1.stdout);
    let stderr1 = String::from_utf8_lossy(&output1.stderr);
    assert_eq!(
        output1.status.code(),
        Some(0),
        "OMP Turn 1 CLI failed: stderr: {stderr1}"
    );
    assert!(
        stdout1.contains("Mock chat output"),
        "Stdout did not contain mock response: {stdout1}"
    );

    // Verify Turn 1 request reached upstream
    let captured1 = f.mock.captured.lock().unwrap().clone();
    assert_eq!(captured1.len(), 1);
    assert_eq!(captured1[0].path, "/v1/chat/completions");

    // AC5 / Transcript Evidence: Assert that a real persisted session transcript file was created
    let session_dir = temp_agent_dir.join("sessions");
    assert!(
        session_dir.exists(),
        "OMP must create a sessions directory on disk"
    );
    let session_files = find_jsonl_files(&session_dir);
    assert!(
        !session_files.is_empty(),
        "OMP must generate at least one persisted session .jsonl file"
    );
    let session_file = &session_files[0];
    let initial_disk_content = std::fs::read_to_string(session_file).unwrap();
    assert!(
        !initial_disk_content.trim().is_empty(),
        "Persisted session transcript cannot be empty"
    );
    assert!(
        initial_disk_content.contains("Turn 1 pristine prompt text"),
        "Persisted session transcript must contain initial user prompt"
    );

    // Extract native session UUID from session filename: <timestamp>_<session_uuid>.jsonl
    let filename = session_file.file_stem().unwrap().to_str().unwrap();
    let session_uuid = filename
        .split('_')
        .nth(1)
        .expect("expected uuid component in session filename")
        .to_string();
    let canonical_sid = format!("omp_{session_uuid}");

    // Register session in CwStore
    let sess = Session {
        id: canonical_sid.clone(),
        parent_session_id: None,
        harness: HarnessKind::Omp,
        workspace_root: None,
        model: "mock-model".to_string(),
        provider: ProviderKind::OpenAI,
        title: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };
    f.store.upsert_session(&sess).unwrap();

    // Commit active tail directive (PinContext).
    // Note: User and system prompts must NEVER be pruned or stubbed (enforced by CW safety contract).
    let pin = MutationRule {
        id: "mut_omp_pin_qual".to_string(),
        session_id: canonical_sid.clone(),
        op: MutationOp::PinContext {
            label: "CRITICAL_OMP_PINNED_POLICY".to_string(),
            content: "Zero-hallucination verified native context hook".to_string(),
            source_node_id: None,
        },
        status: MutationStatus::Committed,
        estimated_saved_tokens: 0,
        estimated_kv_break_tokens: 0,
        created_at: Utc::now(),
    };
    f.store
        .save_mutations_at_revision(&canonical_sid, None, &[pin])
        .unwrap();

    // Turn 2: Native OMP Resume (`omp -r <session_uuid>`).
    // Native extension's `context` hook intercepts resumed messages, queries /transform using canonical_sid,
    // and applies active overlays before model dispatch.
    let omp_bin_clone2 = omp_bin.clone();
    let temp_agent_dir_clone2 = temp_agent_dir.clone();
    let token_file_clone2 = token_file.clone();
    let ext_path_clone2 = ext_path.clone();
    let resume_uuid = session_uuid.clone();

    let output2 = tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new(omp_bin_clone2);
        cmd.args([
            "-r",
            &resume_uuid,
            "-p",
            "Turn 2 follow-up prompt text",
            "--model",
            "cw-proxy/mock-model",
            "-e",
            &ext_path_clone2.to_string_lossy(),
        ]);
        cmd.env("PI_CODING_AGENT_DIR", &temp_agent_dir_clone2);
        cmd.env("CW_CONTROL_TOKEN_FILE", &token_file_clone2);
        cmd.env("CW_DAEMON_URL", format!("http://127.0.0.1:{proxy_port}"));
        cmd.output()
    })
    .await
    .unwrap()
    .expect("failed to run omp binary Turn 2 (resumed)");

    let stdout2 = String::from_utf8_lossy(&output2.stdout);
    let stderr2 = String::from_utf8_lossy(&output2.stderr);
    assert_eq!(
        output2.status.code(),
        Some(0),
        "OMP Turn 2 resumed CLI failed: stderr: {stderr2}"
    );
    assert!(
        stdout2.contains("Mock chat output"),
        "Stdout did not contain mock response: {stdout2}"
    );

    // Verify upstream captured Turn 2 containing injected tail pin
    let captured2 = f.mock.captured.lock().unwrap().clone();
    assert!(
        captured2.len() >= 2,
        "Expected at least 2 captured upstream requests, got {}",
        captured2.len()
    );
    let turn2_body = String::from_utf8_lossy(&captured2[captured2.len() - 1].body);
    assert!(
        turn2_body.contains("CRITICAL_OMP_PINNED_POLICY"),
        "Turn 2 payload did not contain injected pinned directive: {turn2_body}"
    );
    assert!(
        turn2_body.contains("Turn 2 follow-up prompt text"),
        "Turn 2 payload missing follow-up prompt: {turn2_body}"
    );

    // AC7 Invariant: On-disk transcript legitimately grows by appending Turn 2,
    // but the original Turn 1 prompt is preserved and in-memory overlays NEVER touch the disk file.
    let disk_content_after_turn2 = std::fs::read_to_string(session_file).unwrap();
    assert!(
        disk_content_after_turn2.contains("Turn 1 pristine prompt text"),
        "Original Turn 1 prompt must be preserved on disk"
    );
    assert!(
        disk_content_after_turn2.contains("Turn 2 follow-up prompt text"),
        "Turn 2 follow-up prompt must be appended to disk transcript"
    );
    assert!(
        !disk_content_after_turn2.contains("CRITICAL_OMP_PINNED_POLICY"),
        "On-disk transcript must NEVER contain injected pinned policy"
    );

    // Turn 3: Fail-Open Verification under Daemon Unavailability
    // When daemon URL points to an offline endpoint, OMP must still complete Turn 3 successfully with raw prompt
    let ext_path_clone3 = ext_path.clone();
    let temp_agent_dir_clone3 = temp_agent_dir.clone();
    let token_file_clone3 = token_file.clone();
    let resume_uuid3 = session_uuid.clone();

    let output3 = tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new(omp_bin);
        cmd.args([
            "-r",
            &resume_uuid3,
            "-p",
            "Turn 3 offline fail-open prompt text",
            "--model",
            "cw-proxy/mock-model",
            "-e",
            &ext_path_clone3.to_string_lossy(),
        ]);
        cmd.env("PI_CODING_AGENT_DIR", &temp_agent_dir_clone3);
        cmd.env("CW_CONTROL_TOKEN_FILE", &token_file_clone3);
        // Point daemon URL to an unreachable port to test fail-open resiliency
        cmd.env("CW_DAEMON_URL", "http://127.0.0.1:19999");
        cmd.output()
    })
    .await
    .unwrap()
    .expect("failed to run omp binary Turn 3 (fail-open)");

    assert_eq!(
        output3.status.code(),
        Some(0),
        "OMP Turn 3 fail-open failed with error"
    );

    let captured3 = f.mock.captured.lock().unwrap().clone();
    let turn3_body = String::from_utf8_lossy(&captured3[captured3.len() - 1].body);
    assert!(
        turn3_body.contains("Turn 3 offline fail-open prompt text"),
        "Turn 3 prompt was not forwarded during offline fail-open"
    );

    // Header Stripping Check: Upstream received 0 internal contextWatcher metadata headers
    for cap in &captured3 {
        assert!(
            !cap.headers.contains_key("x-cw-control-token"),
            "Upstream captured x-cw-control-token"
        );
        assert!(
            !cap.headers.contains_key("x-cw-session-id"),
            "Upstream captured x-cw-session-id"
        );
    }

    // AC1 / Session Precedence & Idempotency: Assert native session takes precedence over
    // process.env.CW_SESSION_ID and single-pass idempotency holds without time heuristics.
    let ext_js = ext_path.to_string_lossy().to_string();
    let node_script = format!(
        r#"
let capturedUrls = [];
let transformCalls = 0;
globalThis.fetch = async (url) => {{
  capturedUrls.push(String(url));
  transformCalls++;
  return {{
    ok: true,
    json: async () => ({{ messages: [{{ role: "user", content: "transformed" }}] }})
  }};
}};

process.env.CW_SESSION_ID = "omp_legacy_env_wrapper_id";

import("{ext_js}").then(async (mod) => {{
  let contextHook = null;
  let beforeHook = null;
  const mockPi = {{
    on: (evt, fn) => {{
      if (evt === "context") contextHook = fn;
      if (evt === "before_provider_request") beforeHook = fn;
    }},
    registerCommand: () => {{}}
  }};
  mod.default(mockPi);

  // 1. Session Isolation: Native session 1 takes precedence over process.env.CW_SESSION_ID
  const ctx1 = {{ sessionManager: {{ getSessionId: () => "native-uuid-1" }} }};
  const resContext1 = await contextHook({{ messages: [{{ role: "user", content: "t1" }}] }}, ctx1);

  // 2. Session Switching: Switching native sessions within process binds to new native session
  const ctx2 = {{ sessionManager: {{ getSessionId: () => "native-uuid-2" }} }};
  const resContext2 = await contextHook({{ messages: [{{ role: "user", content: "t2" }}] }}, ctx2);

  // 3. Fallback: Absent native session uses env
  await contextHook({{ messages: [{{ role: "user", content: "t3" }}] }}, {{}});

  if (!capturedUrls[0].includes("omp_native-uuid-1")) throw new Error("Expected omp_native-uuid-1, got " + capturedUrls[0]);
  if (!capturedUrls[1].includes("omp_native-uuid-2")) throw new Error("Expected omp_native-uuid-2, got " + capturedUrls[1]);
  if (!capturedUrls[2].includes("omp_legacy_env_wrapper_id")) throw new Error("Expected omp_legacy_env_wrapper_id, got " + capturedUrls[2]);

  // 4. Idempotency: before_provider_request receiving transformed messages skips second call
  const callCountBefore = transformCalls;
  await beforeHook({{ payload: {{ messages: resContext1.messages }} }}, ctx1);
  if (transformCalls !== callCountBefore) throw new Error("Duplicate transformation detected!");

  // 5. Bypass: Turn bypassing context hook still executes
  await beforeHook({{ payload: {{ messages: [{{ role: "user", content: "bypassed" }}] }} }}, ctx1);
  if (transformCalls !== callCountBefore + 1) throw new Error("Bypass turn failed to transform");
}});
"#
    );

    let node_out = Command::new("node")
        .args(["-e", &node_script])
        .output()
        .expect("failed to run node session precedence check");
    assert!(
        node_out.status.success(),
        "Node session isolation/idempotency check failed: {}",
        String::from_utf8_lossy(&node_out.stderr)
    );

    let _ = std::fs::remove_dir_all(&temp_agent_dir);
    let _ = std::fs::remove_dir_all(&token_dir);
}

// =============================================================================
// Suite C: Multi-Turn Opt-in Real-Provider Continuation Tests
// =============================================================================

#[tokio::test]
#[ignore = "opt-in real Codex qualification; requires CW_QUALIFY_REAL_CODEX and OPENAI_API_KEY"]
async fn test_live_continuation_real_codex_multiturn() {
    if std::env::var("CW_QUALIFY_REAL_CODEX").is_err() {
        eprintln!("SKIPPED: CW_QUALIFY_REAL_CODEX not set");
        return;
    }
    let api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY required for live test");
    let codex_bin = require_installed_binary("codex");

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

    // Turn 1
    let output1 = tokio::task::spawn_blocking({
        let codex_bin = codex_bin.clone();
        let api_key = api_key.clone();
        let temp_home_clone = temp_home_clone.clone();
        let sid = sid.clone();
        let token = token.clone();
        move || {
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
            cmd.args(["-m", "gpt-4o-mini", "Say live-codex-turn-1"]);
            cmd.env("OPENAI_API_KEY", api_key);
            cmd.env("CODEX_HOME", &temp_home_clone);
            cmd.env("CW_SESSION_ID", sid);
            cmd.env("CW_CONTROL_TOKEN", token);
            cmd.output()
        }
    })
    .await
    .unwrap()
    .expect("failed to run codex binary on live turn 1");

    assert_eq!(output1.status.code(), Some(0));

    // Commit overlay mutation on live session
    store
        .save_mutation(&MutationRule {
            id: "live_pin_rule".into(),
            session_id: sid.clone(),
            op: MutationOp::PinContext {
                label: "Directive".into(),
                content: "LIVE_INJECTED_OVERLAY_ACTIVE".into(),
                source_node_id: None,
            },
            status: MutationStatus::Committed,
            estimated_saved_tokens: 0,
            estimated_kv_break_tokens: 0,
            created_at: Utc::now(),
        })
        .unwrap();

    // Turn 2
    let output2 = tokio::task::spawn_blocking({
        let codex_bin = codex_bin.clone();
        let api_key = api_key.clone();
        let temp_home_clone = temp_home_clone.clone();
        let sid = sid.clone();
        let token = token.clone();
        move || {
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
            cmd.args(["-m", "gpt-4o-mini", "Say live-codex-turn-2"]);
            cmd.env("OPENAI_API_KEY", api_key);
            cmd.env("CODEX_HOME", &temp_home_clone);
            cmd.env("CW_SESSION_ID", sid);
            cmd.env("CW_CONTROL_TOKEN", token);
            cmd.output()
        }
    })
    .await
    .unwrap()
    .expect("failed to run codex binary on live turn 2");

    let stdout2 = String::from_utf8_lossy(&output2.stdout);
    let stderr2 = String::from_utf8_lossy(&output2.stderr);
    let _ = std::fs::remove_dir_all(&temp_home);
    proxy_task.abort();

    assert_eq!(
        output2.status.code(),
        Some(0),
        "Live Codex turn 2 failed: {stderr2}"
    );
    assert!(stdout2.contains("live-codex-turn-2") || stderr2.contains("live-codex-turn-2"));

    let sess = store
        .get_session(session_id)
        .unwrap()
        .expect("session recorded");
    assert_eq!(sess.id, session_id);
    let turns = store.list_turns(session_id).unwrap();
    assert_eq!(
        turns.len(),
        2,
        "Expected 2 turns recorded for live codex session"
    );
}

#[tokio::test]
#[ignore = "opt-in real Hermes qualification; requires CW_QUALIFY_REAL_HERMES and DEEPSEEK_API_KEY"]
async fn test_live_continuation_real_hermes_multiturn() {
    if std::env::var("CW_QUALIFY_REAL_HERMES").is_err() {
        eprintln!("SKIPPED: CW_QUALIFY_REAL_HERMES not set");
        return;
    }
    let api_key =
        std::env::var("DEEPSEEK_API_KEY").expect("DEEPSEEK_API_KEY required for live test");
    let hermes_bin = require_installed_binary("hermes");

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
    let omp_bin = require_installed_binary("omp");

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
