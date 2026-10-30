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
                        async move { handle_mock_responses(headers, body, captured).await }
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
                            handle_mock_chat_completion(
                                headers,
                                body,
                                "/v1/chat/completions",
                                captured,
                            )
                            .await
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
                            handle_mock_chat_completion(
                                headers,
                                body,
                                "/chat/completions",
                                captured,
                            )
                            .await
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
                            handle_mock_chat_completion(
                                headers,
                                body,
                                "/deepseek/chat/completions",
                                captured,
                            )
                            .await
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
                            }))
                            .into_response()
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
    let body_str = String::from_utf8_lossy(&body);
    // If this is Process A's request in the concurrent qualification test, hold the connection
    // briefly so that Process B's concurrent request is guaranteed to overlap in-flight.
    if body_str.contains("Prompt from Hermes Process A") {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }

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

async fn handle_mock_responses(
    headers: HeaderMap,
    body: axum::body::Bytes,
    captured: Arc<Mutex<Vec<CapturedRequest>>>,
) -> Response {
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
        json_body: json_val.clone(),
    });

    let body_str = String::from_utf8_lossy(&body);

    if body_str.contains("CW_FORCE_UPSTREAM_502") {
        return (StatusCode::BAD_GATEWAY, "Upstream mock gateway failure").into_response();
    }

    if body_str.contains("CW112_TASK_TURN_1")
        || body_str.contains("CW112_TASK_TURN_2")
        || body_str.contains("CW112_TASK_TURN_3")
    {
        let (turn, call_id, cmd, text) = if body_str.contains("CW112_TASK_TURN_3") {
            (
                3,
                "call_turn_3",
                "python3 -c \"with open('test_calculator.py', 'w') as f: f.write('import unittest\\nfrom calculator import add, multiply\\n\\nclass TestCalc(unittest.TestCase):\\n    def test_add(self):\\n        self.assertEqual(add(2, 3), 5)\\n    def test_mul(self):\\n        self.assertEqual(multiply(2, 3), 6)\\n\\nif __name__ == \\\"__main__\\\":\\n    unittest.main()\\n')\" && python3 test_calculator.py",
                "Created test_calculator.py and verified tests pass.",
            )
        } else if body_str.contains("CW112_TASK_TURN_2") {
            (
                2,
                "call_turn_2",
                "python3 -c \"with open('calculator.py', 'a') as f: f.write('def multiply(a, b):\\n    return a * b\\n')\"",
                "Added multiply function to calculator.py.",
            )
        } else {
            (
                1,
                "call_turn_1",
                "python3 -c \"import os; open('env_check.txt', 'w').write(os.environ.get('CW_CONTROL_TOKEN', 'ABSENT')); open('calculator.py', 'w').write('def add(a, b):\\n    return a + b\\n')\"",
                "Created calculator.py with add function.",
            )
        };

        let has_output = json_val.as_ref().is_some_and(|payload| {
            payload
                .get("input")
                .and_then(Value::as_array)
                .is_some_and(|items| {
                    items.iter().any(|item| {
                        item.get("type").and_then(Value::as_str) == Some("function_call_output")
                            && item.get("call_id").and_then(Value::as_str) == Some(call_id)
                    })
                })
        });

        if !has_output {
            let args = serde_json::to_string(&json!({"cmd": cmd})).unwrap();
            if is_stream {
                let events = [
                    json!({"type": "response.created", "response": {"id": format!("resp_{turn}_call"), "status": "in_progress"}}),
                    json!({"type": "response.output_item.added", "output_index": 0, "item": {"id": format!("fc_{turn}"), "type": "function_call", "call_id": call_id, "name": "exec_command", "arguments": ""}}),
                    json!({"type": "response.function_call_arguments.delta", "output_index": 0, "call_id": call_id, "delta": args}),
                    json!({"type": "response.function_call_arguments.done", "output_index": 0, "call_id": call_id, "arguments": args}),
                    json!({"type": "response.output_item.done", "output_index": 0, "item": {"id": format!("fc_{turn}"), "type": "function_call", "call_id": call_id, "name": "exec_command", "arguments": args}}),
                    json!({"type": "response.completed", "response": {"id": format!("resp_{turn}_call"), "status": "completed", "output": [{"id": format!("fc_{turn}"), "type": "function_call", "call_id": call_id, "name": "exec_command", "arguments": args}], "usage": {"input_tokens": 10, "output_tokens": 10, "total_tokens": 20}}}),
                ];
                let mut sse_body = String::new();
                for ev in events {
                    sse_body.push_str(&format!(
                        "data: {}\n\n",
                        serde_json::to_string(&ev).unwrap()
                    ));
                }
                sse_body.push_str("data: [DONE]\n\n");
                return Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "text/event-stream")
                    .header("cache-control", "no-cache")
                    .body(Body::from(sse_body))
                    .unwrap();
            } else {
                return Json(json!({
                    "id": format!("resp_{turn}_call"),
                    "output": [{
                        "id": format!("fc_{turn}"),
                        "type": "function_call",
                        "call_id": call_id,
                        "name": "exec_command",
                        "arguments": args
                    }],
                    "usage": {"input_tokens": 10, "output_tokens": 10, "total_tokens": 20}
                }))
                .into_response();
            }
        } else if is_stream {
            let events = [
                json!({"type": "response.created", "response": {"id": format!("resp_{turn}_done"), "status": "in_progress"}}),
                json!({"type": "response.output_item.added", "output_index": 0, "item": {"id": format!("msg_{turn}"), "type": "message", "role": "assistant", "content": []}}),
                json!({"type": "response.content_part.added", "output_index": 0, "content_index": 0, "part": {"type": "output_text", "text": ""}}),
                json!({"type": "response.output_text.delta", "output_index": 0, "content_index": 0, "delta": text}),
                json!({"type": "response.output_text.done", "output_index": 0, "content_index": 0, "text": text}),
                json!({"type": "response.content_part.done", "output_index": 0, "content_index": 0, "part": {"type": "output_text", "text": text}}),
                json!({"type": "response.output_item.done", "output_index": 0, "item": {"id": format!("msg_{turn}"), "type": "message", "role": "assistant", "content": [{"type": "output_text", "text": text}]}}),
                json!({"type": "response.completed", "response": {"id": format!("resp_{turn}_done"), "status": "completed", "output": [{"id": format!("msg_{turn}"), "type": "message", "role": "assistant", "content": [{"type": "output_text", "text": text}]}], "usage": {"input_tokens": 10, "output_tokens": 10, "total_tokens": 20}}}),
            ];
            let mut sse_body = String::new();
            for ev in events {
                sse_body.push_str(&format!(
                    "data: {}\n\n",
                    serde_json::to_string(&ev).unwrap()
                ));
            }
            sse_body.push_str("data: [DONE]\n\n");
            return Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "text/event-stream")
                .header("cache-control", "no-cache")
                .body(Body::from(sse_body))
                .unwrap();
        } else {
            return Json(json!({
                "id": format!("resp_{turn}_done"),
                "output": [{
                    "id": format!("msg_{turn}"),
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": text}]
                }],
                "usage": {"input_tokens": 10, "output_tokens": 10, "total_tokens": 20}
            }))
            .into_response();
        }
    }

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
            sse_body.push_str(&format!(
                "data: {}\n\n",
                serde_json::to_string(&ev).unwrap()
            ));
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
        }))
        .into_response()
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

fn extract_thread_id_from_stdout(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        if let Ok(v) = serde_json::from_str::<Value>(line)
            && v.get("type").and_then(Value::as_str) == Some("thread.started")
            && let Some(tid) = v.get("thread_id").and_then(Value::as_str)
        {
            return Some(tid.to_string());
        }
    }
    None
}

fn find_rollout_files(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(find_rollout_files(&path));
            } else if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("rollout-") && n.ends_with(".jsonl"))
            {
                files.push(path);
            }
        }
    }
    files
}

#[tokio::test]
#[ignore = "harness binary integration test; requires installed 'codex'; run with cargo test --test qualification harness_binary -- --ignored"]
async fn test_harness_binary_codex_cli_native_resume_three_turn_qualification() {
    let codex_bin = require_installed_binary("codex");

    let f = QualificationFixture::setup().await;
    let cw_session_id = format!("cw_codex_qual_{}", uuid::Uuid::new_v4());
    let temp_home = std::env::temp_dir().join(format!("codex_home_{}", uuid::Uuid::new_v4()));
    let temp_workspace = std::env::temp_dir().join(format!("codex_ws_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_home).unwrap();
    std::fs::create_dir_all(&temp_workspace).unwrap();

    let proxy_port = f.proxy_port;
    let token = f.control_token.clone();

    // Turn 1 Execution: Codex starts a new native session and creates calculator.py
    let output1 = tokio::task::spawn_blocking({
        let codex_bin = codex_bin.clone();
        let sid = cw_session_id.clone();
        let token = token.clone();
        let temp_home = temp_home.clone();
        let temp_workspace = temp_workspace.clone();
        move || {
            let mut cmd = Command::new(codex_bin);
            cmd.current_dir(&temp_workspace);
            cmd.args([
                "exec",
                "--json",
                "--dangerously-bypass-approvals-and-sandbox",
                "--skip-git-repo-check",
                "--ignore-user-config",
            ]);
            cmd.args(cw_harnesses::qualification::codex_proxy_config_args(
                proxy_port,
            ));
            cmd.args([
                "-m",
                "mock-model",
                "CW112_TASK_TURN_1: create calculator.py with add function",
            ]);
            cmd.env("OPENAI_API_KEY", "mock-openai-key");
            cmd.env("CODEX_HOME", &temp_home);
            cmd.env("CW_SESSION_ID", &sid);
            cmd.env("CW_CONTROL_TOKEN", &token);
            cmd.output()
        }
    })
    .await
    .unwrap()
    .expect("failed to run codex binary on turn 1");

    let stdout1 = String::from_utf8_lossy(&output1.stdout);
    let stderr1 = String::from_utf8_lossy(&output1.stderr);
    assert!(
        output1.status.success(),
        "Codex Turn 1 failed: {stderr1}\nstdout: {stdout1}"
    );

    let thread_id = extract_thread_id_from_stdout(&stdout1)
        .expect("Codex must emit thread.started with thread_id on Turn 1");
    assert!(!thread_id.is_empty());

    let calc_file = temp_workspace.join("calculator.py");
    assert!(
        calc_file.exists(),
        "calculator.py should exist after Turn 1"
    );
    let calc_content_1 = std::fs::read_to_string(&calc_file).expect("read calculator.py");
    assert!(
        calc_content_1.contains("def add(a, b):"),
        "calculator.py must contain add function after Turn 1"
    );

    // Verify credential sandboxing: tool command executed by Codex must NOT inherit CW_CONTROL_TOKEN
    let env_check = std::fs::read_to_string(temp_workspace.join("env_check.txt"))
        .expect("read env_check.txt written by model tool execution");
    assert_eq!(
        env_check.trim(),
        "ABSENT",
        "Model-invoked tool command inside Codex must NOT inherit CW_CONTROL_TOKEN"
    );

    // Commit overlay mutation on the session after Turn 1
    f.store
        .save_mutation(&MutationRule {
            id: "cw112_pin_rule".into(),
            session_id: cw_session_id.clone(),
            op: MutationOp::PinContext {
                label: "Policy".into(),
                content: "CW112_POLICY_ACTIVE".into(),
                source_node_id: None,
            },
            status: MutationStatus::Committed,
            estimated_saved_tokens: 0,
            estimated_kv_break_tokens: 0,
            created_at: Utc::now(),
        })
        .unwrap();

    // Turn 2 Execution: Codex resumes the existing native session using `codex exec resume <thread_id>`
    let output2 = tokio::task::spawn_blocking({
        let codex_bin = codex_bin.clone();
        let sid = cw_session_id.clone();
        let token = token.clone();
        let temp_home = temp_home.clone();
        let temp_workspace = temp_workspace.clone();
        let thread_id = thread_id.clone();
        move || {
            let mut cmd = Command::new(codex_bin);
            cmd.current_dir(&temp_workspace);
            cmd.args([
                "exec",
                "resume",
                "--json",
                "--dangerously-bypass-approvals-and-sandbox",
                "--skip-git-repo-check",
                "--ignore-user-config",
            ]);
            cmd.args(cw_harnesses::qualification::codex_proxy_config_args(
                proxy_port,
            ));
            cmd.args([
                "-m",
                "mock-model",
                &thread_id,
                "CW112_TASK_TURN_2: add multiply function to calculator.py",
            ]);
            cmd.env("OPENAI_API_KEY", "mock-openai-key");
            cmd.env("CODEX_HOME", &temp_home);
            cmd.env("CW_SESSION_ID", &sid);
            cmd.env("CW_CONTROL_TOKEN", &token);
            cmd.output()
        }
    })
    .await
    .unwrap()
    .expect("failed to run codex binary on turn 2");

    let stdout2 = String::from_utf8_lossy(&output2.stdout);
    let stderr2 = String::from_utf8_lossy(&output2.stderr);
    assert!(
        output2.status.success(),
        "Codex Turn 2 resume failed: {stderr2}\nstdout: {stdout2}"
    );

    let thread_id_2 = extract_thread_id_from_stdout(&stdout2)
        .expect("Codex must emit thread.started with thread_id on Turn 2");
    assert_eq!(
        thread_id_2, thread_id,
        "Native thread ID must be preserved across resume on Turn 2"
    );

    let calc_content_2 = std::fs::read_to_string(&calc_file).expect("read calculator.py");
    assert!(
        calc_content_2.contains("def add(a, b):") && calc_content_2.contains("def multiply(a, b):"),
        "calculator.py must contain both add and multiply functions after Turn 2"
    );

    // Turn 3 Execution: Second native resume to create test_calculator.py and run tests
    let output3 = tokio::task::spawn_blocking({
        let codex_bin = codex_bin.clone();
        let sid = cw_session_id.clone();
        let token = token.clone();
        let temp_home = temp_home.clone();
        let temp_workspace = temp_workspace.clone();
        let thread_id = thread_id.clone();
        move || {
            let mut cmd = Command::new(codex_bin);
            cmd.current_dir(&temp_workspace);
            cmd.args([
                "exec",
                "resume",
                "--json",
                "--dangerously-bypass-approvals-and-sandbox",
                "--skip-git-repo-check",
                "--ignore-user-config",
            ]);
            cmd.args(cw_harnesses::qualification::codex_proxy_config_args(
                proxy_port,
            ));
            cmd.args([
                "-m",
                "mock-model",
                &thread_id,
                "CW112_TASK_TURN_3: create test_calculator.py and run unit tests",
            ]);
            cmd.env("OPENAI_API_KEY", "mock-openai-key");
            cmd.env("CODEX_HOME", &temp_home);
            cmd.env("CW_SESSION_ID", &sid);
            cmd.env("CW_CONTROL_TOKEN", &token);
            cmd.output()
        }
    })
    .await
    .unwrap()
    .expect("failed to run codex binary on turn 3");

    let stdout3 = String::from_utf8_lossy(&output3.stdout);
    let stderr3 = String::from_utf8_lossy(&output3.stderr);
    assert!(
        output3.status.success(),
        "Codex Turn 3 resume failed: {stderr3}\nstdout: {stdout3}"
    );

    let thread_id_3 = extract_thread_id_from_stdout(&stdout3)
        .expect("Codex must emit thread.started with thread_id on Turn 3");
    assert_eq!(
        thread_id_3, thread_id,
        "Native thread ID must be preserved across resume on Turn 3"
    );

    let test_file = temp_workspace.join("test_calculator.py");
    assert!(
        test_file.exists(),
        "test_calculator.py should exist after Turn 3"
    );

    // Verify unit test execution directly
    let test_run = Command::new("python3")
        .arg("test_calculator.py")
        .current_dir(&temp_workspace)
        .output()
        .expect("failed to run python3 test_calculator.py");
    assert!(
        test_run.status.success(),
        "Generated unit test failed: {}",
        String::from_utf8_lossy(&test_run.stderr)
    );

    // Verify rollout files in CODEX_HOME/sessions:
    // They must parse as valid JSONL and NOT contain CW112_POLICY_ACTIVE
    let rollouts = find_rollout_files(&temp_home.join("sessions"));
    assert!(
        !rollouts.is_empty(),
        "CODEX_HOME/sessions must contain at least one rollout file"
    );
    for rf in &rollouts {
        let content = std::fs::read_to_string(rf).expect("read rollout file");
        for line in content.lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                let parsed = serde_json::from_str::<Value>(trimmed);
                assert!(
                    parsed.is_ok(),
                    "Rollout line in {} must parse as valid JSON: {trimmed}",
                    rf.display()
                );
            }
        }
        assert!(
            !content.contains("CW112_POLICY_ACTIVE"),
            "Rollout file on disk must not be modified with pinned overlay: {}",
            rf.display()
        );
    }

    // Inspect captured requests forwarded upstream to mock provider
    let captured = f.mock.captured.lock().unwrap().clone();
    assert!(
        captured.len() >= 3,
        "Expected at least 3 requests forwarded upstream, got {}",
        captured.len()
    );

    // 1. Upstream requests must NEVER leak contextWatcher internal headers
    for req in &captured {
        assert!(
            !req.headers.contains_key("x-cw-control-token"),
            "Upstream request leaked x-cw-control-token"
        );
        assert!(
            !req.headers.contains_key("x-cw-session-id"),
            "Upstream request leaked x-cw-session-id"
        );
        for key in req.headers.keys() {
            assert!(
                !key.as_str().starts_with("x-cw-"),
                "Upstream request leaked internal header {key}"
            );
        }
        assert!(
            req.headers.contains_key("authorization"),
            "Upstream request must contain Authorization header"
        );
    }

    // 2. Turn 1 initial request must NOT contain CW112_POLICY_ACTIVE
    let turn1_req = captured
        .iter()
        .find(|r| String::from_utf8_lossy(&r.body).contains("CW112_TASK_TURN_1"))
        .expect("Turn 1 request found");
    assert!(
        !String::from_utf8_lossy(&turn1_req.body).contains("CW112_POLICY_ACTIVE"),
        "Turn 1 request should not contain CW112_POLICY_ACTIVE"
    );

    // 3. Turn 2 and Turn 3 initial requests MUST contain CW112_POLICY_ACTIVE
    let turn2_req = captured
        .iter()
        .find(|r| String::from_utf8_lossy(&r.body).contains("CW112_TASK_TURN_2"))
        .expect("Turn 2 request found");
    assert!(
        String::from_utf8_lossy(&turn2_req.body).contains("CW112_POLICY_ACTIVE"),
        "Turn 2 request must have CW112_POLICY_ACTIVE overlay injected"
    );

    let turn3_req = captured
        .iter()
        .find(|r| String::from_utf8_lossy(&r.body).contains("CW112_TASK_TURN_3"))
        .expect("Turn 3 request found");
    assert!(
        String::from_utf8_lossy(&turn3_req.body).contains("CW112_POLICY_ACTIVE"),
        "Turn 3 request must have CW112_POLICY_ACTIVE overlay injected across second resume"
    );

    // Verify ContextWatcher store integrity
    let sess = f
        .store
        .get_session(&cw_session_id)
        .unwrap()
        .expect("ContextWatcher session recorded");
    assert_eq!(sess.id, cw_session_id);

    let turns = f.store.list_turns(&cw_session_id).unwrap();
    assert!(
        turns.len() >= 3,
        "Expected at least 3 turns recorded in CW store, got {}",
        turns.len()
    );
    for t in &turns {
        assert_eq!(t.session_id, cw_session_id);
    }
    for req in &captured {
        if let Some(json) = &req.json_body {
            assert!(
                validate_protocol_invariants(json, ProviderKind::OpenAI).is_ok(),
                "Captured request payload violates OpenAI protocol invariants"
            );
        }
    }

    let _ = std::fs::remove_dir_all(&temp_home);
    let _ = std::fs::remove_dir_all(&temp_workspace);
}

#[tokio::test]
#[ignore = "harness binary integration test; requires installed 'codex'; run with cargo test --test qualification harness_binary -- --ignored"]
async fn test_harness_binary_codex_cli_unknown_native_session_id_resume() {
    let codex_bin = require_installed_binary("codex");
    let f = QualificationFixture::setup().await;
    let temp_home =
        std::env::temp_dir().join(format!("codex_home_unknown_{}", uuid::Uuid::new_v4()));
    let temp_workspace =
        std::env::temp_dir().join(format!("codex_ws_unknown_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_home).unwrap();
    std::fs::create_dir_all(&temp_workspace).unwrap();

    let output = tokio::task::spawn_blocking({
        let codex_bin = codex_bin.clone();
        let proxy_port = f.proxy_port;
        let token = f.control_token.clone();
        let temp_home = temp_home.clone();
        let temp_workspace = temp_workspace.clone();
        move || {
            let mut cmd = Command::new(codex_bin);
            cmd.current_dir(&temp_workspace);
            cmd.args([
                "exec",
                "resume",
                "--json",
                "--dangerously-bypass-approvals-and-sandbox",
                "--skip-git-repo-check",
                "--ignore-user-config",
            ]);
            cmd.args(cw_harnesses::qualification::codex_proxy_config_args(
                proxy_port,
            ));
            cmd.args([
                "-m",
                "mock-model",
                "00000000-0000-0000-0000-000000000000",
                "resume non-existent session",
            ]);
            cmd.env("OPENAI_API_KEY", "mock-openai-key");
            cmd.env("CODEX_HOME", &temp_home);
            cmd.env("CW_SESSION_ID", "cw_unknown_session_test");
            cmd.env("CW_CONTROL_TOKEN", &token);
            cmd.output()
        }
    })
    .await
    .unwrap()
    .expect("failed to run codex resume");

    let _ = std::fs::remove_dir_all(&temp_home);
    let _ = std::fs::remove_dir_all(&temp_workspace);

    assert!(
        !output.status.success(),
        "Codex should fail when resuming non-existent session ID"
    );
}

#[tokio::test]
#[ignore = "harness binary integration test; requires installed 'codex'; run with cargo test --test qualification harness_binary -- --ignored"]
async fn test_harness_binary_codex_cli_unauthorized_token_rejection() {
    let codex_bin = require_installed_binary("codex");
    let f = QualificationFixture::setup().await;
    let temp_home = std::env::temp_dir().join(format!("codex_home_auth_{}", uuid::Uuid::new_v4()));
    let temp_workspace =
        std::env::temp_dir().join(format!("codex_ws_auth_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_home).unwrap();
    std::fs::create_dir_all(&temp_workspace).unwrap();

    let output = tokio::task::spawn_blocking({
        let codex_bin = codex_bin.clone();
        let proxy_port = f.proxy_port;
        let bad_token = "invalid_cw_control_token_12345";
        let temp_home = temp_home.clone();
        let temp_workspace = temp_workspace.clone();
        move || {
            let mut cmd = Command::new(codex_bin);
            cmd.current_dir(&temp_workspace);
            cmd.args([
                "exec",
                "--json",
                "--dangerously-bypass-approvals-and-sandbox",
                "--skip-git-repo-check",
                "--ignore-user-config",
            ]);
            cmd.args(cw_harnesses::qualification::codex_proxy_config_args(
                proxy_port,
            ));
            cmd.args(["-m", "mock-model", "Test invalid token"]);
            cmd.env("OPENAI_API_KEY", "mock-openai-key");
            cmd.env("CODEX_HOME", &temp_home);
            cmd.env("CW_SESSION_ID", "cw_unauthorized_test");
            cmd.env("CW_CONTROL_TOKEN", bad_token);
            cmd.output()
        }
    })
    .await
    .unwrap()
    .expect("failed to run codex with invalid token");

    let _ = std::fs::remove_dir_all(&temp_home);
    let _ = std::fs::remove_dir_all(&temp_workspace);

    assert!(
        !output.status.success(),
        "Codex should fail when proxy rejects unauthorized token"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stderr.contains("403")
            || stdout.contains("403")
            || stderr.contains("Forbidden")
            || stdout.contains("Forbidden"),
        "Expected HTTP 403 Forbidden on invalid control token, got stdout: '{stdout}', stderr: '{stderr}'"
    );
}

#[tokio::test]
#[ignore = "harness binary integration test; requires installed 'codex'; run with cargo test --test qualification harness_binary -- --ignored"]
async fn test_harness_binary_codex_cli_upstream_failure_handling() {
    let codex_bin = require_installed_binary("codex");
    let f = QualificationFixture::setup().await;
    let temp_home = std::env::temp_dir().join(format!("codex_home_fail_{}", uuid::Uuid::new_v4()));
    let temp_workspace =
        std::env::temp_dir().join(format!("codex_ws_fail_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_home).unwrap();
    std::fs::create_dir_all(&temp_workspace).unwrap();

    let output = tokio::task::spawn_blocking({
        let codex_bin = codex_bin.clone();
        let proxy_port = f.proxy_port;
        let token = f.control_token.clone();
        let temp_home = temp_home.clone();
        let temp_workspace = temp_workspace.clone();
        move || {
            let mut cmd = Command::new(codex_bin);
            cmd.current_dir(&temp_workspace);
            cmd.args([
                "exec",
                "--json",
                "--dangerously-bypass-approvals-and-sandbox",
                "--skip-git-repo-check",
                "--ignore-user-config",
            ]);
            cmd.args(cw_harnesses::qualification::codex_proxy_config_args(
                proxy_port,
            ));
            cmd.args(["-m", "mock-model", "CW_FORCE_UPSTREAM_502"]);
            cmd.env("OPENAI_API_KEY", "mock-openai-key");
            cmd.env("CODEX_HOME", &temp_home);
            cmd.env("CW_SESSION_ID", "cw_upstream_fail_test");
            cmd.env("CW_CONTROL_TOKEN", &token);
            cmd.output()
        }
    })
    .await
    .unwrap()
    .expect("failed to run codex with upstream failure");

    let _ = std::fs::remove_dir_all(&temp_home);
    let _ = std::fs::remove_dir_all(&temp_workspace);

    assert!(
        !output.status.success(),
        "Codex should fail when upstream fails with 502 Bad Gateway"
    );

    // Verify proxy store remains uncorrupted
    assert!(
        f.store.list_turns("cw_upstream_fail_test").is_ok(),
        "CW store must remain consistent and accessible after upstream error"
    );
}

#[tokio::test]
#[ignore = "harness binary integration test; requires installed 'codex'; run with cargo test --test qualification harness_binary -- --ignored"]
async fn test_harness_binary_codex_cli_session_isolation() {
    let codex_bin = require_installed_binary("codex");
    let f = QualificationFixture::setup().await;
    let proxy_port = f.proxy_port;
    let token = f.control_token.clone();

    let sid_a = format!("cw_codex_sess_a_{}", uuid::Uuid::new_v4());
    let sid_b = format!("cw_codex_sess_b_{}", uuid::Uuid::new_v4());

    // 1. Run Session A Turn 1 to establish the session in ContextWatcher store
    let temp_home_a = std::env::temp_dir().join(format!("codex_home_a_{}", uuid::Uuid::new_v4()));
    let temp_ws_a = std::env::temp_dir().join(format!("codex_ws_a_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_home_a).unwrap();
    std::fs::create_dir_all(&temp_ws_a).unwrap();

    let output_a1 = tokio::task::spawn_blocking({
        let codex_bin = codex_bin.clone();
        let sid_a = sid_a.clone();
        let token = token.clone();
        let temp_home_a = temp_home_a.clone();
        let temp_ws_a = temp_ws_a.clone();
        move || {
            let mut cmd = Command::new(codex_bin);
            cmd.current_dir(&temp_ws_a);
            cmd.args([
                "exec",
                "--ephemeral",
                "--skip-git-repo-check",
                "--ignore-user-config",
            ]);
            cmd.args(cw_harnesses::qualification::codex_proxy_config_args(
                proxy_port,
            ));
            cmd.args(["-m", "mock-model", "Session A turn 1"]);
            cmd.env("OPENAI_API_KEY", "mock-openai-key");
            cmd.env("CODEX_HOME", &temp_home_a);
            cmd.env("CW_SESSION_ID", &sid_a);
            cmd.env("CW_CONTROL_TOKEN", &token);
            cmd.output()
        }
    })
    .await
    .unwrap()
    .expect("failed to run codex binary for session A turn 1");

    assert!(output_a1.status.success());

    // 2. Commit pin ONLY to Session A
    f.store
        .save_mutation(&MutationRule {
            id: "session_a_isolated_pin".into(),
            session_id: sid_a.clone(),
            op: MutationOp::PinContext {
                label: "Secret".into(),
                content: "CW112_SESSION_A_SECRET_OVERLAY".into(),
                source_node_id: None,
            },
            status: MutationStatus::Committed,
            estimated_saved_tokens: 0,
            estimated_kv_break_tokens: 0,
            created_at: Utc::now(),
        })
        .unwrap();

    // 3. Run Session B Turn 1 with independent session ID and CODEX_HOME
    let temp_home_b = std::env::temp_dir().join(format!("codex_home_b_{}", uuid::Uuid::new_v4()));
    let temp_ws_b = std::env::temp_dir().join(format!("codex_ws_b_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_home_b).unwrap();
    std::fs::create_dir_all(&temp_ws_b).unwrap();

    let output_b = tokio::task::spawn_blocking({
        let codex_bin = codex_bin.clone();
        let sid_b = sid_b.clone();
        let token = token.clone();
        let temp_home_b = temp_home_b.clone();
        let temp_ws_b = temp_ws_b.clone();
        move || {
            let mut cmd = Command::new(codex_bin);
            cmd.current_dir(&temp_ws_b);
            cmd.args([
                "exec",
                "--ephemeral",
                "--skip-git-repo-check",
                "--ignore-user-config",
            ]);
            cmd.args(cw_harnesses::qualification::codex_proxy_config_args(
                proxy_port,
            ));
            cmd.args(["-m", "mock-model", "Session B turn 1"]);
            cmd.env("OPENAI_API_KEY", "mock-openai-key");
            cmd.env("CODEX_HOME", &temp_home_b);
            cmd.env("CW_SESSION_ID", &sid_b);
            cmd.env("CW_CONTROL_TOKEN", &token);
            cmd.output()
        }
    })
    .await
    .unwrap()
    .expect("failed to run codex binary for session B");

    let _ = std::fs::remove_dir_all(&temp_home_b);
    let _ = std::fs::remove_dir_all(&temp_ws_b);
    assert!(output_b.status.success());

    // 4. Run Session A Turn 2
    let output_a2 = tokio::task::spawn_blocking({
        let codex_bin = codex_bin.clone();
        let sid_a = sid_a.clone();
        let token = token.clone();
        let temp_home_a = temp_home_a.clone();
        let temp_ws_a = temp_ws_a.clone();
        move || {
            let mut cmd = Command::new(codex_bin);
            cmd.current_dir(&temp_ws_a);
            cmd.args([
                "exec",
                "--ephemeral",
                "--skip-git-repo-check",
                "--ignore-user-config",
            ]);
            cmd.args(cw_harnesses::qualification::codex_proxy_config_args(
                proxy_port,
            ));
            cmd.args(["-m", "mock-model", "Session A turn 2"]);
            cmd.env("OPENAI_API_KEY", "mock-openai-key");
            cmd.env("CODEX_HOME", &temp_home_a);
            cmd.env("CW_SESSION_ID", &sid_a);
            cmd.env("CW_CONTROL_TOKEN", &token);
            cmd.output()
        }
    })
    .await
    .unwrap()
    .expect("failed to run codex binary for session A turn 2");

    let _ = std::fs::remove_dir_all(&temp_home_a);
    let _ = std::fs::remove_dir_all(&temp_ws_a);
    assert!(output_a2.status.success());

    let captured = f.mock.captured.lock().unwrap().clone();
    let req_b = captured
        .iter()
        .find(|r| String::from_utf8_lossy(&r.body).contains("Session B turn 1"))
        .expect("Session B request found");
    assert!(
        !String::from_utf8_lossy(&req_b.body).contains("CW112_SESSION_A_SECRET_OVERLAY"),
        "Session B must NOT receive Session A's overlay mutation!"
    );

    let req_a2 = captured
        .iter()
        .find(|r| String::from_utf8_lossy(&r.body).contains("Session A turn 2"))
        .expect("Session A turn 2 request found");
    assert!(
        String::from_utf8_lossy(&req_a2.body).contains("CW112_SESSION_A_SECRET_OVERLAY"),
        "Session A MUST receive its committed overlay mutation!"
    );
}

fn parse_json_string_array(s: &str) -> Option<Vec<String>> {
    let trimmed = s.trim();
    if let Ok(arr) = serde_json::from_str::<Vec<String>>(trimmed) {
        return Some(arr);
    }
    let start = s.find('[')?;
    let end = s.rfind(']')?;
    if start < end {
        let slice = &s[start..=end];
        if let Ok(arr) = serde_json::from_str::<Vec<String>>(slice) {
            return Some(arr);
        }
    }
    None
}

#[tokio::test]
#[ignore = "harness binary integration test; requires installed 'hermes'; run with cargo test --test qualification harness_binary -- --ignored"]
async fn test_harness_binary_hermes_middleware_session_binding() {
    let hermes_bin = require_installed_binary("hermes");

    let f = QualificationFixture::setup().await;
    let proxy_port = f.proxy_port;
    let control_token = f.control_token.clone();

    // Create temporary control token file with 0600 mode
    let temp_token_file = std::env::temp_dir().join(format!("cw_token_{}", uuid::Uuid::new_v4()));
    std::fs::write(&temp_token_file, &control_token).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temp_token_file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let token_path_str = temp_token_file.to_string_lossy().to_string();

    let temp_override_dir =
        std::env::temp_dir().join(format!("cw_hermes_overrides_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_override_dir).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temp_override_dir, std::fs::Permissions::from_mode(0o700))
            .unwrap();
    }

    // Capture original Hermes client_env_passthrough to preserve existing entries and restore on completion
    let output = Command::new(&hermes_bin)
        .args(["config", "get", "--json", "terminal.client_env_passthrough"])
        .output()
        .expect("failed to execute hermes config get");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    let (original_json, existing_vars): (Option<String>, Vec<String>) = if !output.status.success()
    {
        if stdout.contains("Config key not set") || stderr.contains("Config key not set") {
            (None, Vec::new())
        } else {
            panic!("hermes config get failed unexpectedly: {stderr} {stdout}");
        }
    } else {
        let vars = parse_json_string_array(&stdout)
            .unwrap_or_else(|| panic!("failed to parse hermes config get output: {stdout}"));
        let start = stdout.find('[').expect("JSON array start [");
        let end = stdout.rfind(']').expect("JSON array end ]");
        (Some(stdout[start..=end].to_string()), vars)
    };

    struct HermesConfigGuard {
        hermes_bin: PathBuf,
        original_config: Option<String>,
        needed_restore: bool,
    }

    impl Drop for HermesConfigGuard {
        fn drop(&mut self) {
            if self.needed_restore {
                if let Some(ref orig) = self.original_config {
                    let _ = Command::new(&self.hermes_bin)
                        .args(["config", "set", "terminal.client_env_passthrough", orig])
                        .status();
                } else {
                    let _ = Command::new(&self.hermes_bin)
                        .args(["config", "unset", "terminal.client_env_passthrough"])
                        .status();
                }
            }
        }
    }

    let mut _config_guard = HermesConfigGuard {
        hermes_bin: hermes_bin.clone(),
        original_config: original_json,
        needed_restore: false,
    };

    let mut updated = existing_vars.clone();
    let mut changed = false;
    for var in &[
        "CW_HERMES_OVERRIDE_FILE",
        "CW_SESSION_ID",
        "CW_CONTROL_TOKEN_FILE",
        "CW_WORKSPACE",
        "CW_HERMES_TRUSTED_DIR",
    ] {
        if !updated.iter().any(|v| v == *var) {
            updated.push(var.to_string());
            changed = true;
        }
    }

    if changed {
        // Arm restoration guard BEFORE executing config set so failures or panics trigger cleanup
        _config_guard.needed_restore = true;
        let merged_json = serde_json::to_string(&updated).unwrap();
        let status = Command::new(&hermes_bin)
            .args([
                "config",
                "set",
                "terminal.client_env_passthrough",
                &merged_json,
            ])
            .status()
            .expect("hermes config set failed to execute");
        assert!(status.success(), "hermes config set must succeed");
    }

    // -------------------------------------------------------------------------
    // Gate 1 & 2 & 3: Concurrent Launches, Isolated Handoff Files & Overlay Isolation
    // -------------------------------------------------------------------------
    let sess_id_a = format!(
        "hermes_concurrent_sess_A_{}",
        &uuid::Uuid::new_v4().to_string()[..8]
    );
    let sess_id_b = format!(
        "hermes_concurrent_sess_B_{}",
        &uuid::Uuid::new_v4().to_string()[..8]
    );

    let now = chrono::Utc::now();
    for (sid, policy_label, policy_content) in [
        (
            &sess_id_a,
            "PolicyA",
            "CRITICAL_POLICY_A_ONLY: STRICT_ISOLATION_A",
        ),
        (
            &sess_id_b,
            "PolicyB",
            "CRITICAL_POLICY_B_ONLY: STRICT_ISOLATION_B",
        ),
    ] {
        f.store
            .upsert_session(&Session {
                id: sid.to_string(),
                parent_session_id: None,
                harness: HarnessKind::Hermes,
                workspace_root: None,
                model: "mock-model".to_string(),
                provider: ProviderKind::OpenAI,
                title: None,
                created_at: now,
                updated_at: now,
            })
            .unwrap();
        f.store
            .save_mutation(&MutationRule {
                id: format!("mut_{policy_label}"),
                session_id: sid.to_string(),
                op: MutationOp::PinContext {
                    label: policy_label.to_string(),
                    content: policy_content.to_string(),
                    source_node_id: None,
                },
                status: MutationStatus::Committed,
                estimated_saved_tokens: 0,
                estimated_kv_break_tokens: 0,
                created_at: now,
            })
            .unwrap();
    }

    let override_uuid_a = uuid::Uuid::new_v4().to_string();
    let override_path_a = temp_override_dir.join(format!("hermes-override-{override_uuid_a}.json"));
    let override_uuid_b = uuid::Uuid::new_v4().to_string();
    let override_path_b = temp_override_dir.join(format!("hermes-override-{override_uuid_b}.json"));

    let create_override = |path: &std::path::Path, sid: &str| {
        let payload = serde_json::json!({
            "session_id": sid,
            "workspace": "/tmp/hermes_workspace",
            "ts": chrono::Utc::now().timestamp(),
        });
        let bytes = serde_json::to_vec(&payload).unwrap();
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(path).unwrap();
        use std::io::Write;
        f.write_all(&bytes).unwrap();
    };

    create_override(&override_path_a, &sess_id_a);
    create_override(&override_path_b, &sess_id_b);

    let hermes_bin_a = hermes_bin.clone();
    let token_path_a = token_path_str.clone();
    let override_file_a_str = override_path_a.to_string_lossy().to_string();
    let trusted_dir_a = temp_override_dir.to_string_lossy().to_string();

    let task_a = tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new(hermes_bin_a);
        cmd.args([
            "chat",
            "-Q",
            "-q",
            "Prompt from Hermes Process A",
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
        cmd.env("CW_CONTROL_TOKEN_FILE", token_path_a);
        cmd.env("CW_HERMES_OVERRIDE_FILE", override_file_a_str);
        cmd.env("CW_HERMES_TRUSTED_DIR", trusted_dir_a);
        cmd.output()
    });

    let hermes_bin_b = hermes_bin.clone();
    let token_path_b = token_path_str.clone();
    let override_file_b_str = override_path_b.to_string_lossy().to_string();
    let trusted_dir_b = temp_override_dir.to_string_lossy().to_string();

    let task_b = tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new(hermes_bin_b);
        cmd.args([
            "chat",
            "-Q",
            "-q",
            "Prompt from Hermes Process B",
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
        cmd.env("CW_CONTROL_TOKEN_FILE", token_path_b);
        cmd.env("CW_HERMES_OVERRIDE_FILE", override_file_b_str);
        cmd.env("CW_HERMES_TRUSTED_DIR", trusted_dir_b);
        cmd.output()
    });

    let (out_a, out_b) = tokio::join!(task_a, task_b);
    let output_a = out_a.unwrap().expect("hermes A failed");
    let output_b = out_b.unwrap().expect("hermes B failed");

    let stdout_a = String::from_utf8_lossy(&output_a.stdout);
    let stderr_a = String::from_utf8_lossy(&output_a.stderr);
    let stdout_b = String::from_utf8_lossy(&output_b.stdout);
    let stderr_b = String::from_utf8_lossy(&output_b.stderr);

    eprintln!("Hermes A stdout: {stdout_a}");
    eprintln!("Hermes A stderr: {stderr_a}");
    eprintln!("Hermes B stdout: {stdout_b}");
    eprintln!("Hermes B stderr: {stderr_b}");

    let captured = f.mock.captured.lock().unwrap().clone();
    for (i, c) in captured.iter().enumerate() {
        eprintln!(
            "Captured {i}: headers={:?} body={}",
            c.headers,
            String::from_utf8_lossy(&c.body)
        );
    }

    assert_eq!(
        output_a.status.code(),
        Some(0),
        "Hermes A failed: {stderr_a}"
    );
    assert_eq!(
        output_b.status.code(),
        Some(0),
        "Hermes B failed: {stderr_b}"
    );

    // Gate 1: Assert override files were consumed and unlinked immediately on read
    assert!(
        !override_path_a.exists(),
        "Override file A must be unlinked after consumption"
    );
    assert!(
        !override_path_b.exists(),
        "Override file B must be unlinked after consumption"
    );

    // Extract native Hermes session IDs
    let native_sid_a = stderr_a
        .lines()
        .find_map(|l| l.strip_prefix("session_id: "))
        .map(str::trim)
        .expect("Hermes A must output session_id");
    let native_sid_b = stderr_b
        .lines()
        .find_map(|l| l.strip_prefix("session_id: "))
        .map(str::trim)
        .expect("Hermes B must output session_id");

    assert_ne!(
        native_sid_a, native_sid_b,
        "Processes must have distinct native IDs"
    );

    // Gate 3: Upstream Overlay Isolation
    let captured = f.mock.captured.lock().unwrap().clone();
    assert!(captured.len() >= 2, "Expected at least 2 captured requests");

    let req_a = captured
        .iter()
        .find(|c| String::from_utf8_lossy(&c.body).contains("Prompt from Hermes Process A"))
        .expect("Request from Process A captured");
    let req_b = captured
        .iter()
        .find(|c| String::from_utf8_lossy(&c.body).contains("Prompt from Hermes Process B"))
        .expect("Request from Process B captured");

    let body_a = String::from_utf8_lossy(&req_a.body);
    let body_b = String::from_utf8_lossy(&req_b.body);

    assert!(
        body_a.contains("CRITICAL_POLICY_A_ONLY"),
        "Request A missing Policy A: {body_a}"
    );
    assert!(
        !body_a.contains("CRITICAL_POLICY_B_ONLY"),
        "CROSSOVER DETECTED: Request A contains Policy B: {body_a}"
    );

    assert!(
        body_b.contains("CRITICAL_POLICY_B_ONLY"),
        "Request B missing Policy B: {body_b}"
    );
    assert!(
        !body_b.contains("CRITICAL_POLICY_A_ONLY"),
        "CROSSOVER DETECTED: Request B contains Policy A: {body_b}"
    );

    let turns_a = f.store.list_turns(&sess_id_a).unwrap();
    let turns_b = f.store.list_turns(&sess_id_b).unwrap();
    assert_eq!(turns_a.len(), 1, "Session A must have exactly 1 turn");
    assert_eq!(turns_b.len(), 1, "Session B must have exactly 1 turn");

    // -------------------------------------------------------------------------
    // Gate 4: Resume Gate (Native resume continues under original session identity)
    // -------------------------------------------------------------------------
    let hermes_bin_resume_a = hermes_bin.clone();
    let token_path_resume_a = token_path_str.clone();
    let native_sid_a_str = native_sid_a.to_string();

    let output_resume_a = tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new(hermes_bin_resume_a);
        cmd.args([
            "chat",
            "-Q",
            "-r",
            &native_sid_a_str,
            "-q",
            "Resume follow-up prompt from A",
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
        cmd.env("CW_CONTROL_TOKEN_FILE", token_path_resume_a);
        cmd.output()
    })
    .await
    .unwrap()
    .expect("Hermes resume A failed");

    assert_eq!(output_resume_a.status.code(), Some(0));
    let turns_a2 = f.store.list_turns(&sess_id_a).unwrap();
    assert_eq!(
        turns_a2.len(),
        2,
        "Session A must have 2 turns after resume"
    );

    let captured_resume = f.mock.captured.lock().unwrap().clone();
    let req_resume_a = captured_resume
        .iter()
        .find(|c| String::from_utf8_lossy(&c.body).contains("Resume follow-up prompt from A"))
        .expect("Resume request captured");
    let body_resume_a = String::from_utf8_lossy(&req_resume_a.body);
    assert!(
        body_resume_a.contains("CRITICAL_POLICY_A_ONLY"),
        "Resumed A missing Policy A"
    );
    assert!(
        !body_resume_a.contains("CRITICAL_POLICY_B_ONLY"),
        "CROSSOVER: Resumed A contains Policy B"
    );

    // -------------------------------------------------------------------------
    // Gate 5: Invalid Handoff & Crash Recovery Gate (Fails safe without fallback)
    // -------------------------------------------------------------------------
    let hermes_bin_invalid = hermes_bin.clone();
    let token_path_invalid = token_path_str.clone();
    let missing_override_file = temp_override_dir.join("hermes-override-nonexistent.json");
    let trusted_dir_invalid = temp_override_dir.to_string_lossy().to_string();

    let output_invalid = tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new(hermes_bin_invalid);
        cmd.args([
            "chat",
            "-Q",
            "-q",
            "Prompt with invalid override file",
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
        cmd.env("CW_CONTROL_TOKEN_FILE", token_path_invalid);
        cmd.env(
            "CW_HERMES_OVERRIDE_FILE",
            missing_override_file.to_string_lossy().to_string(),
        );
        cmd.env("CW_HERMES_TRUSTED_DIR", trusted_dir_invalid);
        cmd.output()
    })
    .await
    .unwrap()
    .expect("Hermes invalid handoff execution failed");

    assert_eq!(output_invalid.status.code(), Some(0));
    let captured_after_invalid = f.mock.captured.lock().unwrap().clone();
    let last_req = captured_after_invalid
        .last()
        .expect("last request captured");
    let last_body = String::from_utf8_lossy(&last_req.body);
    assert!(
        !last_body.contains("CRITICAL_POLICY_A_ONLY"),
        "Must not bind to session A"
    );
    assert!(
        !last_body.contains("CRITICAL_POLICY_B_ONLY"),
        "Must not bind to session B"
    );

    // -------------------------------------------------------------------------
    // Gate 6: Credential Stripping Gate
    // -------------------------------------------------------------------------
    for cap in &captured_after_invalid {
        assert!(!cap.headers.contains_key("x-cw-control-token"));
        assert!(!cap.headers.contains_key("x-cw-session-id"));
    }

    // -------------------------------------------------------------------------
    // Gate 7: 20-Session Stress Test
    // -------------------------------------------------------------------------
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .unwrap();
    let test_plugin_script = repo_root.join("plugins/hermes/context-watcher/test_plugin.py");
    let py_test = Command::new("python3")
        .arg(&test_plugin_script)
        .current_dir(repo_root)
        .env("CW_HERMES_TRUSTED_DIR", &temp_override_dir)
        .output()
        .expect("failed to run python test_plugin.py");
    assert!(
        py_test.status.success(),
        "Python 20-session stress test failed: {}",
        String::from_utf8_lossy(&py_test.stderr)
    );

    let _ = std::fs::remove_file(&temp_token_file);
    let _ = std::fs::remove_dir_all(&temp_override_dir);
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
#[ignore = "opt-in real Codex qualification; requires CW_QUALIFY_REAL_CODEX=1 and OPENAI_API_KEY with authorized budget"]
async fn test_live_continuation_real_codex_multiturn() {
    let opt_in = std::env::var("CW_QUALIFY_REAL_CODEX").unwrap_or_default();
    if opt_in != "1" {
        panic!(
            "SKIPPED: Live Codex qualification requires explicit opt-in with CW_QUALIFY_REAL_CODEX=1 and authorized live spend budget"
        );
    }
    let api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY required for live test");
    let codex_bin = require_installed_binary("codex");

    let store = CwStore::open_in_memory().unwrap();
    store
        .set_autopilot_config(&cw_core::autopilot::AutoPilotConfig {
            enabled: true,
            ..Default::default()
        })
        .unwrap();
    let mut state = DaemonState::new(store.clone());
    state.openai_upstream = "https://api.openai.com".to_string();
    state.capture_wire_egress = true;
    let control_token = state.control_token.clone();

    let app = build_proxy_router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_port = listener.local_addr().unwrap().port();
    let proxy_task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let session_id = "codex_real_live_qual";
    let temp_home = std::env::temp_dir().join(format!("codex_real_home_{}", uuid::Uuid::new_v4()));
    let temp_workspace =
        std::env::temp_dir().join(format!("codex_real_ws_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_home).unwrap();
    std::fs::create_dir_all(&temp_workspace).unwrap();

    let temp_home_clone = temp_home.clone();
    let temp_ws_clone = temp_workspace.clone();
    let sid = session_id.to_string();
    let token = control_token.clone();

    // Turn 1: Create calculator.py
    let output1 = tokio::task::spawn_blocking({
        let codex_bin = codex_bin.clone();
        let api_key = api_key.clone();
        let temp_home_clone = temp_home_clone.clone();
        let temp_ws_clone = temp_ws_clone.clone();
        let sid = sid.clone();
        let token = token.clone();
        move || {
            let mut cmd = Command::new(codex_bin);
            cmd.current_dir(&temp_ws_clone);
            cmd.args([
                "exec",
                "--json",
                "--skip-git-repo-check",
                "--ignore-user-config",
                "-c",
                "approval_policy=\"never\"",
                "-c",
                "sandbox_mode=\"workspace-write\"",
            ]);
            cmd.args(cw_harnesses::qualification::codex_proxy_config_args(
                proxy_port,
            ));
            cmd.args([
                "-m",
                "gpt-4o-mini",
                "Create a python file calculator.py with a function add(a, b) that returns a + b.",
            ]);
            cmd.stdin(std::process::Stdio::null());
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

    let stdout1 = String::from_utf8_lossy(&output1.stdout);
    assert_eq!(
        output1.status.code(),
        Some(0),
        "Turn 1 failed: {}",
        String::from_utf8_lossy(&output1.stderr)
    );

    let thread_id = extract_thread_id_from_stdout(&stdout1)
        .expect("Codex must emit thread.started with thread_id on live turn 1");

    // Correlate and verify all model requests generated during Turn 1 invocation
    let all_turns_after_turn1 = store.list_turns(session_id).unwrap();
    assert!(
        !all_turns_after_turn1.is_empty(),
        "Expected at least one model turn recorded for live Codex Turn 1"
    );
    for t in &all_turns_after_turn1 {
        assert_eq!(
            t.outcome.as_deref(),
            Some("completed"),
            "Turn 1 provider request must complete successfully at egress"
        );
        assert!(
            t.observed_usage.is_some(),
            "Turn 1 must record provider-returned token usage"
        );
        // Egress byte assertion: captured at forwarding boundary
        if let Some(egress) = store.get_wire_egress(&t.id).unwrap() {
            let egress_str = String::from_utf8_lossy(&egress);
            assert!(
                !egress_str.contains("CW112_POLICY_ACTIVE"),
                "Turn 1 egress request must not contain overlay before commitment"
            );
        }
        // Prepared outbound payload assertion
        let payload = store
            .get_turn_mutated_payload(&t.id)
            .unwrap()
            .or_else(|| store.get_turn_raw_payload(&t.id).unwrap())
            .expect("Turn 1 payload recorded in store");
        assert!(
            !serde_json::to_string(&payload)
                .unwrap()
                .contains("CW112_POLICY_ACTIVE"),
            "Turn 1 prepared outbound request must not contain overlay before commitment"
        );
    }

    // Commit overlay mutation on live session
    store
        .save_mutation(&MutationRule {
            id: "live_pin_rule".into(),
            session_id: sid.clone(),
            op: MutationOp::PinContext {
                label: "Directive".into(),
                content: "CW112_POLICY_ACTIVE".into(),
                source_node_id: None,
            },
            status: MutationStatus::Committed,
            estimated_saved_tokens: 0,
            estimated_kv_break_tokens: 0,
            created_at: Utc::now(),
        })
        .unwrap();

    // Turn 2: Native Resume to add multiply function
    let output2 = tokio::task::spawn_blocking({
        let codex_bin = codex_bin.clone();
        let api_key = api_key.clone();
        let temp_home_clone = temp_home_clone.clone();
        let temp_ws_clone = temp_ws_clone.clone();
        let sid = sid.clone();
        let token = token.clone();
        let thread_id = thread_id.clone();
        move || {
            let mut cmd = Command::new(codex_bin);
            cmd.current_dir(&temp_ws_clone);
            cmd.args([
                "exec",
                "resume",
                "--json",
                "--skip-git-repo-check",
                "--ignore-user-config",
                "-c",
                "approval_policy=\"never\"",
                "-c",
                "sandbox_mode=\"workspace-write\"",
            ]);
            cmd.args(cw_harnesses::qualification::codex_proxy_config_args(
                proxy_port,
            ));
            cmd.args([
                "-m",
                "gpt-4o-mini",
                &thread_id,
                "Add a function multiply(a, b) that returns a * b to calculator.py.",
            ]);
            cmd.stdin(std::process::Stdio::null());
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

    assert_eq!(
        output2.status.code(),
        Some(0),
        "Live Codex turn 2 failed: {stderr2}"
    );

    let thread_id_2 = extract_thread_id_from_stdout(&stdout2)
        .expect("Codex must emit thread.started with thread_id on live turn 2");
    assert_eq!(
        thread_id_2, thread_id,
        "Native thread ID must match across live resume"
    );

    // Correlate and verify all model requests generated during Turn 2 invocation
    let count_before_turn2 = all_turns_after_turn1.len();
    let all_turns_after_turn2 = store.list_turns(session_id).unwrap();
    let turn2_turns = all_turns_after_turn2[count_before_turn2..].to_vec();
    assert!(
        !turn2_turns.is_empty(),
        "Expected at least one model turn recorded for live Codex Turn 2"
    );
    for t in &turn2_turns {
        assert_eq!(
            t.outcome.as_deref(),
            Some("completed"),
            "Turn 2 provider request must complete successfully at egress"
        );
        assert!(
            t.observed_usage.is_some(),
            "Turn 2 must record provider-returned token usage"
        );
    }
    // Egress wire byte assertion: proves the bytes were actually transmitted to upstream
    let turn2_egress_has_overlay = turn2_turns.iter().any(|t| {
        store
            .get_wire_egress(&t.id)
            .unwrap()
            .map(|b| String::from_utf8_lossy(&b).contains("CW112_POLICY_ACTIVE"))
            .unwrap_or(false)
    });
    assert!(
        turn2_egress_has_overlay,
        "Turn 2 egress request sent to provider must contain committed overlay"
    );
    // Prepared outbound payload assertion
    let turn2_payload_has_overlay = turn2_turns.iter().any(|t| {
        store
            .get_turn_mutated_payload(&t.id)
            .unwrap()
            .map(|p| {
                serde_json::to_string(&p)
                    .unwrap()
                    .contains("CW112_POLICY_ACTIVE")
            })
            .unwrap_or(false)
    });
    assert!(
        turn2_payload_has_overlay,
        "Turn 2 prepared outbound request must contain committed overlay"
    );

    // Turn 3: Second Native Resume to create and run tests
    let output3 = tokio::task::spawn_blocking({
        let codex_bin = codex_bin.clone();
        let api_key = api_key.clone();
        let temp_home_clone = temp_home_clone.clone();
        let temp_ws_clone = temp_ws_clone.clone();
        let sid = sid.clone();
        let token = token.clone();
        let thread_id = thread_id.clone();
        move || {
            let mut cmd = Command::new(codex_bin);
            cmd.current_dir(&temp_ws_clone);
            cmd.args([
                "exec",
                "resume",
                "--json",
                "--skip-git-repo-check",
                "--ignore-user-config",
                "-c",
                "approval_policy=\"never\"",
                "-c",
                "sandbox_mode=\"workspace-write\"",
            ]);
            cmd.args(cw_harnesses::qualification::codex_proxy_config_args(
                proxy_port,
            ));
            cmd.args([
                "-m",
                "gpt-4o-mini",
                &thread_id,
                "Create a test file test_calculator.py with unit tests for add and multiply, and run python3 test_calculator.py.",
            ]);
            cmd.stdin(std::process::Stdio::null());
            cmd.env("OPENAI_API_KEY", api_key);
            cmd.env("CODEX_HOME", &temp_home_clone);
            cmd.env("CW_SESSION_ID", sid);
            cmd.env("CW_CONTROL_TOKEN", token);
            cmd.output()
        }
    })
    .await
    .unwrap()
    .expect("failed to run codex binary on live turn 3");

    let stdout3 = String::from_utf8_lossy(&output3.stdout);
    let stderr3 = String::from_utf8_lossy(&output3.stderr);

    assert_eq!(
        output3.status.code(),
        Some(0),
        "Live Codex turn 3 failed: {stderr3}"
    );

    let thread_id_3 = extract_thread_id_from_stdout(&stdout3)
        .expect("Codex must emit thread.started with thread_id on live turn 3");
    assert_eq!(
        thread_id_3, thread_id,
        "Native thread ID must match across second live resume"
    );

    // Correlate and verify all model requests generated during Turn 3 invocation
    let count_before_turn3 = all_turns_after_turn2.len();
    let all_turns_after_turn3 = store.list_turns(session_id).unwrap();
    let turn3_turns = all_turns_after_turn3[count_before_turn3..].to_vec();
    assert!(
        !turn3_turns.is_empty(),
        "Expected at least one model turn recorded for live Codex Turn 3"
    );
    for t in &turn3_turns {
        assert_eq!(
            t.outcome.as_deref(),
            Some("completed"),
            "Turn 3 provider request must complete successfully at egress"
        );
        assert!(
            t.observed_usage.is_some(),
            "Turn 3 must record provider-returned token usage"
        );
    }
    // Egress wire byte assertion: proves the bytes were actually transmitted to upstream
    let turn3_egress_has_overlay = turn3_turns.iter().any(|t| {
        store
            .get_wire_egress(&t.id)
            .unwrap()
            .map(|b| String::from_utf8_lossy(&b).contains("CW112_POLICY_ACTIVE"))
            .unwrap_or(false)
    });
    assert!(
        turn3_egress_has_overlay,
        "Turn 3 egress request sent to provider must retain committed overlay across resume"
    );
    // Prepared outbound payload assertion
    let turn3_payload_has_overlay = turn3_turns.iter().any(|t| {
        store
            .get_turn_mutated_payload(&t.id)
            .unwrap()
            .map(|p| {
                serde_json::to_string(&p)
                    .unwrap()
                    .contains("CW112_POLICY_ACTIVE")
            })
            .unwrap_or(false)
    });
    assert!(
        turn3_payload_has_overlay,
        "Turn 3 prepared outbound request must retain committed overlay across resume"
    );

    // Verify on-disk rollout files are valid JSONL and do not contain injected overlay
    let rollouts = find_rollout_files(&temp_home.join("sessions"));
    assert!(
        !rollouts.is_empty(),
        "CODEX_HOME/sessions must contain rollout files"
    );
    for rf in &rollouts {
        let content = std::fs::read_to_string(rf).expect("read rollout file");
        for line in content.lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                assert!(
                    serde_json::from_str::<Value>(trimmed).is_ok(),
                    "Rollout line in {} must be valid JSON",
                    rf.display()
                );
            }
        }
        assert!(
            !content.contains("CW112_POLICY_ACTIVE"),
            "Rollout file on disk must not contain injected overlay: {}",
            rf.display()
        );
    }

    let _ = std::fs::remove_dir_all(&temp_home);
    let _ = std::fs::remove_dir_all(&temp_workspace);
    proxy_task.abort();

    let sess = store
        .get_session(session_id)
        .unwrap()
        .expect("session recorded");
    assert_eq!(sess.id, session_id);
    let turns = store.list_turns(session_id).unwrap();
    assert!(
        turns.len() >= 3,
        "Expected at least 3 turns recorded for live codex session, got {}",
        turns.len()
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
    store
        .set_autopilot_config(&cw_core::autopilot::AutoPilotConfig {
            enabled: true,
            ..Default::default()
        })
        .unwrap();
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
    store
        .set_autopilot_config(&cw_core::autopilot::AutoPilotConfig {
            enabled: true,
            ..Default::default()
        })
        .unwrap();
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
