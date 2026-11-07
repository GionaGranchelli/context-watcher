use axum::{
    Json, Router,
    body::Body,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::Utc;
use cw_core::{
    ir::*,
    normalizer::parse_request_to_nodes,
    spend_guard::{SpendGuard, SpendGuardConfig},
    validator::validate_protocol_invariants,
};
use cw_proxy::{DaemonState, build_proxy_router};
use cw_store::CwStore;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::PathBuf,
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

#[derive(Debug, Clone)]
struct CapturedRequest {
    path: String,
    headers: HeaderMap,
    body: Vec<u8>,
    json_body: Option<Value>,
}

#[derive(Debug, Clone)]
struct CapturedResponse {
    status: u16,
    body: Vec<u8>,
}

struct MockProvider {
    url: String,
    #[allow(dead_code)]
    port: u16,
    captured: Arc<Mutex<Vec<CapturedRequest>>>,
    responses: Arc<Mutex<Vec<CapturedResponse>>>,
    redirect_target: Arc<Mutex<Option<String>>>,
    server_task: tokio::task::JoinHandle<()>,
}

impl MockProvider {
    async fn start() -> Self {
        let captured = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
        let captured_clone = captured.clone();
        let responses = Arc::new(Mutex::new(Vec::<CapturedResponse>::new()));
        let responses_clone = responses.clone();
        let redirect_target = Arc::new(Mutex::new(None::<String>));
        let redirect_target_clone = redirect_target.clone();

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
                    let responses = responses_clone.clone();
                    let redirect_target = redirect_target_clone.clone();
                    move |headers: HeaderMap, body: axum::body::Bytes| {
                        let captured = captured.clone();
                        let responses = responses.clone();
                        let redirect_target = redirect_target.clone();
                        async move {
                            let response = if headers.contains_key("x-test-redirect") {
                                captured.lock().unwrap().push(CapturedRequest {
                                    path: "/v1/responses".into(),
                                    headers: headers.clone(),
                                    body: body.to_vec(),
                                    json_body: serde_json::from_slice(&body).ok(),
                                });
                                let target = redirect_target.lock().unwrap().clone().unwrap();
                                Response::builder()
                                    .status(StatusCode::TEMPORARY_REDIRECT)
                                    .header("location", target)
                                    .body(Body::empty())
                                    .unwrap()
                            } else {
                                handle_mock_responses(headers, body, captured).await
                            };
                            let (parts, body) = response.into_parts();
                            let bytes = axum::body::to_bytes(body, 16 * 1024 * 1024)
                                .await
                                .expect("mock Responses body is bounded");
                            responses.lock().unwrap().push(CapturedResponse {
                                status: parts.status.as_u16(),
                                body: bytes.to_vec(),
                            });
                            Response::from_parts(parts, Body::from(bytes))
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
            responses,
            redirect_target,
            server_task,
        }
    }
}

impl Drop for MockProvider {
    fn drop(&mut self) {
        self.server_task.abort();
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

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn cw122b1_diagnostic_command(family: &str) -> String {
    let script = if family == "f1" {
        "import os; from retry import retry_delays; cases=[(5,0.25,2.0),(3,1,4),(-1,1,4)]; [print((a,b,c), retry_delays(a,b,c)) for a,b,c in cases]; print('secret-env-present', any(k in os.environ for k in ('CW_CONTROL_TOKEN','OPENAI_API_KEY','HTTP_PROXY','HTTPS_PROXY','ALL_PROXY')))"
    } else {
        "import os; from storage import Storage; s=Storage(); s.put('red','k','v',5); print('cross-namespace',s.get('blue','k',0)); print('at-expiry',s.get('red','k',5)); print('negative-ttl-check-is-not-implemented'); print('secret-env-present', any(k in os.environ for k in ('CW_CONTROL_TOKEN','OPENAI_API_KEY','HTTP_PROXY','HTTPS_PROXY','ALL_PROXY')))"
    };
    format!("python3 -B -c {}", shell_quote(script))
}

fn cw122b1_solution_command(family: &str) -> String {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../benchmark/cw122a");
    let solutions: Value = serde_json::from_slice(
        &std::fs::read(root.join("synthetic-solutions.json")).expect("frozen mock solutions"),
    )
    .expect("valid frozen mock solutions");
    let mut commands = Vec::new();
    for (name, content) in solutions["solutions"][family]
        .as_object()
        .expect("family solution object")
    {
        let content = content.as_str().expect("solution contents");
        commands.push(format!(
            "cat > {} <<'CW122B1_EOF'\n{}\nCW122B1_EOF",
            shell_quote(name),
            content
        ));
    }
    commands.join("\n")
}

fn codex_tool_response(call_id: &str, command: &str, turn: usize, is_stream: bool) -> Response {
    let arguments = serde_json::to_string(&json!({"cmd": command})).expect("mock tool arguments");
    let item_id = format!("cw122b1_fc_{turn}");
    let response_id = format!("cw122b1_resp_{turn}");
    let usage = if turn.is_multiple_of(2) {
        json!({"input_tokens": 120 + turn as u64, "output_tokens": 20, "total_tokens": 140 + turn as u64})
    } else {
        json!({"input_tokens": 120 + turn as u64, "input_tokens_details":{"cached_tokens": 0}, "output_tokens": 20, "total_tokens": 140 + turn as u64})
    };
    let output = json!({"id":item_id,"type":"function_call","call_id":call_id,"name":"exec_command","arguments":arguments});
    if !is_stream {
        return Json(json!({"id":response_id,"output":[output],"usage":usage})).into_response();
    }
    let events = [
        json!({"type":"response.created","response":{"id":response_id,"status":"in_progress"}}),
        json!({"type":"response.output_item.added","output_index":0,"item":{"id":item_id,"type":"function_call","call_id":call_id,"name":"exec_command","arguments":""}}),
        json!({"type":"response.function_call_arguments.delta","output_index":0,"call_id":call_id,"delta":arguments}),
        json!({"type":"response.function_call_arguments.done","output_index":0,"call_id":call_id,"arguments":arguments}),
        json!({"type":"response.output_item.done","output_index":0,"item":output}),
        json!({"type":"response.completed","response":{"id":response_id,"status":"completed","output":[output],"usage":usage}}),
    ];
    let mut body = String::new();
    for event in events {
        body.push_str(&format!(
            "data: {}\n\n",
            serde_json::to_string(&event).expect("mock event")
        ));
    }
    body.push_str("data: [DONE]\n\n");
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .body(Body::from(body))
        .expect("mock SSE response")
}

fn codex_text_response(text: &str, response_id: &str, is_stream: bool) -> Response {
    let item = json!({"id":format!("{response_id}_item"),"type":"message","role":"assistant","content":[{"type":"output_text","text":text}]});
    let usage = json!({"input_tokens":160,"input_tokens_details":{"cached_tokens":32},"output_tokens":8,"total_tokens":168});
    if !is_stream {
        return Json(json!({"id":response_id,"output":[item],"usage":usage})).into_response();
    }
    let events = [
        json!({"type":"response.created","response":{"id":response_id,"status":"in_progress"}}),
        json!({"type":"response.output_item.added","output_index":0,"item":{"id":format!("{response_id}_item"),"type":"message","role":"assistant","content":[]}}),
        json!({"type":"response.content_part.added","output_index":0,"content_index":0,"part":{"type":"output_text","text":""}}),
        json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":text}),
        json!({"type":"response.output_text.done","output_index":0,"content_index":0,"text":text}),
        json!({"type":"response.content_part.done","output_index":0,"content_index":0,"part":{"type":"output_text","text":text}}),
        json!({"type":"response.output_item.done","output_index":0,"item":item}),
        json!({"type":"response.completed","response":{"id":response_id,"status":"completed","output":[item],"usage":usage}}),
    ];
    let mut body = String::new();
    for event in events {
        body.push_str(&format!(
            "data: {}\n\n",
            serde_json::to_string(&event).expect("mock event")
        ));
    }
    body.push_str("data: [DONE]\n\n");
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .body(Body::from(body))
        .expect("mock SSE response")
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

    if json_val
        .as_ref()
        .and_then(|value| value.get("model"))
        .and_then(Value::as_str)
        == Some("gpt-6-luna")
    {
        return Json(json!({
            "id":"resp_luna_mock",
            "model":"gpt-6-luna",
            "service_tier":"default",
            "output":[{"id":"msg_luna_mock","type":"message","role":"assistant","content":[{"type":"output_text","text":"loopback qualification complete"}]}],
            "usage":{"input_tokens":1000,"output_tokens":1000,"total_tokens":2000,
                "input_tokens_details":{"cached_tokens":100,"cache_write_tokens":500,
                    "cache_write_tokens_details":{"ttl_30m_tokens":200,"ttl_12h_tokens":300}}}
        }))
        .into_response();
    }

    let body_str = String::from_utf8_lossy(&body);

    if body_str.contains("CW122B1_F1-03_RETRY_LOOP") {
        return Response::builder()
            .status(StatusCode::TOO_MANY_REQUESTS)
            .header("content-type", "application/json")
            .header("retry-after", "0")
            .body(Body::from(
                json!({"error":{"message":"retry fault injection","type":"rate_limit_error","code":"rate_limit_exceeded"},"usage":{"input_tokens":0,"output_tokens":0,"total_tokens":0}}).to_string(),
            ))
            .unwrap();
    }

    if body_str.contains("CW122B1_F1-09_ACTIVE_CODEX") {
        return std::future::pending::<Response>().await;
    }

    let injected = if body_str.contains("CW122B1_F1-04_429") {
        Some((StatusCode::TOO_MANY_REQUESTS, "rate limited".to_owned()))
    } else if body_str.contains("CW122B1_F1-04_401") {
        Some((StatusCode::UNAUTHORIZED, "unauthorized".to_owned()))
    } else if body_str.contains("CW122B1_F1-04_403") {
        Some((StatusCode::FORBIDDEN, "forbidden".to_owned()))
    } else if body_str.contains("CW122B1_F1-04_500") {
        Some((StatusCode::INTERNAL_SERVER_ERROR, "server error".to_owned()))
    } else if body_str.contains("CW122B1_F1-04_503") {
        Some((
            StatusCode::SERVICE_UNAVAILABLE,
            "service unavailable".to_owned(),
        ))
    } else {
        None
    };
    if let Some((status, message)) = injected {
        return (status, message).into_response();
    }
    if body_str.contains("CW122B1_F1-05_INVALID_SSE") {
        return Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "text/event-stream")
            .body(Body::from("data: {not-json}\n\n"))
            .unwrap();
    }
    if body_str.contains("CW122B1_F1-05_TRUNCATED_SSE") {
        return Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "text/event-stream")
            .body(Body::from(
                "data: {\"type\":\"response.completed\",\"response\":",
            ))
            .unwrap();
    }
    if body_str.contains("CW122B1_F1-05_CLOSE_BEFORE_COMPLETE") {
        return Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "text/event-stream")
            .body(Body::from(
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n",
            ))
            .unwrap();
    }
    if body_str.contains("CW122B1_F1-06_MISSING_USAGE")
        || body_str.contains("CW122B1_F1-06_INVALID_USAGE")
        || body_str.contains("CW122B1_F1-06_CONTRADICTORY_USAGE")
    {
        let usage = if body_str.contains("INVALID_USAGE") {
            json!({"input_tokens":"unknown","output_tokens":4})
        } else if body_str.contains("CONTRADICTORY_USAGE") {
            json!({"input_tokens":1,"output_tokens":4,"input_tokens_details":{"cached_tokens":2}})
        } else {
            Value::Null
        };
        let mut response = json!({
            "type":"response.completed",
            "response":{"id":"resp-fault-complete","status":"completed","output":[]}
        });
        if !usage.is_null() {
            response["response"]["usage"] = usage;
        }
        return Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "text/event-stream")
            .body(Body::from(format!("data: {}\n\n", response)))
            .unwrap();
    }
    if body_str.contains("CW122B1_F1-05_STALL") || body_str.contains("CW122B1_F1-07_STALL") {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
    }

    if body_str.contains("CW_FORCE_UPSTREAM_502") {
        return (StatusCode::BAD_GATEWAY, "Upstream mock gateway failure").into_response();
    }

    if body_str.contains("retry_delays") || body_str.contains("namespace/key pairs") {
        let family = if body_str.contains("retry_delays") {
            "f1"
        } else {
            "f2"
        };
        let implementing = body_str.contains("Implement the requested change now.");
        let output_call = if implementing {
            "cw122b1_patch"
        } else {
            "cw122b1_probe"
        };
        let has_output = json_val
            .as_ref()
            .and_then(|payload| payload.get("input"))
            .and_then(Value::as_array)
            .is_some_and(|items| {
                items.iter().any(|item| {
                    item.get("type").and_then(Value::as_str) == Some("function_call_output")
                        && item.get("call_id").and_then(Value::as_str) == Some(output_call)
                })
            });
        if !implementing && has_output {
            return codex_text_response("Inspection complete.", "cw122b1_inspected", is_stream);
        }
        if implementing && has_output {
            return codex_text_response("Implementation complete.", "cw122b1_complete", is_stream);
        }
        let command = if implementing {
            cw122b1_solution_command(family)
        } else {
            cw122b1_diagnostic_command(family)
        };
        return codex_tool_response(
            output_call,
            &command,
            captured.lock().unwrap().len(),
            is_stream,
        );
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
    captured: Arc<Mutex<Vec<CapturedRequest>>>,
    #[cfg(feature = "qualification-fault-hooks")]
    qualification_fault_hook: Option<cw_proxy::state::QualificationFaultHook>,
}

struct OpenAiUpstream<'a> {
    url: Option<&'a str>,
    api_key: Option<&'a str>,
    key_configured: bool,
    capture_response: bool,
    expected_live_identity: Option<(&'a str, Option<&'a str>)>,
    live_authorization_expires_at: Option<chrono::DateTime<Utc>>,
    store: Option<CwStore>,
}

impl QualificationFixture {
    async fn setup() -> Self {
        Self::setup_config(None, None, false).await
    }

    async fn setup_config(
        spend_guard: Option<SpendGuard>,
        private_control_token: Option<String>,
        capture_wire_egress: bool,
    ) -> Self {
        Self::setup_config_with_upstream_key(
            spend_guard,
            private_control_token,
            capture_wire_egress,
            None,
        )
        .await
    }

    async fn setup_config_with_upstream_key(
        spend_guard: Option<SpendGuard>,
        private_control_token: Option<String>,
        capture_wire_egress: bool,
        upstream_key: Option<&str>,
    ) -> Self {
        Self::setup_config_with_key_state(
            spend_guard,
            private_control_token,
            capture_wire_egress,
            upstream_key,
            upstream_key.is_some(),
            false,
            None,
        )
        .await
    }

    async fn setup_config_with_key_state(
        spend_guard: Option<SpendGuard>,
        private_control_token: Option<String>,
        capture_wire_egress: bool,
        upstream_key: Option<&str>,
        key_configured: bool,
        capture_wire_response: bool,
        expected_live_model: Option<&str>,
    ) -> Self {
        Self::setup_config_with_live_identity(
            spend_guard,
            private_control_token,
            capture_wire_egress,
            upstream_key,
            key_configured,
            capture_wire_response,
            expected_live_model.map(|model| (model, None)),
        )
        .await
    }

    async fn setup_config_with_live_identity(
        spend_guard: Option<SpendGuard>,
        private_control_token: Option<String>,
        capture_wire_egress: bool,
        upstream_key: Option<&str>,
        key_configured: bool,
        capture_wire_response: bool,
        expected_live_identity: Option<(&str, Option<&str>)>,
    ) -> Self {
        Self::setup_config_with_openai_upstream(
            spend_guard,
            private_control_token,
            capture_wire_egress,
            OpenAiUpstream {
                url: None,
                api_key: upstream_key,
                key_configured,
                capture_response: capture_wire_response,
                expected_live_identity,
                live_authorization_expires_at: None,
                store: None,
            },
        )
        .await
    }

    async fn setup_config_with_openai_upstream(
        spend_guard: Option<SpendGuard>,
        private_control_token: Option<String>,
        capture_wire_egress: bool,
        openai: OpenAiUpstream<'_>,
    ) -> Self {
        let mock = MockProvider::start().await;
        let store = openai
            .store
            .unwrap_or_else(|| CwStore::open_in_memory().unwrap());
        store
            .set_autopilot_config(&cw_core::autopilot::AutoPilotConfig {
                enabled: true,
                ..Default::default()
            })
            .unwrap();

        let mut state = DaemonState::new(store.clone());
        if let Some(tok) = private_control_token {
            state.control_token = tok;
        } else if let Ok(tok) = cw_core::control::control_token(false) {
            state.control_token = tok;
        }
        state.openai_upstream = openai.url.unwrap_or(&mock.url).to_owned();
        state.openai_api_key = openai.api_key.map(str::to_string);
        state.openai_api_key_configured = openai.key_configured;
        state.anthropic_upstream = mock.url.clone();
        state.deepseek_upstream = mock.url.clone();
        state.spend_guard = spend_guard;
        state.capture_wire_egress = capture_wire_egress;
        state.capture_wire_response = openai.capture_response;
        state.live_provider_model = openai
            .expected_live_identity
            .map(|(model, _)| (ProviderKind::OpenAI, model.to_owned()));
        let l0_session_id = openai
            .expected_live_identity
            .and_then(|(_, session)| session);
        state.live_allowed_session_id = l0_session_id.map(str::to_owned);
        state.live_authorization_expires_at = openai
            .live_authorization_expires_at
            .or_else(|| l0_session_id.map(|_| Utc::now() + chrono::Duration::minutes(1)));
        #[cfg(feature = "qualification-fault-hooks")]
        {
            state.qualification_fault_hook = cw_proxy::state::QualificationFaultHook::from_env();
        }
        #[cfg(feature = "qualification-fault-hooks")]
        let qualification_fault_hook = state.qualification_fault_hook.clone();
        let control_token = state.control_token.clone();

        let app = build_proxy_router(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_port = listener.local_addr().unwrap().port();
        let proxy_task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let captured = mock.captured.clone();
        Self {
            mock,
            captured,
            proxy_port,
            control_token,
            store,
            proxy_task,
            #[cfg(feature = "qualification-fault-hooks")]
            qualification_fault_hook,
        }
    }
}

impl Drop for QualificationFixture {
    fn drop(&mut self) {
        self.proxy_task.abort();
    }
}

#[tokio::test]
async fn qualification_fixture_uses_explicit_openai_upstream() {
    let upstream = MockProvider::start().await;
    let store_root =
        std::env::temp_dir().join(format!("cw122b2-l1-file-store-{}", uuid::Uuid::new_v4()));
    create_private_dir(&store_root);
    let store_path = store_root.join("cw.sqlite");
    let fixture = QualificationFixture::setup_config_with_openai_upstream(
        None,
        Some("qualification-control-token-000000000000".into()),
        true,
        OpenAiUpstream {
            url: Some(&upstream.url),
            api_key: None,
            key_configured: false,
            capture_response: true,
            expected_live_identity: None,
            live_authorization_expires_at: None,
            store: Some(CwStore::open(&store_path).unwrap()),
        },
    )
    .await;
    let response = reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{}/openai/v1/responses",
            fixture.proxy_port
        ))
        .header("x-cw-session-id", "explicit-upstream-fixture-session")
        .header("x-cw-control-token", &fixture.control_token)
        .json(&json!({
            "model":"gpt-4o-mini",
            "input":[{"type":"message","role":"user","content":"route probe"}]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert!(store_path.is_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&store_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert_eq!(upstream.captured.lock().unwrap().len(), 1);
    assert!(fixture.mock.captured.lock().unwrap().is_empty());
    let turn = fixture
        .store
        .get_latest_turn("explicit-upstream-fixture-session")
        .unwrap()
        .unwrap();
    let (body, complete, truncated) =
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if let Some(response) = fixture.store.get_wire_response(&turn.id).unwrap() {
                    break response;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("provider response evidence persisted");
    assert!(!body.is_empty());
    assert!(complete);
    assert!(!truncated);
    let evidence_root = std::env::temp_dir().join(format!(
        "cw122b2-l1-partial-evidence-{}",
        uuid::Uuid::new_v4()
    ));
    create_private_dir(&evidence_root);
    drop(PartialCodexEvidence {
        root: evidence_root.clone(),
        label: "explicit-upstream".into(),
        session_id: "explicit-upstream-fixture-session".into(),
        store: fixture.store.clone(),
        captured: fixture.mock.captured.clone(),
        responses: fixture.mock.responses.clone(),
        codex_home: None,
        complete: false,
    });
    let failed = evidence_root.join("failed/explicit-upstream");
    assert_eq!(std::fs::read(failed.join("response_00.sse")).unwrap(), body);
    let metadata: Value =
        serde_json::from_slice(&std::fs::read(failed.join("response_meta_00.json")).unwrap())
            .unwrap();
    assert_eq!(metadata, json!({"complete":true,"truncated":false}));
    std::fs::remove_dir_all(evidence_root).unwrap();
    drop(fixture);
    drop(upstream);
    std::fs::remove_dir_all(store_root).unwrap();
}

fn completed_responses_body(bytes: &[u8]) -> Option<Value> {
    if let Ok(value) = serde_json::from_slice::<Value>(bytes)
        && value.get("status").and_then(Value::as_str) == Some("completed")
    {
        return Some(value);
    }
    for line in bytes.split(|byte| *byte == b'\n') {
        let Some(data) = line.strip_prefix(b"data: ") else {
            continue;
        };
        let Ok(event) = serde_json::from_slice::<Value>(data) else {
            continue;
        };
        if event.get("type").and_then(Value::as_str) == Some("response.completed")
            && event.pointer("/response/status").and_then(Value::as_str) == Some("completed")
        {
            return event.get("response").cloned();
        }
    }
    None
}

#[derive(Debug, PartialEq, Eq)]
enum L1ServiceTierError {
    NonObjectPayload,
    NonStandardTier,
}

fn add_expected_l1_service_tier(payload: &mut Value) -> Result<bool, L1ServiceTierError> {
    let Some(object) = payload.as_object_mut() else {
        return Err(L1ServiceTierError::NonObjectPayload);
    };
    match object.get("service_tier") {
        None => {
            object.insert("service_tier".into(), Value::String("default".into()));
            Ok(true)
        }
        Some(tier) if tier.as_str() == Some("default") => Ok(false),
        Some(_) => Err(L1ServiceTierError::NonStandardTier),
    }
}

fn l1_tool_output_pointer(item_pointer: &str) -> String {
    format!("{item_pointer}/output")
}

#[test]
fn l1_egress_expectation_separates_default_tier_from_pruning() {
    let mut omitted = json!({"model":"gpt-6-luna"});
    assert_eq!(add_expected_l1_service_tier(&mut omitted), Ok(true));
    assert_eq!(omitted["service_tier"], "default");

    let mut explicit = json!({"model":"gpt-6-luna","service_tier":"default"});
    assert_eq!(add_expected_l1_service_tier(&mut explicit), Ok(false));
    assert_eq!(explicit["service_tier"], "default");

    let mut nonstandard = json!({"model":"gpt-6-luna","service_tier":"priority"});
    assert_eq!(
        add_expected_l1_service_tier(&mut nonstandard),
        Err(L1ServiceTierError::NonStandardTier)
    );

    for mut malformed in [Value::Null, json!("not an object"), json!([])] {
        let original = malformed.clone();
        assert_eq!(
            add_expected_l1_service_tier(&mut malformed),
            Err(L1ServiceTierError::NonObjectPayload)
        );
        assert_eq!(malformed, original);
    }
}

#[test]
fn l1_tool_output_pointer_uses_the_observed_item_index() {
    let item_pointer = "/input/7";
    let output_pointer = l1_tool_output_pointer(item_pointer);
    assert_eq!(output_pointer, "/input/7/output");
    assert_ne!(output_pointer, "/input/2/output");
}

fn snapshot_workspace(workspace: &std::path::Path) -> std::collections::BTreeMap<String, String> {
    std::fs::read_dir(workspace)
        .expect("read isolated workspace")
        .map(|entry| {
            let entry = entry.expect("workspace entry");
            let bytes = std::fs::read(entry.path()).expect("read workspace fixture file");
            (
                entry.file_name().to_string_lossy().into_owned(),
                sha256_hex(&bytes),
            )
        })
        .collect()
}

#[tokio::test]
#[ignore = "real OpenAI M1/M2 task smoke; run only through the separately authorized CW-122B2-L1 runner"]
async fn test_cw122b2l1_live_matched_smoke_pair() {
    use std::{collections::BTreeMap, time::Duration};

    let authorization_path = PathBuf::from(
        std::env::var_os("CW122B2L1_SMOKE_AUTHORIZATION")
            .expect("launch through benchmark/cw122b2l1/run_smoke.py"),
    );
    let authorization: Value = serde_json::from_slice(
        &std::fs::read(&authorization_path).expect("read smoke authorization"),
    )
    .expect("valid smoke authorization JSON");
    assert_eq!(authorization["experiment"], "CW-122B2-L1-SMOKE");
    assert_eq!(authorization["provider"], "OpenAI API");
    assert_eq!(authorization["model"], "gpt-6-luna");
    assert_eq!(authorization["endpoint"], "https://api.openai.com/v1");
    assert_eq!(authorization["processing_tier"], "standard");
    assert_eq!(authorization["pair_id"], "f1-r1");
    assert_eq!(authorization["attempt_order"], json!(["M1", "M2"]));
    assert_eq!(authorization["exact_attempt_count"], 2);
    assert_eq!(authorization["max_requests"], 60);
    assert_eq!(authorization["max_requests_per_attempt"], 30);
    assert_eq!(authorization["max_seconds"], 1200);
    assert_eq!(authorization["max_seconds_per_attempt"], 600);
    assert_eq!(authorization["reservation_overrun_risk_accepted"], true);
    let budget = authorization["spend_ceiling_usd"]
        .as_f64()
        .filter(|value| value.is_finite() && *value > 0.0)
        .expect("finite positive smoke spend ceiling");
    let request_reservation = authorization["request_reservation_usd"]
        .as_f64()
        .filter(|value| value.is_finite() && *value > 0.0)
        .expect("positive per-request reservation");

    let issued = chrono::DateTime::parse_from_rfc3339(
        authorization["issued_at"]
            .as_str()
            .expect("authorization issue time"),
    )
    .expect("valid issue time")
    .with_timezone(&Utc);
    let expires = chrono::DateTime::parse_from_rfc3339(
        authorization["expires_at"]
            .as_str()
            .expect("authorization expiry"),
    )
    .expect("valid expiry")
    .with_timezone(&Utc);
    assert!(issued <= Utc::now() && Utc::now() < expires);
    assert!(expires - issued <= chrono::Duration::hours(24));

    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let campaign_root = cw122a_root();
    let campaign_bytes = std::fs::read(campaign_root.join("campaign.json")).unwrap();
    assert_eq!(
        authorization["campaign_sha256"],
        sha256_hex(&campaign_bytes),
        "smoke must bind the frozen CW-122A campaign"
    );
    let campaign: Value = serde_json::from_slice(&campaign_bytes).unwrap();
    let pair = campaign["pairs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|pair| pair["pair_id"] == "f1-r1")
        .expect("frozen F1 smoke pair");
    assert_eq!(pair["order"], json!(["M1", "M2"]));
    let instructions = campaign["tasks"]["f1"]["instructions"]
        .as_str()
        .expect("frozen F1 instructions");
    let initial_prompt = format!(
        "{instructions}\n\nFirst inspect the implementation and run relevant diagnostics. Report what needs fixing without editing files yet."
    );
    let implementation_prompt = "Implement the requested change now.";
    let model = authorization["model"].as_str().unwrap();
    let endpoint = authorization["endpoint"].as_str().unwrap();
    let evidence_root = PathBuf::from(
        authorization["private_evidence_directory"]
            .as_str()
            .expect("private evidence path"),
    );
    assert!(evidence_root.is_absolute() && evidence_root.is_dir());
    assert!(
        !evidence_root
            .canonicalize()
            .unwrap()
            .starts_with(repository.canonicalize().unwrap())
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&evidence_root)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
    let credential_path = PathBuf::from(
        authorization["credential_file"]
            .as_str()
            .expect("credential file path"),
    );
    assert!(credential_path.is_absolute() && credential_path.is_file());
    assert!(
        !credential_path
            .canonicalize()
            .unwrap()
            .starts_with(repository.canonicalize().unwrap())
    );
    assert!(
        !credential_path
            .canonicalize()
            .unwrap()
            .starts_with(evidence_root.canonicalize().unwrap())
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&credential_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    let credential_bytes = std::fs::read(&credential_path).expect("read authorized key file");
    assert_eq!(
        authorization["credential_sha256"],
        sha256_hex(&credential_bytes),
        "credential changed after authorization"
    );
    let api_key = String::from_utf8(credential_bytes)
        .expect("UTF-8 OpenAI credential")
        .trim()
        .to_owned();
    assert!(!api_key.is_empty());

    let guard = SpendGuard::new_with_campaign_request_limit(
        SpendGuardConfig {
            authorized_budget_usd: budget,
            max_session_cost_usd: budget,
            max_requests_per_session: 30,
            require_verified_pricing: true,
            request_reservation_usd: request_reservation,
            ..Default::default()
        },
        60,
    );
    let campaign_started = tokio::time::Instant::now();
    let campaign_deadline = Duration::from_secs(1200);
    let mut pair_hashes: Option<BTreeMap<String, String>> = None;
    let mut attempts = Vec::new();
    for mode in ["M1", "M2"] {
        let session_id = format!(
            "cw122b2l1-{}-{}",
            authorization["authorization_id"].as_str().unwrap(),
            uuid::Uuid::new_v4().simple()
        );
        assert!(cw_core::control::is_valid_session_id(&session_id));
        let attempt_root = evidence_root.join("attempts").join(&session_id);
        create_private_dir(&attempt_root);
        let home = attempt_root.join("home");
        let codex_home = attempt_root.join("codex");
        let temp_dir = attempt_root.join("tmp");
        let workspace = attempt_root.join("workspace");
        for path in [&home, &codex_home, &temp_dir, &workspace] {
            create_private_dir(path);
        }
        for entry in std::fs::read_dir(campaign_root.join("fixtures/f1")).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_file() {
                std::fs::copy(entry.path(), workspace.join(entry.file_name())).unwrap();
            }
        }
        let initial_files = snapshot_workspace(&workspace);
        assert!(
            pair_hashes
                .replace(initial_files.clone())
                .is_none_or(|previous| previous == initial_files),
            "matched smoke arms began from different fixtures"
        );
        assert_eq!(
            cw122_oracle_check("f1", &workspace)["passed"].as_bool(),
            Some(false),
            "frozen fixture should fail before coding"
        );

        let control_token = uuid::Uuid::new_v4().simple().to_string();
        let database = attempt_root.join("cw.sqlite");
        let store = CwStore::open(&database).expect("open persistent evidence database");
        let live_expiry = expires;
        let fixture = QualificationFixture::setup_config_with_openai_upstream(
            Some(guard.clone()),
            Some(control_token.clone()),
            true,
            OpenAiUpstream {
                url: Some(endpoint),
                api_key: Some(&api_key),
                key_configured: true,
                capture_response: true,
                expected_live_identity: Some((model, Some(&session_id))),
                live_authorization_expires_at: Some(live_expiry),
                store: Some(store),
            },
        )
        .await;
        fixture
            .store
            .set_autopilot_config(&cw_core::autopilot::AutoPilotConfig {
                enabled: mode == "M2",
                auto_diet_at_breakpoint: false,
                local_model_window_guard: false,
                auto_heal_low_health: false,
                auto_cache_safe_pin: false,
                zero_break_tail_hygiene: false,
                auto_sync_to_disk: false,
                ..Default::default()
            })
            .unwrap();
        assert!(
            !fixture
                .store
                .get_pin_context_policy(&session_id)
                .unwrap()
                .is_enabled_for(&session_id),
            "PinContext must remain inactive"
        );
        let codex_binary = std::fs::canonicalize(require_installed_binary("codex"))
            .expect("canonical Codex executable");
        let codex = CodexStep {
            binary: &codex_binary,
            model,
            service_tier: Some("default"),
            workspace: &workspace,
            home: &home,
            codex_home: &codex_home,
            temp_dir: &temp_dir,
            proxy_port: fixture.proxy_port,
            session_id: &session_id,
            control_token: &control_token,
        };
        let mut partial = PartialCodexEvidence {
            root: attempt_root.clone(),
            label: "provider-attempt".into(),
            session_id: session_id.clone(),
            store: fixture.store.clone(),
            captured: fixture.mock.captured.clone(),
            responses: fixture.mock.responses.clone(),
            codex_home: Some(codex_home.clone()),
            complete: false,
        };
        let spent_before_attempt = guard.total_spent_usd();
        let attempt_started = tokio::time::Instant::now();
        let remaining = |started: tokio::time::Instant| {
            let attempt_left = Duration::from_secs(600).saturating_sub(started.elapsed());
            let campaign_left = campaign_deadline.saturating_sub(campaign_started.elapsed());
            attempt_left.min(campaign_left)
        };
        assert!(
            remaining(attempt_started) > Duration::ZERO,
            "smoke deadline expired before bootstrap"
        );
        let (bootstrap_diagnostics, bootstrap_ms) = codex
            .run(None, &initial_prompt, remaining(attempt_started))
            .await
            .unwrap_or_else(|error| panic!("{mode} bootstrap Codex failed: {error}"));
        let thread_id =
            codex_thread_id_from_rollout(&codex_home).expect("Codex saved a native thread ID");
        let mut bootstrap_turns = fixture.store.list_turns(&session_id).unwrap();
        bootstrap_turns.sort_by_key(|turn| turn.sequence_number);
        assert!(
            bootstrap_turns.len() >= 2,
            "bootstrap did not complete a tool round trip"
        );
        let mut generated_calls = std::collections::BTreeSet::new();
        let mut selected: Option<(String, String, String, String)> = None;
        for turn in &bootstrap_turns {
            let ingress = fixture
                .store
                .get_wire_request(&turn.id)
                .unwrap()
                .expect("bootstrap ingress persisted");
            let payload: Value = serde_json::from_slice(&ingress).expect("Responses ingress JSON");
            let (response_bytes, complete, truncated) = fixture
                .store
                .get_wire_response(&turn.id)
                .unwrap()
                .expect("provider response persisted");
            assert!(
                complete && !truncated,
                "bootstrap response capture incomplete"
            );
            let response =
                completed_responses_body(&response_bytes).expect("bootstrap response completed");
            assert_eq!(response["model"], model, "provider model mismatch");
            assert_eq!(
                response["service_tier"], "default",
                "provider tier mismatch"
            );
            if selected.is_none()
                && let Some(input) = payload.get("input").and_then(Value::as_array)
            {
                for (index, item) in input.iter().enumerate() {
                    let Some(call_id) = item.get("call_id").and_then(Value::as_str) else {
                        continue;
                    };
                    if item.get("type").and_then(Value::as_str) == Some("function_call_output")
                        && generated_calls.contains(call_id)
                        && item
                            .get("output")
                            .and_then(Value::as_str)
                            .is_some_and(|output| !output.is_empty())
                    {
                        let nodes = parse_request_to_nodes(&payload, ProviderKind::OpenAI, 2)
                            .expect("normalize actual tool-result request");
                        let node = nodes
                            .into_iter()
                            .find(|node| {
                                matches!(
                                    &node.kind,
                                    NodeKind::ToolResult { call_id: id, .. } if id == call_id
                                )
                            })
                            .expect("actual tool result has a canonical node");
                        assert_eq!(node.raw_json_pointer, format!("/input/{index}"));
                        selected = Some((
                            call_id.to_owned(),
                            item["output"].as_str().unwrap().to_owned(),
                            node.raw_json_pointer,
                            node.fingerprint.0,
                        ));
                        break;
                    }
                }
            }
            for item in response["output"].as_array().into_iter().flatten() {
                if item.get("type").and_then(Value::as_str) == Some("function_call")
                    && let Some(call_id) = item.get("call_id").and_then(Value::as_str)
                {
                    generated_calls.insert(call_id.to_owned());
                }
            }
        }
        let (target_call_id, original_tool_output, target_pointer, target_fingerprint) =
            selected.expect("bootstrap generated and consumed a real command output");
        let declared_target_pointer = campaign
            .pointer("/treatment/pointer")
            .and_then(Value::as_str)
            .expect("frozen F1 target pointer")
            .to_owned();
        let target_output_pointer = l1_tool_output_pointer(&target_pointer);
        let frozen_pointer_match = target_output_pointer == declared_target_pointer;
        assert_eq!(
            snapshot_workspace(&workspace),
            initial_files,
            "diagnostic phase changed the fixture before the coding request"
        );
        let bootstrap_max_sequence = bootstrap_turns
            .last()
            .expect("bootstrap turns")
            .sequence_number;
        let rule_id = "cw122b2l1-smoke-historical-prune";
        let stub = campaign["tasks"]["f1"]["stub"]
            .as_str()
            .expect("frozen F1 prune stub");
        if mode == "M2" {
            fixture
                .store
                .save_mutation(&MutationRule {
                    id: rule_id.into(),
                    session_id: session_id.clone(),
                    op: MutationOp::PruneOrStub {
                        target_fingerprint: Some(target_fingerprint.clone()),
                        target_call_id: Some(target_call_id.clone()),
                        target_json_pointer: Some(target_pointer.clone()),
                        stub_text: stub.into(),
                    },
                    status: MutationStatus::Committed,
                    estimated_saved_tokens: 0,
                    estimated_kv_break_tokens: 0,
                    created_at: Utc::now(),
                })
                .unwrap();
        }
        assert!(
            remaining(attempt_started) > Duration::ZERO,
            "smoke deadline expired before coding"
        );
        let (coding_diagnostics, coding_ms) = codex
            .run(
                Some(&thread_id),
                implementation_prompt,
                remaining(attempt_started),
            )
            .await
            .unwrap_or_else(|error| panic!("{mode} coding Codex failed: {error}"));
        let acceptance = cw122_oracle_check("f1", &workspace);
        assert!(
            acceptance["passed"].as_bool() == Some(true),
            "independent F1 oracle failed"
        );

        let mut turns = fixture.store.list_turns(&session_id).unwrap();
        turns.sort_by_key(|turn| turn.sequence_number);
        assert!(turns.len() <= 30, "per-attempt request cap exceeded");
        assert!(
            guard.total_request_count() <= 60,
            "pair request cap exceeded"
        );
        let mut verified_prunes = 0usize;
        let mut requests = Vec::new();
        let ledger = rusqlite::Connection::open(&database).expect("open private spend ledger");
        let mut ledger_settled_cost_total_usd = 0.0f64;
        let mut total_input_tokens = 0u64;
        let mut total_output_tokens = 0u64;
        let mut total_cached_input_tokens = 0u64;
        let mut total_cache_write_tokens = 0u64;
        for turn in &turns {
            let ingress = fixture
                .store
                .get_wire_request(&turn.id)
                .unwrap()
                .expect("request ingress persisted");
            let egress = fixture
                .store
                .get_wire_egress(&turn.id)
                .unwrap()
                .expect("provider egress persisted");
            let original: Value = serde_json::from_slice(&ingress).expect("ingress JSON");
            let actual: Value = serde_json::from_slice(&egress).expect("egress JSON");
            let mut expected = original.clone();
            let service_tier_defaulted = add_expected_l1_service_tier(&mut expected)
                .unwrap_or_else(|error| panic!("invalid L1 Responses payload: {error:?}"));
            let mut changed = false;
            if mode == "M2" && turn.sequence_number > bootstrap_max_sequence {
                let input = expected
                    .get_mut("input")
                    .and_then(Value::as_array_mut)
                    .expect("Responses input array");
                let targets: Vec<_> = input
                    .iter()
                    .enumerate()
                    .filter(|(_, item)| is_function_output(item, &target_call_id))
                    .map(|(index, _)| index)
                    .collect();
                assert!(
                    targets.len() <= 1,
                    "selected call ID duplicated in one request"
                );
                if let Some(index) = targets.first() {
                    assert_eq!(input[*index]["output"], original_tool_output);
                    input[*index]["output"] = Value::String(stub.into());
                    changed = true;
                    verified_prunes += 1;
                }
            }
            assert_eq!(actual, expected, "unexpected full-wire change");
            if changed {
                assert_eq!(turn.applied_mutation_ids, [rule_id]);
            } else {
                assert!(turn.applied_mutation_ids.is_empty());
                if !service_tier_defaulted {
                    assert_eq!(ingress, egress, "passthrough bytes changed");
                }
            }
            let (response_bytes, complete, truncated) = fixture
                .store
                .get_wire_response(&turn.id)
                .unwrap()
                .expect("complete raw provider response");
            assert!(
                complete && !truncated,
                "provider response capture incomplete"
            );
            let response = completed_responses_body(&response_bytes)
                .expect("provider response did not complete");
            assert_eq!(response["model"], model, "provider model mismatch");
            assert_eq!(
                response["service_tier"], "default",
                "provider tier mismatch"
            );
            let usage = response
                .get("usage")
                .cloned()
                .expect("provider usage telemetry");
            assert!(usage["input_tokens"].as_u64().is_some());
            assert!(usage["output_tokens"].as_u64().is_some());
            assert!(
                usage["input_tokens_details"]["cached_tokens"]
                    .as_u64()
                    .is_some()
            );
            assert!(
                usage["input_tokens_details"]["cache_write_tokens"]
                    .as_u64()
                    .is_some()
            );
            let (admission_status, settlement_status, reservation_usd, settled_usd, ledger_usage_json):
                (String, String, Option<f64>, Option<f64>, Option<String>) = ledger
                .query_row(
                    "SELECT admission_status,settlement_status,reservation_usd,settled_usd,usage_json FROM spend_guard_ledger WHERE turn_id=?1",
                    [&turn.id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
                )
                .expect("provider turn has persisted SpendGuard ledger row");
            assert_eq!(admission_status, "ADMITTED");
            assert_eq!(settlement_status, "SETTLED");
            assert_eq!(reservation_usd, Some(request_reservation));
            let settled_usd = settled_usd
                .filter(|cost| cost.is_finite() && *cost >= 0.0)
                .expect("persisted request settlement amount");
            let ledger_usage: Value = serde_json::from_str(
                ledger_usage_json
                    .as_deref()
                    .expect("settled ledger usage JSON"),
            )
            .expect("valid settled ledger usage JSON");
            assert_eq!(
                ledger_usage, usage,
                "ledger usage matches provider response"
            );
            ledger_settled_cost_total_usd += settled_usd;
            total_input_tokens = total_input_tokens
                .checked_add(usage["input_tokens"].as_u64().unwrap())
                .expect("input token total overflow");
            total_output_tokens = total_output_tokens
                .checked_add(usage["output_tokens"].as_u64().unwrap())
                .expect("output token total overflow");
            total_cached_input_tokens = total_cached_input_tokens
                .checked_add(
                    usage["input_tokens_details"]["cached_tokens"]
                        .as_u64()
                        .unwrap(),
                )
                .expect("cached input total overflow");
            total_cache_write_tokens = total_cache_write_tokens
                .checked_add(
                    usage["input_tokens_details"]["cache_write_tokens"]
                        .as_u64()
                        .unwrap(),
                )
                .expect("cache-write total overflow");
            requests.push(json!({
                "turn_id":turn.id,
                "sequence_number":turn.sequence_number,
                "ingress_sha256":sha256_hex(&ingress),
                "egress_sha256":sha256_hex(&egress),
                "response_sha256":sha256_hex(&response_bytes),
                "model":response["model"],
                "service_tier":response["service_tier"],
                "usage":usage,
                "spend_ledger":{
                    "admission_status":admission_status,
                    "settlement_status":settlement_status,
                    "reservation_usd":reservation_usd,
                    "settled_usd":settled_usd,
                    "usage":ledger_usage
                },
                "applied_rule_ids":turn.applied_mutation_ids,
                "skipped_mutations":turn.skipped_mutations,
                "proxy_transport_normalizations":if service_tier_defaulted {json!( ["service_tier=default"] )} else {json!([])},
                "response_complete":complete,
                "response_truncated":truncated
            }));
        }
        assert!(
            (ledger_settled_cost_total_usd - (guard.total_spent_usd() - spent_before_attempt))
                .abs()
                <= 1e-9,
            "persisted request settlements reconcile with shared SpendGuard total"
        );
        if mode == "M2" {
            assert!(
                verified_prunes > 0,
                "M2 had no verified provider-bound transformation"
            );
        } else {
            assert_eq!(verified_prunes, 0);
        }
        let rollouts = find_rollout_files(&codex_home.join("sessions"));
        assert!(!rollouts.is_empty(), "Codex transcript missing");
        let mut rollout_hashes = Vec::new();
        for (index, path) in rollouts.iter().enumerate() {
            let bytes = std::fs::read(path).expect("read Codex transcript");
            assert!(!String::from_utf8_lossy(&bytes).contains(stub));
            write_private_file(
                &attempt_root.join(format!("rollout_{index:02}.jsonl")),
                &bytes,
            );
            rollout_hashes.push(sha256_hex(&bytes));
        }
        let report = json!({
            "pair_id":"f1-r1",
            "family":"f1",
            "mode":mode,
            "session_id":session_id,
            "thread_id":thread_id,
            "model":model,
            "endpoint":endpoint,
            "sampling":"Codex defaults; explicit seed unavailable",
            "task_instructions_sha256":sha256_hex(instructions.as_bytes()),
            "initial_prompt_sha256":sha256_hex(initial_prompt.as_bytes()),
            "implementation_prompt_sha256":sha256_hex(implementation_prompt.as_bytes()),
            "initial_fixture_hashes":initial_files,
            "task_acceptance":acceptance,
            "request_count":turns.len(),
            "requests":requests,
            "bootstrap_wall_ms":bootstrap_ms,
            "coding_wall_ms":coding_ms,
            "complete_attempt_wall_ms":attempt_started.elapsed().as_millis(),
            "provider_input_tokens":total_input_tokens,
            "provider_output_tokens":total_output_tokens,
            "provider_cached_input_tokens":total_cached_input_tokens,
            "provider_cache_write_tokens":total_cache_write_tokens,
            "persisted_ledger_settled_cost_usd":ledger_settled_cost_total_usd,
            "bootstrap_diagnostics":bootstrap_diagnostics,
            "coding_diagnostics":coding_diagnostics,
            "treatment":{
                "rule_id":if mode=="M2" {json!(rule_id)} else {Value::Null},
                "stub_sha256":sha256_hex(stub.as_bytes()),
                "target_history_source":"live_codex_generated",
                "target_item_pointer":target_pointer,
                "target_output_pointer":target_output_pointer,
                "frozen_target_output_pointer":declared_target_pointer,
                "frozen_pointer_match":frozen_pointer_match,
                "frozen_pointer_deviation":if frozen_pointer_match {Value::Null} else {json!({"frozen":declared_target_pointer,"observed":target_output_pointer})},
                "target_call_id_sha256":sha256_hex(target_call_id.as_bytes()),
                "target_output_sha256":sha256_hex(original_tool_output.as_bytes()),
                "verified_prunes":verified_prunes,
                "status":if mode=="M2" {"verified_real_provider_egress"} else {"passthrough"}
            },
            "transcript_sha256":rollout_hashes,
            "settled_cost_total_usd":guard.total_spent_usd() - spent_before_attempt,
            "status":"SMOKE_ATTEMPT_ACCEPTED"
        });
        write_private_file(
            &attempt_root.join("attempt.json"),
            &serde_json::to_vec_pretty(&report).unwrap(),
        );
        save_cw122b1_raw_evidence(
            Some(&attempt_root),
            &session_id,
            &turns,
            &fixture.store,
            &[],
        );
        assert!(
            attempt_started.elapsed() <= Duration::from_secs(600),
            "per-attempt deadline exceeded before evidence finalization"
        );
        partial.complete();
        attempts.push(report);
        drop(fixture);
        assert!(
            !guard.is_killswitch_tripped(),
            "SpendGuard stopped the smoke pair"
        );
    }
    assert_eq!(attempts.len(), 2);
    assert!(campaign_started.elapsed() < campaign_deadline);
    assert_eq!(
        guard.total_request_count(),
        attempts
            .iter()
            .map(|a| a["request_count"].as_u64().unwrap() as usize)
            .sum::<usize>()
    );
    assert!(!guard.is_killswitch_tripped());
    let m1_input = attempts[0]["provider_input_tokens"].as_u64().unwrap();
    let m2_input = attempts[1]["provider_input_tokens"].as_u64().unwrap();
    let m1_cost = attempts[0]["settled_cost_total_usd"].as_f64().unwrap();
    let m2_cost = attempts[1]["settled_cost_total_usd"].as_f64().unwrap();
    let paired_metrics = json!({
        "m1_input_tokens":m1_input,
        "m2_input_tokens":m2_input,
        "input_tokens_delta_m2_minus_m1":m2_input as i128 - m1_input as i128,
        "input_tokens_relative_delta":if m1_input == 0 {Value::Null} else {json!((m2_input as f64 - m1_input as f64) / m1_input as f64)},
        "m1_output_tokens":attempts[0]["provider_output_tokens"],
        "m2_output_tokens":attempts[1]["provider_output_tokens"],
        "m1_settled_cost_usd":m1_cost,
        "m2_settled_cost_usd":m2_cost,
        "settled_cost_delta_m2_minus_m1_usd":m2_cost - m1_cost,
        "acceptance_m1":attempts[0]["task_acceptance"]["passed"],
        "acceptance_m2":attempts[1]["task_acceptance"]["passed"]
    });
    let manifest = json!({
        "experiment":"CW-122B2-L1-SMOKE",
        "status":"MATCHED_SMOKE_PAIR_PASS",
        "pair_id":"f1-r1",
        "provider":"OpenAI API",
        "model":model,
        "endpoint":endpoint,
        "attempts":attempts,
        "paired_metrics":paired_metrics,
        "request_count":guard.total_request_count(),
        "settled_cost_total_usd":guard.total_spent_usd(),
        "reservation_overrun_risk_accepted":true,
        "product_value":"NOT_EVALUATED; one matched smoke pair is not the preregistered six-pair campaign",
        "cache_economics":"NOT_QUALIFIED; initial provider cache state and isolation remain unverified"
    });
    write_private_file(
        &evidence_root.join("smoke-manifest.json"),
        &serde_json::to_vec_pretty(&manifest).unwrap(),
    );
}

async fn f1_send_chat(f: &QualificationFixture, session: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{}/openai/v1/chat/completions",
            f.proxy_port
        ))
        .header("x-cw-session-id", session)
        .header("x-cw-control-token", &f.control_token)
        .json(&json!({
            "model":"gpt-4o-mini",
            "messages":[{"role":"user","content":"fault qualification probe"}]
        }))
        .send()
        .await
        .unwrap()
}

async fn f1_send_responses(
    f: &QualificationFixture,
    session: &str,
    scenario_id: &str,
) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{}/openai/v1/responses",
            f.proxy_port
        ))
        .header("x-cw-session-id", session)
        .header("x-cw-control-token", &f.control_token)
        .json(&json!({
            "model":"gpt-4o-mini",
            "stream":true,
            "input":[{"role":"user","content":[{"type":"input_text","text":scenario_id}]}]
        }))
        .send()
        .await
        .unwrap()
}

#[cfg(feature = "qualification-fault-hooks")]
#[tokio::test]
#[ignore = "CW-122B1-F1 boundary worker; only run under the private supervisor"]
async fn test_cw122b1_f1_partial_boundary_worker() {
    let boundary = std::env::var("CW122B1_F1_BOUNDARY_ID").expect("boundary id");
    let guard = SpendGuard::new(SpendGuardConfig {
        authorized_budget_usd: 1.0,
        require_verified_pricing: false,
        ..Default::default()
    });
    let fixture = QualificationFixture::setup_config(Some(guard), None, true).await;
    let session = format!("f1-boundary-{}", uuid::Uuid::new_v4());
    match boundary.as_str() {
        "after_ingress_before_completed_egress"
        | "after_admission_before_dispatch"
        | "after_response_before_usage_settlement" => {
            let response = f1_send_chat(&fixture, &session).await;
            let _ = response.bytes().await;
            panic!("boundary supervisor did not stop the request");
        }
        _ => panic!("unknown boundary id: {boundary}"),
    }
}

static F1_EVIDENCE_CAPTURE_ID: AtomicUsize = AtomicUsize::new(0);

fn f1_capture_private_evidence(
    scenario_id: &str,
    session_ids: &[&str],
    f: &QualificationFixture,
    guard: &SpendGuard,
    accepted_admissions: usize,
    rejected_admissions: usize,
    process_cleanup: &str,
) -> Value {
    let Some(root) = std::env::var_os("CW122B1_F1_EVIDENCE_DIR").map(PathBuf::from) else {
        return json!({"status":"NOT_CONFIGURED_FOR_PRIVATE_CAPTURE"});
    };
    let capture_id = F1_EVIDENCE_CAPTURE_ID.fetch_add(1, Ordering::SeqCst);
    let directory = root
        .join("faults")
        .join(scenario_id)
        .join(format!("attempt-{}-{capture_id:04}", std::process::id()));
    create_private_dir(&root.join("faults"));
    create_private_dir(&root.join("faults").join(scenario_id));
    create_private_dir(&directory);
    let mut files = Vec::new();
    let requests = f.mock.captured.lock().unwrap().clone();
    let responses = f.mock.responses.lock().unwrap().clone();
    for (index, request) in requests.iter().enumerate() {
        let path = format!("provider_request_{index:02}.bin");
        let bytes = &request.body;
        write_private_file(&directory.join(&path), bytes);
        files.push(json!({"path":path,"sha256":sha256_hex(bytes),"kind":"provider_request"}));
    }
    for (index, response) in responses.iter().enumerate() {
        let path = format!("provider_response_{index:02}.bin");
        write_private_file(&directory.join(&path), &response.body);
        files.push(json!({"path":path,"sha256":sha256_hex(&response.body),"kind":"provider_response","status":response.status}));
    }
    let mut turns = Vec::new();
    for session_id in session_ids {
        for (index, turn) in f
            .store
            .list_turns(session_id)
            .unwrap_or_default()
            .iter()
            .enumerate()
        {
            for (kind, payload) in [
                ("ingress", f.store.get_wire_request(&turn.id).ok().flatten()),
                ("egress", f.store.get_wire_egress(&turn.id).ok().flatten()),
            ] {
                if let Some(bytes) = payload {
                    let path = format!("{session_id}_{kind}_{index:02}.json");
                    write_private_file(&directory.join(&path), &bytes);
                    files.push(json!({"path":path,"sha256":sha256_hex(&bytes),"kind":kind}));
                }
            }
            turns.push(turn.clone());
        }
    }
    let record = json!({
        "scenario_id":scenario_id,
        "fault_and_phase":match scenario_id {
            "F1-01" => "Per-session request cap at proxy admission before dispatch",
            "F1-02" => "Campaign request cap at shared pre-dispatch admission",
            "F1-03" => "Retryable 429 returned by loopback provider after dispatch",
            "F1-04" => "Loopback provider returns representative HTTP error status",
            "F1-05" => "Loopback provider emits malformed, incomplete, or stalled SSE",
            "F1-06" => "Completed response reaches provider usage settlement",
            "F1-07" => "Codex per-run deadline while a mock response is stalled",
            "F1-08" => "Campaign deadline bounds active attempt and blocks next attempt",
            _ => "Unknown fault phase",
        },
        "session_ids":session_ids,
        "accepted_admissions":accepted_admissions,
        "rejected_admissions":rejected_admissions,
        "upstream_dispatch_count":requests.len(),
        "settlement":{"spent_usd":guard.total_spent_usd(),"reserved_usd":guard.total_reserved_usd(),"killswitch":guard.is_killswitch_tripped()},
        "settlement_basis":"SpendGuard accounting over synthetic loopback usage or conservative reservation fallback",
        "provider_usage_source":"LOCAL_MOCK_SYNTHETIC",
        "economic_qualification":"NOT_EVALUATED",
        "turns":turns.iter().map(|turn| json!({
            "turn_id":turn.id,
            "outcome":turn.outcome,
            "usage":turn.observed_usage,
            "applied_mutation_ids":turn.applied_mutation_ids,
            "skipped_mutations":turn.skipped_mutations,
        })).collect::<Vec<_>>(),
        "final_attempt_classification":turns.iter().map(|turn| turn.outcome.as_deref().unwrap_or("NOT_AVAILABLE")).collect::<Vec<_>>(),
        "process_cleanup":process_cleanup,
        "files":files,
    });
    let record_bytes = serde_json::to_vec_pretty(&record).unwrap();
    write_private_file(&directory.join("attempt.json"), &record_bytes);
    files.push(
        json!({"path":"attempt.json","sha256":sha256_hex(&record_bytes),"kind":"attempt_manifest"}),
    );
    json!({"status":"PRESERVED","directory":directory,"files":files})
}

#[allow(clippy::too_many_arguments)]
fn f1_emit_result(
    scenario_id: &str,
    injection_boundary: &str,
    expected_behavior: &str,
    observed_behavior: &str,
    dispatch_count: usize,
    accepted: usize,
    rejected: usize,
    guard: &SpendGuard,
    process_cleanup: &str,
    evidence: Value,
    layer: &str,
    result: &str,
) {
    println!(
        "CW122B1_F1_RESULT:{}",
        json!({
            "scenario_id":scenario_id,
            "injection_boundary":injection_boundary,
            "expected_behavior":expected_behavior,
            "observed_behavior":observed_behavior,
            "upstream_dispatch_count":dispatch_count,
            "accepted_admissions":accepted,
            "rejected_admissions":rejected,
            "settlement":{"spent_usd":guard.total_spent_usd(),"reserved_usd":guard.total_reserved_usd(),"killswitch":guard.is_killswitch_tripped()},
            "process_cleanup":process_cleanup,
            "evidence_preservation":evidence,
            "qualification_layer":layer,
            "result":result,
        })
    );
}

#[tokio::test]
async fn test_cw122b1_f1_01_per_session_request_exhaustion_prevents_fourth_dispatch() {
    let guard = SpendGuard::new(SpendGuardConfig {
        authorized_budget_usd: 1.0,
        max_requests_per_session: 3,
        ..Default::default()
    });
    let f = QualificationFixture::setup_config(Some(guard.clone()), None, true).await;
    for _ in 0..3 {
        let response = f1_send_chat(&f, "f1-01-session").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response
                .headers()
                .get(reqwest::header::CONTENT_LENGTH)
                .is_none()
        );
        assert!(serde_json::from_slice::<Value>(&response.bytes().await.unwrap()).is_ok());
    }
    let rejected = f1_send_chat(&f, "f1-01-session").await;
    assert_eq!(rejected.status(), StatusCode::TOO_MANY_REQUESTS);
    let body = rejected.text().await.unwrap();
    assert!(body.contains("Provider request count limit exceeded"));
    assert_eq!(f.mock.captured.lock().unwrap().len(), 3);
    assert_eq!(guard.session_request_count("f1-01-session"), 3);
    assert_eq!(guard.total_request_count(), 3);
    assert_eq!(guard.total_reserved_usd(), 0.0);
    assert!(!guard.is_killswitch_tripped());
    let evidence = f1_capture_private_evidence(
        "F1-01",
        &["f1-01-session"],
        &f,
        &guard,
        3,
        1,
        "NOT_APPLICABLE_LAYER_A",
    );
    f1_emit_result(
        "F1-01",
        "proxy admission before upstream dispatch",
        "third admission succeeds; fourth rejected before dispatch",
        "3 admitted; fourth returned 429; upstream saw 3",
        3,
        3,
        1,
        &guard,
        "NOT_APPLICABLE_LAYER_A",
        evidence,
        "A",
        "PASS",
    );
}

#[tokio::test]
async fn test_cw122b1_f1_02_campaign_request_exhaustion_crosses_sessions() {
    let guard = SpendGuard::new_with_campaign_request_limit(
        SpendGuardConfig {
            authorized_budget_usd: 1.0,
            max_requests_per_session: 10,
            ..Default::default()
        },
        3,
    );
    let f = QualificationFixture::setup_config(Some(guard.clone()), None, true).await;
    for session in ["f1-02-a", "f1-02-a", "f1-02-b"] {
        assert_eq!(f1_send_chat(&f, session).await.status(), StatusCode::OK);
    }
    let rejected = f1_send_chat(&f, "f1-02-new-session").await;
    assert_eq!(rejected.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(
        rejected
            .text()
            .await
            .unwrap()
            .contains("Provider request count limit exceeded")
    );
    assert_eq!(f.mock.captured.lock().unwrap().len(), 3);
    assert_eq!(guard.total_request_count(), 3);
    assert_eq!(guard.session_request_count("f1-02-new-session"), 0);
    assert_eq!(guard.total_reserved_usd(), 0.0);
    assert!(!guard.is_killswitch_tripped());
    let evidence = f1_capture_private_evidence(
        "F1-02",
        &["f1-02-a", "f1-02-b", "f1-02-new-session"],
        &f,
        &guard,
        3,
        1,
        "NOT_APPLICABLE_LAYER_A",
    );
    f1_emit_result(
        "F1-02",
        "shared SpendGuard pre-dispatch admission",
        "global fourth request rejected across sessions",
        "3 admitted across 2 sessions; new session rejected before dispatch",
        3,
        3,
        1,
        &guard,
        "NOT_APPLICABLE_LAYER_A",
        evidence,
        "A",
        "PASS",
    );
}

#[tokio::test]
async fn test_openai_file_credential_replaces_dummy_bearer_only_upstream() {
    const SECRET: &str = "synthetic-upstream-secret-for-test";
    let f =
        QualificationFixture::setup_config_with_upstream_key(None, None, true, Some(SECRET)).await;
    let response = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/openai/v1/chat/completions", f.proxy_port))
        .header("authorization", "Bearer codex-dummy-token")
        .header("x-cw-session-id", "credential-injection-test")
        .header("x-cw-control-token", &f.control_token)
        .json(&json!({"model":"gpt-4o-mini","messages":[{"role":"user","content":"credential probe"}]}))
        .send().await.unwrap();
    let response_body = response.bytes().await.unwrap();
    assert!(!String::from_utf8_lossy(&response_body).contains(SECRET));
    let captured = f.captured.lock().unwrap().clone();
    assert_eq!(captured.len(), 1);
    assert_eq!(
        captured[0].headers["authorization"],
        format!("Bearer {SECRET}")
    );
    assert!(!String::from_utf8_lossy(&captured[0].body).contains(SECRET));
    assert!(!String::from_utf8_lossy(&captured[0].body).contains("codex-dummy-token"));
}

#[tokio::test]
async fn test_invalid_configured_openai_credential_fails_before_dispatch() {
    let f = QualificationFixture::setup_config_with_key_state(
        None, None, false, None, true, false, None,
    )
    .await;
    let response = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/openai/v1/chat/completions", f.proxy_port))
        .header("authorization", "Bearer codex-dummy-token")
        .header("x-cw-session-id", "invalid-credential-test")
        .header("x-cw-control-token", &f.control_token)
        .json(&json!({"model":"gpt-4o-mini","messages":[{"role":"user","content":"must not dispatch"}]}))
        .send().await.unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(f.captured.lock().unwrap().is_empty());
}

#[tokio::test]
async fn test_smoke_provider_model_mismatch_fails_before_dispatch() {
    let f = QualificationFixture::setup_config_with_key_state(
        None,
        None,
        false,
        None,
        false,
        false,
        Some("gpt-6-luna"),
    )
    .await;
    let response = reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{}/openai/v1/responses",
            f.proxy_port
        ))
        .header("x-cw-session-id", "smoke-model-mismatch")
        .header("x-cw-control-token", &f.control_token)
        .json(&json!({"model":"gpt-6-luna-preview","input":"no dispatch"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(f.captured.lock().unwrap().is_empty());
}

#[tokio::test]
async fn test_l0_smoke_rejects_non_openai_provider_fallback_before_dispatch() {
    let guard = SpendGuard::new_with_campaign_request_limit(
        SpendGuardConfig {
            authorized_budget_usd: 0.30,
            max_session_cost_usd: 0.30,
            max_requests_per_session: 1,
            request_reservation_usd: 0.30,
            ..Default::default()
        },
        1,
    );
    let f = QualificationFixture::setup_config_with_key_state(
        Some(guard.clone()),
        None,
        false,
        None,
        false,
        false,
        Some("gpt-6-luna"),
    )
    .await;
    let response = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", f.proxy_port))
        .header("x-cw-session-id", "l0-provider-fallback")
        .header("x-cw-control-token", &f.control_token)
        .json(&json!({"model":"claude-sonnet","max_tokens":8,"messages":[{"role":"user","content":"must not fall back"}]}))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(f.mock.captured.lock().unwrap().is_empty());
    assert_eq!(guard.total_request_count(), 0);
    assert_eq!(guard.total_reserved_usd(), 0.0);
}

#[tokio::test]
async fn test_gpt6_luna_loopback_smoke_settles_and_captures_exact_wire_evidence() {
    let guard = SpendGuard::new_with_campaign_request_limit(
        SpendGuardConfig {
            authorized_budget_usd: 0.30,
            max_session_cost_usd: 0.30,
            max_requests_per_session: 1,
            request_reservation_usd: 0.01,
            ..Default::default()
        },
        1,
    );
    let f = QualificationFixture::setup_config_with_live_identity(
        Some(guard.clone()),
        None,
        true,
        None,
        false,
        true,
        Some(("gpt-6-luna", Some("gpt6-luna-loopback-smoke"))),
    )
    .await;
    let payload = json!({
        "model":"gpt-6-luna",
        "service_tier":"default",
        "stream":false,
        "input":"CW122B2L0 loopback smoke; no external provider",
    });
    let ingress = serde_json::to_vec(&payload).unwrap();
    let response = reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{}/openai/v1/responses",
            f.proxy_port
        ))
        .header("x-cw-session-id", "gpt6-luna-loopback-smoke")
        .header("x-cw-control-token", &f.control_token)
        .header("content-type", "application/json")
        .body(ingress.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response_bytes = response.bytes().await.unwrap().to_vec();
    let response_json: Value = serde_json::from_slice(&response_bytes).unwrap();
    let usage = response_json.get("usage").unwrap();
    assert_eq!(usage["input_tokens"], 1000);
    assert_eq!(usage["input_tokens_details"]["cached_tokens"], 100);
    assert_eq!(usage["input_tokens_details"]["cache_write_tokens"], 500);
    assert_eq!(usage["output_tokens"], 1000);

    let captured = f.mock.captured.lock().unwrap().clone();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].path, "/v1/responses");
    assert_eq!(captured[0].body, ingress);
    assert_eq!(f.mock.responses.lock().unwrap()[0].body, response_bytes);
    assert_eq!(guard.total_request_count(), 1);
    assert_eq!(guard.total_spent_usd(), 0.000641);
    assert_eq!(guard.total_reserved_usd(), 0.0);

    let turns = f.store.list_turns("gpt6-luna-loopback-smoke").unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].model.as_deref(), Some("gpt-6-luna"));
    assert_eq!(turns[0].observed_usage.as_ref(), Some(usage));
    let turn_id = &turns[0].id;
    assert_eq!(
        f.store.get_wire_request(turn_id).unwrap(),
        Some(ingress.clone())
    );
    assert_eq!(f.store.get_wire_egress(turn_id).unwrap(), Some(ingress));
    assert_eq!(
        f.store.get_wire_response(turn_id).unwrap(),
        Some((response_bytes, true, false))
    );
}

#[tokio::test]
async fn test_opt_in_wire_capture_preserves_full_provider_response() {
    let f = QualificationFixture::setup_config_with_key_state(
        None,
        None,
        true,
        None,
        false,
        true,
        Some("gpt-4o-mini"),
    )
    .await;
    let response = reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{}/openai/v1/responses",
            f.proxy_port
        ))
        .header("x-cw-session-id", "response-evidence-test")
        .header("x-cw-control-token", &f.control_token)
        .json(&json!({"model":"gpt-4o-mini","stream":true,"input":"capture exact response"}))
        .send()
        .await
        .unwrap();
    let response_bytes = response.bytes().await.unwrap().to_vec();
    let turns = f.store.list_turns("response-evidence-test").unwrap();
    assert_eq!(turns.len(), 1);
    let turn_id = &turns[0].id;
    assert!(
        !f.store
            .get_wire_request(turn_id)
            .unwrap()
            .unwrap()
            .is_empty()
    );
    assert!(f.store.get_wire_egress(turn_id).unwrap().is_some());
    assert_eq!(
        f.store.get_wire_response(turn_id).unwrap(),
        Some((response_bytes, true, false))
    );
}

#[tokio::test]
async fn test_cw122b2p_campaign_cap_360_rejects_request_361_before_dispatch() {
    let guard = SpendGuard::new_with_campaign_request_limit(
        SpendGuardConfig {
            authorized_budget_usd: 100.0,
            max_requests_per_session: 30,
            require_verified_pricing: false,
            request_reservation_usd: 0.001,
            ..Default::default()
        },
        360,
    );
    let fixture = QualificationFixture::setup_config(Some(guard.clone()), None, false).await;
    for request in 0..360 {
        let session = format!("cw122b2p-session-{}", request / 30);
        let response = f1_send_chat(&fixture, &session).await;
        let status = response.status();
        let body = response.text().await.expect("consume success response");
        assert_eq!(status, StatusCode::OK, "admission {request}: {body}");
    }
    let rejected = f1_send_chat(&fixture, "cw122b2p-new-session-361").await;
    assert_eq!(rejected.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(
        rejected
            .text()
            .await
            .expect("consume rejection response")
            .contains("Provider request count limit exceeded")
    );
    assert_eq!(fixture.mock.captured.lock().unwrap().len(), 360);
    assert_eq!(guard.total_request_count(), 360);
    assert_eq!(guard.session_request_count("cw122b2p-new-session-361"), 0);
}

#[tokio::test]
async fn test_cw122b1_f1_03_retryable_responses_consume_each_admission() {
    let guard = SpendGuard::new(SpendGuardConfig {
        authorized_budget_usd: 1.0,
        max_requests_per_session: 3,
        require_verified_pricing: false,
        ..Default::default()
    });
    let f = QualificationFixture::setup_config(Some(guard.clone()), None, true).await;
    let session = "f1-03-layer-a";
    for _ in 0..3 {
        let response = f1_send_responses(&f, session, "CW122B1_F1-03_RETRY_LOOP").await;
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        let _ = response.bytes().await.unwrap();
    }
    let blocked = f1_send_responses(&f, session, "CW122B1_F1-03_RETRY_LOOP").await;
    assert_eq!(blocked.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(
        blocked
            .text()
            .await
            .unwrap()
            .contains("Provider request count limit exceeded")
    );
    assert_eq!(f.mock.captured.lock().unwrap().len(), 3);
    assert_eq!(guard.session_request_count(session), 3);
    assert_eq!(guard.total_request_count(), 3);
    assert!(!guard.is_killswitch_tripped());
    let evidence = f1_capture_private_evidence(
        "F1-03",
        &[session],
        &f,
        &guard,
        3,
        1,
        "NOT_APPLICABLE_LAYER_A",
    );
    f1_emit_result(
        "F1-03",
        "each mock HTTP 429 response settles before another proxy admission",
        "all retries count toward request cap and fourth request is rejected before provider dispatch",
        "three 429 dispatches admitted; fourth request blocked at session cap",
        3,
        3,
        1,
        &guard,
        "NOT_APPLICABLE_LAYER_A",
        evidence,
        "A",
        "PASS",
    );
}

#[tokio::test]
async fn test_cw122b1_f1_04_http_failures_are_recorded_and_fail_closed() {
    for (scenario, expected) in [
        ("429", StatusCode::TOO_MANY_REQUESTS),
        ("500", StatusCode::INTERNAL_SERVER_ERROR),
        ("503", StatusCode::SERVICE_UNAVAILABLE),
        ("401", StatusCode::UNAUTHORIZED),
        ("403", StatusCode::FORBIDDEN),
    ] {
        let guard = SpendGuard::new(SpendGuardConfig {
            authorized_budget_usd: 1.0,
            ..Default::default()
        });
        let f = QualificationFixture::setup_config(Some(guard.clone()), None, true).await;
        let session = format!("f1-04-{scenario}");
        let marker = format!("CW122B1_F1-04_{scenario}");
        let response = f1_send_responses(&f, &session, &marker).await;
        assert_eq!(response.status(), expected, "scenario {scenario}");
        assert!(
            response
                .headers()
                .get(reqwest::header::CONTENT_LENGTH)
                .is_none()
        );
        let _ = response.bytes().await.unwrap();
        assert_eq!(
            f.mock.captured.lock().unwrap().len(),
            1,
            "scenario {scenario}"
        );
        assert_eq!(guard.session_request_count(&session), 1);
        assert_eq!(guard.total_request_count(), 1);
        assert_eq!(guard.total_reserved_usd(), 0.0);
        assert!(guard.is_killswitch_tripped(), "scenario {scenario}");
        let turns = f.store.list_turns(&session).unwrap();
        assert_eq!(turns.len(), 1);
        assert_eq!(
            turns[0].outcome.as_deref(),
            Some("upstream_error"),
            "scenario {scenario}"
        );
        assert!(turns[0].observed_usage.is_none());
        let evidence = f1_capture_private_evidence(
            "F1-04",
            &[&session],
            &f,
            &guard,
            1,
            0,
            "NOT_APPLICABLE_LAYER_A",
        );
        f1_emit_result(
            "F1-04",
            "mock provider HTTP response",
            "provider HTTP error classified and reservation settled fail-closed",
            &format!("HTTP {} recorded as upstream_error", expected.as_u16()),
            1,
            1,
            0,
            &guard,
            "NOT_APPLICABLE_LAYER_A",
            evidence,
            "A",
            "PASS",
        );
    }
}

#[tokio::test]
async fn test_cw122b1_f1_05_incomplete_sse_cannot_settle_or_complete() {
    for (scenario, expected_body) in [
        ("INVALID_SSE", "not-json"),
        ("TRUNCATED_SSE", "response.completed"),
        ("CLOSE_BEFORE_COMPLETE", "partial"),
    ] {
        let guard = SpendGuard::new(SpendGuardConfig {
            authorized_budget_usd: 1.0,
            ..Default::default()
        });
        let f = QualificationFixture::setup_config(Some(guard.clone()), None, true).await;
        let session = format!("f1-05-{scenario}");
        let marker = format!("CW122B1_F1-05_{scenario}");
        let response = f1_send_responses(&f, &session, &marker).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response
                .headers()
                .get(reqwest::header::CONTENT_LENGTH)
                .is_none()
        );
        let bytes = response.bytes().await.unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains(expected_body));
        assert_eq!(f.mock.captured.lock().unwrap().len(), 1);
        let turns = f.store.list_turns(&session).unwrap();
        assert_eq!(turns.len(), 1);
        assert_eq!(
            turns[0].outcome.as_deref(),
            Some("stream_error"),
            "scenario {scenario}"
        );
        assert!(turns[0].observed_usage.is_none());
        assert!(guard.is_killswitch_tripped());
        assert!(
            guard.total_spent_usd() > 0.0,
            "reservation fallback is an accounting estimate"
        );
        let evidence = f1_capture_private_evidence(
            "F1-05",
            &[&session],
            &f,
            &guard,
            1,
            0,
            "NOT_APPLICABLE_LAYER_A",
        );
        f1_emit_result(
            "F1-05",
            "mock streaming response before terminal completion",
            "incomplete SSE classified stream_error and no valid usage recorded",
            &format!("{scenario} classified stream_error; usage unavailable"),
            1,
            1,
            0,
            &guard,
            "NOT_APPLICABLE_LAYER_A",
            evidence,
            "A",
            "PASS",
        );
    }
}

#[tokio::test]
async fn test_cw122b1_f1_05_stalled_response_times_out_with_no_usage() {
    let guard = SpendGuard::new(SpendGuardConfig {
        authorized_budget_usd: 1.0,
        ..Default::default()
    });
    let f = QualificationFixture::setup_config(Some(guard.clone()), None, true).await;
    let session = "f1-05-stall";
    let request = tokio::time::timeout(
        std::time::Duration::from_millis(100),
        f1_send_responses(&f, session, "CW122B1_F1-05_STALL"),
    )
    .await;
    assert!(
        request.is_err(),
        "the delayed mock response must exceed the test deadline"
    );
    assert_eq!(f.mock.captured.lock().unwrap().len(), 1);
    assert!(f.mock.responses.lock().unwrap().is_empty());
    assert_eq!(guard.total_request_count(), 1);
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if guard.total_reserved_usd() == 0.0 && guard.is_killswitch_tripped() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("cancelled upstream request must settle its reservation");
    let turns = f.store.list_turns(session).unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].outcome.as_deref(), Some("cancelled"));
    assert!(turns[0].observed_usage.is_none());
    let evidence = f1_capture_private_evidence(
        "F1-05",
        &[session],
        &f,
        &guard,
        1,
        0,
        "NOT_APPLICABLE_LAYER_A",
    );
    f1_emit_result(
        "F1-05",
        "mock stalls before response headers",
        "client deadline cancels an admitted request with no provider usage",
        "cancelled outcome recorded and reservation settled conservatively",
        1,
        1,
        0,
        &guard,
        "NOT_APPLICABLE_LAYER_A",
        evidence,
        "A",
        "PASS",
    );
    drop(f);
}

#[tokio::test]
async fn test_cw122b1_f1_06_invalid_or_missing_usage_trips_settlement_guard() {
    for (scenario, expected_usage) in [
        ("MISSING_USAGE", None),
        (
            "INVALID_USAGE",
            Some(json!({"input_tokens":"unknown","output_tokens":4})),
        ),
        (
            "CONTRADICTORY_USAGE",
            Some(
                json!({"input_tokens":1,"output_tokens":4,"input_tokens_details":{"cached_tokens":2}}),
            ),
        ),
    ] {
        let guard = SpendGuard::new(SpendGuardConfig {
            authorized_budget_usd: 1.0,
            ..Default::default()
        });
        let f = QualificationFixture::setup_config(Some(guard.clone()), None, true).await;
        let session = format!("f1-06-{scenario}");
        let marker = format!("CW122B1_F1-06_{scenario}");
        let response = f1_send_responses(&f, &session, &marker).await;
        assert_eq!(response.status(), StatusCode::OK);
        let _ = response.bytes().await.unwrap();
        let turns = f.store.list_turns(&session).unwrap();
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].outcome.as_deref(), Some("completed"));
        assert_eq!(turns[0].observed_usage, expected_usage);
        assert!(guard.is_killswitch_tripped());
        assert!((guard.total_spent_usd() - guard.config().request_reservation_usd).abs() < 1e-6);
        assert_eq!(guard.total_reserved_usd(), 0.0);
        assert_eq!(guard.total_request_count(), 1);
        assert_eq!(
            f1_send_chat(&f, &session).await.status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(f.mock.captured.lock().unwrap().len(), 1);
        let evidence = f1_capture_private_evidence(
            "F1-06",
            &[&session],
            &f,
            &guard,
            1,
            0,
            "NOT_APPLICABLE_LAYER_A",
        );
        f1_emit_result(
            "F1-06",
            "provider usage settlement",
            "missing or invalid telemetry remains unavailable; conservative reservation charged and killswitch trips",
            &format!(
                "{scenario} preserved as {:?}; no measured cost",
                turns[0].observed_usage
            ),
            1,
            1,
            0,
            &guard,
            "NOT_APPLICABLE_LAYER_A",
            evidence,
            "A",
            "PASS",
        );
    }
}

#[tokio::test]
#[ignore = "CW-122B1-F1 active Codex interruption; loopback mock only; run via benchmark/cw122b1/run_fault_matrix.py"]
async fn test_cw122b1_interruption_active_mock_request() {
    use std::time::Duration;

    let binary = std::fs::canonicalize(require_installed_binary("codex")).unwrap();
    let evidence_root = PathBuf::from(
        std::env::var_os("CW122B1_EVIDENCE_DIR")
            .expect("run the active interruption through run_mock_campaign.py"),
    );
    let fault_root = evidence_root.join("faults");
    let scenario_root = fault_root.join("F1-09");
    let layer_root = scenario_root.join("layer-b");
    create_private_dir(&fault_root);
    create_private_dir(&scenario_root);
    create_private_dir(&layer_root);

    let root = std::env::temp_dir().join(format!("cw122b1-f1-interrupt-{}", uuid::Uuid::new_v4()));
    create_private_dir(&root);
    let home = root.join("home");
    let codex_home = root.join("codex");
    let temp_dir = root.join("tmp");
    let workspace = root.join("workspace");
    for path in [&home, &codex_home, &temp_dir, &workspace] {
        create_private_dir(path);
    }
    let session = uuid::Uuid::new_v4().to_string();
    let token = uuid::Uuid::new_v4().simple().to_string();
    let guard = SpendGuard::new(SpendGuardConfig {
        authorized_budget_usd: 1.0,
        require_verified_pricing: false,
        ..Default::default()
    });
    let fixture =
        QualificationFixture::setup_config(Some(guard.clone()), Some(token.clone()), true).await;
    let codex = CodexStep {
        binary: &binary,
        model: "gpt-4o-mini",
        service_tier: None,
        workspace: &workspace,
        home: &home,
        codex_home: &codex_home,
        temp_dir: &temp_dir,
        proxy_port: fixture.proxy_port,
        session_id: &session,
        control_token: &token,
    };
    let captured = fixture.mock.captured.clone();
    let store = fixture.store.clone();
    let monitor_guard = guard.clone();
    let monitor_session = session.clone();
    tokio::spawn(async move {
        loop {
            let request = captured.lock().unwrap().first().cloned();
            if let Some(request) = request {
                let turns = store.list_turns(&monitor_session).unwrap();
                if let Some(turn) = turns.first() {
                    let ingress = store
                        .get_wire_request(&turn.id)
                        .unwrap()
                        .expect("active attempt ingress was persisted");
                    let egress = store
                        .get_wire_egress(&turn.id)
                        .unwrap()
                        .expect("active attempt egress was persisted");
                    let layer_root = PathBuf::from(
                        std::env::var_os("CW122B1_EVIDENCE_DIR").expect("private evidence root"),
                    )
                    .join("faults/F1-09/layer-b");
                    write_private_file(&layer_root.join("ingress.bin"), &ingress);
                    write_private_file(&layer_root.join("egress.bin"), &egress);
                    write_private_file(&layer_root.join("mock-request.bin"), &request.body);
                    let record = json!({
                        "scenario_id":"F1-09",
                        "phase":"Codex request active; loopback mock received egress and withheld response",
                        "session_id":monitor_session,
                        "turn_id":turn.id,
                        "accepted_admissions":monitor_guard.session_request_count(&monitor_session),
                        "upstream_dispatch_count":captured.lock().unwrap().len(),
                        "captured_response_count":0,
                        "reserved_usd":monitor_guard.total_reserved_usd(),
                        "settlement":"NOT_SETTLED_AT_INTERRUPTION",
                        "ingress_sha256":sha256_hex(&ingress),
                        "egress_sha256":sha256_hex(&egress),
                        "mock_request_sha256":sha256_hex(&request.body),
                    });
                    write_private_file(
                        &layer_root.join("active-attempt.json"),
                        &serde_json::to_vec_pretty(&record).unwrap(),
                    );
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });

    let result = codex
        .run(
            None,
            "CW122B1_F1-09_ACTIVE_CODEX wait for the interrupted local mock response",
            Duration::from_secs(600),
        )
        .await;
    panic!("active mock request unexpectedly completed: {result:?}");
}

#[tokio::test]
#[ignore = "CW-122B1-F1 Codex request-cap faults; loopback mock only; run via benchmark/cw122b1/run_fault_matrix.py"]
async fn test_cw122b1_f1_codex_request_caps_block_agent_continuation() {
    use std::time::Duration;

    let binary = std::fs::canonicalize(require_installed_binary("codex")).unwrap();
    let root = std::env::temp_dir().join(format!("cw122b1-f1-caps-{}", uuid::Uuid::new_v4()));
    create_private_dir(&root);
    let env_root = PathBuf::from(
        std::env::var_os("CW122B1_F1_EVIDENCE_DIR")
            .expect("run Codex fault tests through run_fault_matrix.py"),
    );

    let per_session = SpendGuard::new(SpendGuardConfig {
        authorized_budget_usd: 1.0,
        max_requests_per_session: 1,
        require_verified_pricing: false,
        ..Default::default()
    });
    let token = uuid::Uuid::new_v4().simple().to_string();
    let session = uuid::Uuid::new_v4().to_string();
    let fixture =
        QualificationFixture::setup_config(Some(per_session.clone()), Some(token.clone()), true)
            .await;
    let paths: Vec<_> = ["home", "codex", "tmp", "workspace"]
        .iter()
        .map(|name| root.join(format!("session-{name}")))
        .collect();
    for path in &paths {
        create_private_dir(path);
    }
    let codex = CodexStep {
        binary: &binary,
        model: "gpt-4o-mini",
        service_tier: None,
        workspace: &paths[3],
        home: &paths[0],
        codex_home: &paths[1],
        temp_dir: &paths[2],
        proxy_port: fixture.proxy_port,
        session_id: &session,
        control_token: &token,
    };
    let mut partial = PartialCodexEvidence {
        root: env_root.clone(),
        label: format!("F1-01_{session}"),
        session_id: session.clone(),
        store: fixture.store.clone(),
        captured: fixture.mock.captured.clone(),
        responses: fixture.mock.responses.clone(),
        codex_home: Some(paths[1].clone()),
        complete: false,
    };
    let result = codex
        .run(
            None,
            "retry_delays: inspect the implementation and run diagnostics before editing",
            Duration::from_secs(20),
        )
        .await;
    assert!(
        result.is_err(),
        "Codex must see the blocked follow-up request"
    );
    let requests = fixture.mock.captured.lock().unwrap().len();
    assert_eq!(
        requests, 1,
        "second Codex request must be blocked before dispatch"
    );
    assert_eq!(per_session.session_request_count(&session), 1);
    let turns = fixture.store.list_turns(&session).unwrap();
    assert!(
        turns
            .iter()
            .any(|turn| turn.outcome.as_deref() == Some("blocked"))
    );
    let process_status =
        format!("Codex termination status: {result:?}; no live process cleanup required");
    let evidence = f1_capture_private_evidence(
        "F1-01",
        &[&session],
        &fixture,
        &per_session,
        1,
        1,
        &process_status,
    );
    f1_emit_result(
        "F1-01",
        "Codex tool round trip reaches second request after one-request session cap",
        "follow-up is rejected before dispatch and Codex cannot continue",
        &format!("one request reached mock; Codex result: {result:?}"),
        requests,
        1,
        1,
        &per_session,
        &process_status,
        evidence.clone(),
        "B",
        "PASS",
    );
    partial.complete();
    drop(fixture);

    let campaign = SpendGuard::new_with_campaign_request_limit(
        SpendGuardConfig {
            authorized_budget_usd: 1.0,
            max_requests_per_session: 5,
            require_verified_pricing: false,
            ..Default::default()
        },
        1,
    );
    let token = uuid::Uuid::new_v4().simple().to_string();
    let first_session = uuid::Uuid::new_v4().to_string();
    let second_session = uuid::Uuid::new_v4().to_string();
    let fixture =
        QualificationFixture::setup_config(Some(campaign.clone()), Some(token.clone()), true).await;
    let first_paths: Vec<_> = ["home", "codex", "tmp", "workspace"]
        .iter()
        .map(|name| root.join(format!("first-{name}")))
        .collect();
    let second_paths: Vec<_> = ["home", "codex", "tmp", "workspace"]
        .iter()
        .map(|name| root.join(format!("second-{name}")))
        .collect();
    for path in first_paths.iter().chain(&second_paths) {
        create_private_dir(path);
    }
    let first = CodexStep {
        binary: &binary,
        model: "gpt-4o-mini",
        service_tier: None,
        workspace: &first_paths[3],
        home: &first_paths[0],
        codex_home: &first_paths[1],
        temp_dir: &first_paths[2],
        proxy_port: fixture.proxy_port,
        session_id: &first_session,
        control_token: &token,
    };
    assert!(
        first
            .run(None, "complete one mock request", Duration::from_secs(20))
            .await
            .is_ok()
    );
    let second = CodexStep {
        binary: &binary,
        model: "gpt-4o-mini",
        service_tier: None,
        workspace: &second_paths[3],
        home: &second_paths[0],
        codex_home: &second_paths[1],
        temp_dir: &second_paths[2],
        proxy_port: fixture.proxy_port,
        session_id: &second_session,
        control_token: &token,
    };
    let result = second
        .run(None, "try a new campaign session", Duration::from_secs(20))
        .await;
    assert!(result.is_err());
    assert_eq!(fixture.mock.captured.lock().unwrap().len(), 1);
    assert_eq!(campaign.total_request_count(), 1);
    assert_eq!(campaign.session_request_count(&second_session), 0);
    assert!(
        fixture
            .store
            .list_turns(&second_session)
            .unwrap()
            .iter()
            .any(|turn| turn.outcome.as_deref() == Some("blocked"))
    );
    let process_status =
        format!("Second Codex termination status: {result:?}; no live process cleanup required");
    let evidence = f1_capture_private_evidence(
        "F1-02",
        &[&first_session, &second_session],
        &fixture,
        &campaign,
        1,
        1,
        &process_status,
    );
    f1_emit_result(
        "F1-02",
        "second Codex session hits shared campaign cap before upstream dispatch",
        "a new session cannot exceed the campaign-wide request ceiling",
        &format!("one total mock dispatch; second Codex run: {result:?}"),
        1,
        1,
        1,
        &campaign,
        &process_status,
        evidence,
        "B",
        "PASS",
    );
    drop(fixture);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
#[ignore = "CW-122B1-F1 Codex retry fault; loopback mock only; run via benchmark/cw122b1/run_fault_matrix.py"]
async fn test_cw122b1_f1_codex_retry_loop_is_counted_and_bounded() {
    use std::time::Duration;

    let binary = std::fs::canonicalize(require_installed_binary("codex")).unwrap();
    let root = std::env::temp_dir().join(format!("cw122b1-f1-codex-{}", uuid::Uuid::new_v4()));
    create_private_dir(&root);
    let home = root.join("home");
    let codex_home = root.join("codex");
    let temp_dir = root.join("tmp");
    let workspace = root.join("workspace");
    for path in [&home, &codex_home, &temp_dir, &workspace] {
        create_private_dir(path);
    }
    let session = uuid::Uuid::new_v4().to_string();
    let token = uuid::Uuid::new_v4().simple().to_string();
    let guard = SpendGuard::new(SpendGuardConfig {
        authorized_budget_usd: 1.0,
        max_requests_per_session: 3,
        require_verified_pricing: false,
        ..Default::default()
    });
    let f =
        QualificationFixture::setup_config(Some(guard.clone()), Some(token.clone()), true).await;
    let codex = CodexStep {
        binary: &binary,
        model: "gpt-4o-mini",
        service_tier: None,
        workspace: &workspace,
        home: &home,
        codex_home: &codex_home,
        temp_dir: &temp_dir,
        proxy_port: f.proxy_port,
        session_id: &session,
        control_token: &token,
    };
    let evidence_root = PathBuf::from(
        std::env::var_os("CW122B1_F1_EVIDENCE_DIR")
            .expect("run the Codex fault test through run_fault_matrix.py"),
    );
    let mut partial = PartialCodexEvidence {
        root: evidence_root,
        label: format!("F1-03_{session}"),
        session_id: session.clone(),
        store: f.store.clone(),
        captured: f.mock.captured.clone(),
        responses: f.mock.responses.clone(),
        codex_home: Some(codex_home.clone()),
        complete: false,
    };
    let run = codex
        .run(
            None,
            "CW122B1_F1-03_RETRY_LOOP repeatedly receive retryable 429 responses",
            Duration::from_secs(30),
        )
        .await;
    let requests = f.mock.captured.lock().unwrap().len();
    let responses = f.mock.responses.lock().unwrap().clone();
    assert!(requests > 0, "fault was not triggered through Codex");
    assert!(requests <= 3, "Codex exceeded the configured request cap");
    assert_eq!(guard.total_request_count(), requests);
    assert_eq!(responses.len(), requests);
    assert!(responses.iter().all(|response| response.status == 429));
    assert!(run.is_err(), "retry fault must not be reported as success");
    let turns = f.store.list_turns(&session).unwrap();
    let admitted = guard.session_request_count(&session);
    let rejected = turns.len().saturating_sub(admitted);
    assert_eq!(admitted, requests.min(3));
    assert!(
        turns
            .iter()
            .filter(|turn| turn.outcome.as_deref() == Some("upstream_error"))
            .count()
            >= requests
    );
    let process_status =
        format!("Codex termination status: {run:?}; no live process cleanup required");
    let evidence = f1_capture_private_evidence(
        "F1-03",
        &[&session],
        &f,
        &guard,
        admitted,
        rejected,
        &process_status,
    );
    let observed_retry = requests > 1;
    f1_emit_result(
        "F1-03",
        "Codex request routed through authenticated loopback proxy to retryable mock 429",
        "each retry consumes admission; request cap prevents dispatch beyond three",
        &format!("Codex made {requests} observed mock requests; run result: {run:?}"),
        requests,
        admitted,
        rejected,
        &guard,
        &process_status,
        evidence.clone(),
        "B",
        if observed_retry {
            "PASS"
        } else {
            "NOT_VERIFIED"
        },
    );
    f1_emit_result(
        "F1-04",
        "Codex receives HTTP 429 through the loopback proxy",
        "provider failure is classified and does not fall back to another endpoint",
        &format!(
            "{} observed 429 response(s); Codex result: {run:?}",
            responses.len()
        ),
        requests,
        admitted,
        rejected,
        &guard,
        &process_status,
        evidence.clone(),
        "B",
        "PASS",
    );
    partial.complete();

    let stall_guard = SpendGuard::new(SpendGuardConfig {
        authorized_budget_usd: 1.0,
        max_requests_per_session: 3,
        ..Default::default()
    });
    let stall_token = uuid::Uuid::new_v4().simple().to_string();
    let stall_session = uuid::Uuid::new_v4().to_string();
    let stall_fixture = QualificationFixture::setup_config(
        Some(stall_guard.clone()),
        Some(stall_token.clone()),
        true,
    )
    .await;
    let stall_home = root.join("stall-home");
    let stall_codex_home = root.join("stall-codex");
    let stall_tmp = root.join("stall-tmp");
    let stall_workspace = root.join("stall-workspace");
    for path in [&stall_home, &stall_codex_home, &stall_tmp, &stall_workspace] {
        create_private_dir(path);
    }
    let stall_codex = CodexStep {
        binary: &binary,
        model: "gpt-4o-mini",
        service_tier: None,
        workspace: &stall_workspace,
        home: &stall_home,
        codex_home: &stall_codex_home,
        temp_dir: &stall_tmp,
        proxy_port: stall_fixture.proxy_port,
        session_id: &stall_session,
        control_token: &stall_token,
    };
    let mut stall_partial = PartialCodexEvidence {
        root: PathBuf::from(std::env::var_os("CW122B1_F1_EVIDENCE_DIR").unwrap()),
        label: format!("F1-07_{stall_session}"),
        session_id: stall_session.clone(),
        store: stall_fixture.store.clone(),
        captured: stall_fixture.mock.captured.clone(),
        responses: stall_fixture.mock.responses.clone(),
        codex_home: Some(stall_codex_home.clone()),
        complete: false,
    };
    let remaining_campaign = bounded_codex_timeout(Duration::ZERO, Duration::from_secs(7199))
        .expect("one second remains in the simulated campaign");
    assert_eq!(remaining_campaign, Duration::from_secs(1));
    let stall_result = stall_codex
        .run(
            None,
            "CW122B1_F1-07_STALL wait for the hanging local mock response",
            remaining_campaign,
        )
        .await;
    assert!(
        stall_result
            .as_ref()
            .is_err_and(|error| error.contains("process_group_reaped")),
        "per-run deadline must kill and reap Codex: {stall_result:?}"
    );
    assert_eq!(stall_fixture.mock.captured.lock().unwrap().len(), 1);
    assert!(stall_fixture.mock.responses.lock().unwrap().is_empty());
    assert_eq!(
        bounded_codex_timeout(Duration::ZERO, Duration::from_secs(7200)),
        None,
        "campaign expiry must prevent a subsequent Codex attempt"
    );
    let stall_process_status =
        format!("Codex termination status: {stall_result:?}; process-group reap verified");
    let stall_evidence = f1_capture_private_evidence(
        "F1-07",
        &[&stall_session],
        &stall_fixture,
        &stall_guard,
        1,
        0,
        &stall_process_status,
    );
    f1_emit_result(
        "F1-07",
        "Codex run deadline expires while the mock provider stalls before response",
        "Codex and its process group are reaped; no response is treated as completed",
        &format!("one local request; Codex timeout: {stall_result:?}"),
        1,
        1,
        0,
        &stall_guard,
        &stall_process_status,
        stall_evidence.clone(),
        "B",
        "PASS",
    );
    f1_emit_result(
        "F1-08",
        "Codex request is active with one second left in the simulated campaign budget",
        "active attempt is cancelled at the remaining budget and no subsequent attempt starts",
        "one local dispatch was cancelled at the campaign budget; expired budget returned no next-run timeout",
        1,
        1,
        0,
        &stall_guard,
        &stall_process_status,
        f1_capture_private_evidence(
            "F1-08",
            &[&stall_session],
            &stall_fixture,
            &stall_guard,
            1,
            0,
            &stall_process_status,
        ),
        "B",
        "PASS",
    );
    f1_emit_result(
        "F1-05",
        "Codex receives a stalled streaming response until the per-run deadline",
        "stalled response cannot produce success or valid usage and the Codex process is reaped",
        &format!("one local dispatch; timed out and reaped: {stall_result:?}"),
        1,
        1,
        0,
        &stall_guard,
        &stall_process_status,
        f1_capture_private_evidence(
            "F1-05",
            &[&stall_session],
            &stall_fixture,
            &stall_guard,
            1,
            0,
            &stall_process_status,
        ),
        "B",
        "PASS",
    );
    stall_partial.complete();
    drop(stall_fixture);
    let _ = std::fs::remove_dir_all(root);
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
    f.store
        .set_pin_context_policy(session_id, true, true)
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
    f.store
        .set_pin_context_policy(session_id, true, true)
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
    f.store
        .set_pin_context_policy(session_id, true, true)
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
    f.store
        .set_pin_context_policy(session_id, true, true)
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
    f.store
        .set_pin_context_policy(session_id, true, true)
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
async fn test_cw122b1_proxy_preserves_default_redirect_behavior() {
    let f = QualificationFixture::setup().await;
    let redirect_hits = Arc::new(AtomicUsize::new(0));
    let hits = redirect_hits.clone();
    let trap = Router::new().route(
        "/tripwire",
        post(move || {
            let hits = hits.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                StatusCode::OK
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let trap_url = format!("http://{}/tripwire", listener.local_addr().unwrap());
    let trap_task = tokio::spawn(async move { axum::serve(listener, trap).await.unwrap() });
    *f.mock.redirect_target.lock().unwrap() = Some(trap_url.clone());

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let session_id = uuid::Uuid::new_v4().to_string();
    let response = client
        .post(format!(
            "http://127.0.0.1:{}/openai/v1/responses",
            f.proxy_port
        ))
        .header("x-cw-session-id", &session_id)
        .header("x-cw-control-token", &f.control_token)
        .header("x-test-redirect", "yes")
        .json(&json!({"model":"gpt-4o-mini","input":[]}))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(redirect_hits.load(Ordering::SeqCst), 1);
    assert_eq!(f.mock.captured.lock().unwrap().len(), 1);
    trap_task.abort();
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
    f.store
        .set_pin_context_policy(&agy_session_id, true, true)
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
            cmd.env_remove("OPENAI_API_KEY")
                .env("CW_PROXY_DUMMY_API_KEY", "cw-local-proxy-dummy");
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
    f.store.set_pin_context_policy(&sid, true, true).unwrap();

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
            cmd.env_remove("OPENAI_API_KEY")
                .env("CW_PROXY_DUMMY_API_KEY", "cw-local-proxy-dummy");
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

fn codex_thread_id_from_rollout(codex_home: &std::path::Path) -> Option<String> {
    find_rollout_files(&codex_home.join("sessions"))
        .into_iter()
        .find_map(|path| {
            let contents = std::fs::read_to_string(path).ok()?;
            contents.lines().find_map(|line| {
                let value: Value = serde_json::from_str(line).ok()?;
                (value.get("type").and_then(Value::as_str) == Some("session_meta"))
                    .then(|| {
                        value
                            .pointer("/payload/id")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
                    .flatten()
            })
        })
}

fn cw122a_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../benchmark/cw122a")
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[test]
fn test_cw122b1_sha256_known_vector() {
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[cfg(unix)]
fn create_private_dir(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(path).expect("create isolated directory");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .expect("restrict isolated directory");
}

#[cfg(not(unix))]
fn create_private_dir(path: &std::path::Path) {
    std::fs::create_dir_all(path).expect("create isolated directory");
}

fn write_private_file(path: &std::path::Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        create_private_dir(parent);
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    use std::io::Write;
    let mut file = options.open(path).expect("create private evidence file");
    file.write_all(bytes).expect("write private evidence file");
}

struct CodexStep<'a> {
    binary: &'a std::path::Path,
    model: &'a str,
    service_tier: Option<&'a str>,
    workspace: &'a std::path::Path,
    home: &'a std::path::Path,
    codex_home: &'a std::path::Path,
    temp_dir: &'a std::path::Path,
    proxy_port: u16,
    session_id: &'a str,
    control_token: &'a str,
}

impl CodexStep<'_> {
    async fn run(
        &self,
        resume_thread: Option<&str>,
        prompt: &str,
        timeout: std::time::Duration,
    ) -> Result<(Value, u128), String> {
        if timeout.is_zero() {
            return Err("campaign_or_run_deadline_exceeded".into());
        }
        #[cfg(feature = "qualification-fault-hooks")]
        if let Some(hook) = cw_proxy::state::QualificationFaultHook::from_env() {
            let attempt = serde_json::to_vec(&json!({
                "session_id":self.session_id,
                "attempt":"campaign reached Codex launch boundary",
                "first_provider_dispatch_count":0,
            }))
            .unwrap();
            hook.pause(
                "before_first_provider_dispatch",
                json!({
                    "phase":"real Codex campaign reached first launch boundary; Codex process and provider request not yet created",
                    "session_id":self.session_id,
                    "accepted_admissions":0,
                    "rejected_admissions":0,
                    "upstream_dispatch_count":0,
                    "settlement":"NOT_APPLICABLE_BEFORE_ADMISSION",
                }),
                &[("pre-dispatch-attempt.json", attempt.as_slice())],
            )
            .await;
        }
        let path = std::env::var_os("PATH").unwrap_or_default();
        let started = tokio::time::Instant::now();
        let mut cmd = tokio::process::Command::new(self.binary);
        if resume_thread.is_some() {
            cmd.args(["exec", "resume"]);
        } else {
            cmd.args(["exec"]);
        }
        cmd.args([
            "--json",
            "--skip-git-repo-check",
            "--ignore-user-config",
            "--config",
            "approval_policy=\"never\"",
            "--config",
            "sandbox_mode=\"workspace-write\"",
            "--config",
            "sandbox_workspace_write.network_access=false",
            "--config",
            "analytics.enabled=false",
            "--config",
            "otel.metrics_exporter=\"none\"",
        ])
        .args(cw_harnesses::qualification::codex_proxy_config_args(
            self.proxy_port,
        ));
        for key in [
            "CW_CONTROL_TOKEN",
            "CW_SESSION_ID",
            "OPENAI_API_KEY",
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "NO_PROXY",
        ] {
            cmd.args([
                "--config",
                &format!("shell_environment_policy.filters.{key}=\"exclude\""),
            ]);
        }
        cmd.args(codex_model_args(self.model, self.service_tier));
        if let Some(thread) = resume_thread {
            cmd.arg(thread);
        }
        cmd.arg(prompt)
            .current_dir(self.workspace)
            .env_clear()
            .env("PATH", path)
            .env("HOME", self.home)
            .env("CODEX_HOME", self.codex_home)
            .env("TMPDIR", self.temp_dir)
            .env("LANG", "C.UTF-8")
            .env("LC_ALL", "C.UTF-8")
            .env("CW_PROXY_DUMMY_API_KEY", "cw-local-proxy-dummy")
            .env("CW_SESSION_ID", self.session_id)
            .env("CW_CONTROL_TOKEN", self.control_token)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.as_std_mut().process_group(0);
        }
        let mut child = cmd.spawn().map_err(|_| "codex_spawn_failed".to_owned())?;
        let pid = child.id().unwrap_or_default();
        let mut stdout = child.stdout.take().expect("piped Codex stdout");
        let mut stderr = child.stderr.take().expect("piped Codex stderr");
        let stdout_task = tokio::spawn(async move {
            let mut bytes = Vec::new();
            tokio::io::AsyncReadExt::read_to_end(&mut stdout, &mut bytes)
                .await
                .map(|_| bytes)
        });
        let stderr_task = tokio::spawn(async move {
            let mut bytes = Vec::new();
            tokio::io::AsyncReadExt::read_to_end(&mut stderr, &mut bytes)
                .await
                .map(|_| bytes)
        });
        let status = match tokio::time::timeout(timeout, child.wait()).await {
            Ok(Ok(status)) => status,
            Ok(Err(_)) => return Err("codex_wait_failed".into()),
            Err(_) => {
                #[cfg(unix)]
                if pid > 0 {
                    // Kill the isolated Codex group, then wait for the direct child to be reaped.
                    unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
                }
                #[cfg(not(unix))]
                let _ = child.start_kill();
                let status = tokio::time::timeout(std::time::Duration::from_secs(5), child.wait())
                    .await
                    .ok()
                    .and_then(Result::ok);
                let reaped = status.is_some();
                let exit_status = status
                    .map(codex_exit_status)
                    .unwrap_or_else(|| "unavailable".into());
                #[cfg(unix)]
                let group_gone = wait_for_process_group_exit(pid).await;
                #[cfg(not(unix))]
                let group_gone = reaped;
                if group_gone {
                    let _ = stdout_task.await;
                    let _ = stderr_task.await;
                } else {
                    stdout_task.abort();
                    stderr_task.abort();
                }
                return Err(if reaped && group_gone {
                    format!("codex_run_deadline_exceeded_{exit_status}_and_process_group_reaped")
                } else {
                    "codex_run_deadline_exceeded_process_cleanup_unverified".into()
                });
            }
        };
        let output = std::process::Output {
            status,
            stdout: stdout_task
                .await
                .map_err(|_| "codex_stdout_reader_failed")?
                .map_err(|_| "codex_stdout_read_failed")?,
            stderr: stderr_task
                .await
                .map_err(|_| "codex_stderr_reader_failed")?
                .map_err(|_| "codex_stderr_read_failed")?,
        };
        if output.status.success() {
            Ok((
                codex_cli_diagnostics(&output),
                started.elapsed().as_millis(),
            ))
        } else {
            Err(format!("codex_exit_{}", output.status.code().unwrap_or(-1)))
        }
    }
}

fn codex_model_args(model: &str, service_tier: Option<&str>) -> Vec<String> {
    let mut args = vec!["-m".into(), model.into()];
    if let Some(tier) = service_tier {
        args.extend(["--config".into(), format!("service_tier=\"{tier}\"")]);
    }
    args
}

#[test]
fn codex_qualification_model_args_bind_model_and_optional_tier() {
    assert_eq!(codex_model_args("gpt-4o-mini", None), ["-m", "gpt-4o-mini"]);
    assert_eq!(
        codex_model_args("gpt-6-luna", Some("default")),
        ["-m", "gpt-6-luna", "--config", "service_tier=\"default\""]
    );
}

fn codex_exit_status(status: std::process::ExitStatus) -> String {
    if let Some(code) = status.code() {
        return format!("exit_code_{code}");
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return format!("exit_signal_{signal}");
        }
    }
    "exit_status_unknown".into()
}

#[cfg(unix)]
async fn wait_for_process_group_exit(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    for _ in 0..200 {
        if unsafe { libc::kill(-(pid as i32), 0) } != 0 {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    false
}

fn bounded_codex_timeout(
    run_elapsed: std::time::Duration,
    campaign_elapsed: std::time::Duration,
) -> Option<std::time::Duration> {
    let campaign_remaining = std::time::Duration::from_secs(7200).saturating_sub(campaign_elapsed);
    (!campaign_remaining.is_zero()).then(|| {
        std::time::Duration::from_secs(600)
            .saturating_sub(run_elapsed)
            .min(campaign_remaining)
    })
}

fn remaining_codex_timeout(
    run_started: tokio::time::Instant,
    campaign_started: tokio::time::Instant,
) -> Option<std::time::Duration> {
    bounded_codex_timeout(run_started.elapsed(), campaign_started.elapsed())
}

#[test]
fn test_cw122b1_f1_08_campaign_deadline_bounds_current_and_next_run() {
    use std::time::Duration;

    assert_eq!(
        bounded_codex_timeout(Duration::from_secs(10), Duration::from_secs(7199)),
        Some(Duration::from_secs(1)),
        "active run receives only the remaining campaign budget"
    );
    assert_eq!(
        bounded_codex_timeout(Duration::ZERO, Duration::from_secs(7200)),
        None,
        "no subsequent run may begin at or after campaign expiry"
    );
    let evidence = if let Some(root) = std::env::var_os("CW122B1_F1_EVIDENCE_DIR") {
        let path = PathBuf::from(root).join("faults/F1-08/deadline-admission.json");
        let bytes = serde_json::to_vec_pretty(&json!({
            "scenario_id":"F1-08",
            "campaign_elapsed_ms":7200000,
            "remaining_attempt_timeout_ms":null,
            "next_attempt_started":false,
            "provider_dispatches":0,
        }))
        .unwrap();
        write_private_file(&path, &bytes);
        json!({"status":"PRESERVED","directory":path.parent().unwrap(),"files":[
            {"path":"deadline-admission.json","sha256":sha256_hex(&bytes),"kind":"deadline_guard"}
        ]})
    } else {
        json!({"status":"NOT_CONFIGURED_FOR_PRIVATE_CAPTURE"})
    };
    f1_emit_result(
        "F1-08",
        "campaign budget calculation before starting the next attempt",
        "active attempt is limited to remaining time and no attempt starts after expiry",
        "one second remained at 7199 seconds; timeout became unavailable at 7200 seconds",
        0,
        0,
        0,
        &SpendGuard::new(SpendGuardConfig::default()),
        "NOT_APPLICABLE_LAYER_A",
        evidence,
        "A",
        "PASS",
    );
}

fn cw122_oracle_check(family: &str, workspace: &std::path::Path) -> Value {
    let root = cw122a_root();
    let code = "import json,sys; from oracle import check; print(json.dumps(check(sys.argv[1],sys.argv[2],json.load(open(sys.argv[3])))))";
    let output = Command::new("python3")
        .args(["-B", "-c", code, family])
        .arg(workspace)
        .arg(root.join("campaign.json"))
        .current_dir(workspace)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", workspace)
        .env("PYTHONPATH", &root)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .expect("run independent CW-122A oracle");
    assert!(
        output.status.success(),
        "independent oracle subprocess failed"
    );
    serde_json::from_slice(&output.stdout).expect("oracle JSON result")
}

fn is_function_output(value: &Value, call_id: &str) -> bool {
    value.get("type").and_then(Value::as_str) == Some("function_call_output")
        && value.get("call_id").and_then(Value::as_str) == Some(call_id)
}

fn has_consumed_historical_probe(payload: &Value) -> bool {
    let Some(input) = payload.get("input").and_then(Value::as_array) else {
        return false;
    };
    let Some(probe_index) = input
        .iter()
        .position(|item| is_function_output(item, "cw122b1_probe"))
    else {
        return false;
    };
    let assistant_after = input.iter().enumerate().any(|(index, item)| {
        index > probe_index
            && item.get("type").and_then(Value::as_str) == Some("message")
            && item.get("role").and_then(Value::as_str) == Some("assistant")
    });
    let implementation_prompt_after = input.iter().enumerate().any(|(index, item)| {
        index > probe_index
            && item.get("type").and_then(Value::as_str) == Some("message")
            && item.get("role").and_then(Value::as_str) == Some("user")
            && item
                .to_string()
                .contains("Implement the requested change now.")
    });
    assistant_after && implementation_prompt_after
}

fn verified_tool_output_pointer(
    payload: &Value,
    target_pointer: &str,
    call_id: &str,
) -> Option<String> {
    let target = payload.pointer(target_pointer)?;
    if !is_function_output(target, call_id) {
        return None;
    }
    let output_pointer = format!("{target_pointer}/output");
    payload
        .pointer(&output_pointer)?
        .is_string()
        .then_some(output_pointer)
}

fn codex_analytics_enabled(payload: &Value) -> Option<bool> {
    let metadata = payload
        .pointer("/client_metadata/x-codex-turn-metadata")?
        .as_str()?;
    let metadata: Value = serde_json::from_str(metadata).ok()?;
    metadata.get("analytics_enabled")?.as_bool()
}

fn codex_cli_diagnostics(output: &std::process::Output) -> Value {
    use std::collections::BTreeSet;
    let mut warnings = BTreeSet::new();
    let mut tool_error_count = 0u64;
    for line in output
        .stdout
        .split(|byte| *byte == b'\n')
        .chain(output.stderr.split(|byte| *byte == b'\n'))
    {
        let Ok(event) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        if event.get("type").and_then(Value::as_str) != Some("item.completed") {
            continue;
        }
        let Some(item) = event.get("item") else {
            continue;
        };
        match item.get("type").and_then(Value::as_str) {
            Some("error") => {
                let message = item
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                warnings.insert(if message.contains("Model metadata") {
                    "MODEL_METADATA_FALLBACK"
                } else {
                    "CODEX_ERROR_EVENT"
                });
            }
            Some("command_execution")
                if item
                    .get("exit_code")
                    .and_then(Value::as_i64)
                    .is_some_and(|code| code != 0) =>
            {
                tool_error_count += 1;
            }
            _ => {}
        }
    }
    json!({
        "warning_codes": warnings,
        "tool_error_count": tool_error_count,
        "retry_count": null,
        "retry_count_status": "NOT_AVAILABLE",
    })
}

fn differing_json_paths(left: &Value, right: &Value) -> Vec<String> {
    use std::collections::BTreeSet;
    fn walk(left: &Value, right: &Value, path: &str, out: &mut Vec<String>) {
        match (left, right) {
            (Value::Object(left), Value::Object(right)) => {
                let keys: BTreeSet<_> = left.keys().chain(right.keys()).collect();
                for key in keys {
                    let child = format!("{path}/{key}");
                    match (left.get(key), right.get(key)) {
                        (Some(a), Some(b)) => walk(a, b, &child, out),
                        _ => out.push(child),
                    }
                    if out.len() >= 12 {
                        return;
                    }
                }
            }
            (Value::Array(left), Value::Array(right)) => {
                if left.len() != right.len() {
                    out.push(format!("{path}/length"));
                }
                for (index, (a, b)) in left.iter().zip(right).enumerate() {
                    walk(a, b, &format!("{path}/{index}"), out);
                    if out.len() >= 12 {
                        return;
                    }
                }
            }
            _ if left != right => out.push(path.to_owned()),
            _ => {}
        }
    }
    let mut paths = Vec::new();
    walk(left, right, "", &mut paths);
    paths
}

fn save_cw122b1_raw_evidence(
    evidence_dir: Option<&std::path::Path>,
    session_id: &str,
    turns: &[cw_core::ir::ContextTurn],
    store: &CwStore,
    responses: &[CapturedResponse],
) {
    let Some(root) = evidence_dir else { return };
    create_private_dir(&root.join("raw"));
    for (index, turn) in turns.iter().enumerate() {
        let name = format!("raw/{session_id}/request_{index:02}.json");
        let ingress = store
            .get_wire_request(&turn.id)
            .expect("read captured ingress")
            .expect("wire ingress present");
        write_private_file(&root.join(&name), &ingress);
        let egress = store
            .get_wire_egress(&turn.id)
            .expect("read persisted egress")
            .expect("wire egress present");
        write_private_file(
            &root.join(format!("raw/{session_id}/egress_{index:02}.json")),
            &egress,
        );
        if let Some((response, complete, truncated)) = store
            .get_wire_response(&turn.id)
            .expect("read captured provider response")
        {
            write_private_file(
                &root.join(format!("raw/{session_id}/response_{index:02}.sse")),
                &response,
            );
            write_private_file(
                &root.join(format!("raw/{session_id}/response_meta_{index:02}.json")),
                &serde_json::to_vec(&json!({"complete":complete,"truncated":truncated})).unwrap(),
            );
        } else if let Some(response) = responses.get(index) {
            write_private_file(
                &root.join(format!("raw/{session_id}/response_{index:02}.sse")),
                &response.body,
            );
        }
    }
}

struct PartialCodexEvidence {
    root: PathBuf,
    label: String,
    session_id: String,
    store: CwStore,
    captured: Arc<Mutex<Vec<CapturedRequest>>>,
    responses: Arc<Mutex<Vec<CapturedResponse>>>,
    codex_home: Option<PathBuf>,
    complete: bool,
}

impl PartialCodexEvidence {
    fn complete(&mut self) {
        self.complete = true;
    }
}

impl Drop for PartialCodexEvidence {
    fn drop(&mut self) {
        if self.complete {
            return;
        }
        let directory = self.root.join("failed").join(&self.label);
        create_private_dir(&self.root.join("failed"));
        create_private_dir(&directory);
        let turns = self.store.list_turns(&self.session_id).unwrap_or_default();
        for (index, turn) in turns.iter().enumerate() {
            if let Ok(Some(bytes)) = self.store.get_wire_request(&turn.id) {
                let _ = write_private_file_result(
                    &directory.join(format!("ingress_{index:02}.json")),
                    &bytes,
                );
            }
            if let Ok(Some(bytes)) = self.store.get_wire_egress(&turn.id) {
                let _ = write_private_file_result(
                    &directory.join(format!("egress_{index:02}.json")),
                    &bytes,
                );
            }
            if let Ok(Some((bytes, complete, truncated))) = self.store.get_wire_response(&turn.id) {
                let _ = write_private_file_result(
                    &directory.join(format!("response_{index:02}.sse")),
                    &bytes,
                );
                if let Ok(metadata) = serde_json::to_vec(&json!({
                    "complete":complete,
                    "truncated":truncated,
                })) {
                    let _ = write_private_file_result(
                        &directory.join(format!("response_meta_{index:02}.json")),
                        &metadata,
                    );
                }
            }
        }
        if let Ok(captured) = self.captured.lock() {
            for (index, request) in captured.iter().enumerate() {
                let _ = write_private_file_result(
                    &directory.join(format!("provider_request_{index:02}.json")),
                    &request.body,
                );
            }
        }
        if let Ok(responses) = self.responses.lock() {
            for (index, response) in responses.iter().enumerate() {
                let _ = write_private_file_result(
                    &directory.join(format!("provider_response_{index:02}.bin")),
                    &response.body,
                );
            }
        }
        let mut transcript_files = self
            .codex_home
            .as_ref()
            .map(|home| find_rollout_files(&home.join("sessions")))
            .unwrap_or_default();
        transcript_files.sort();
        let mut transcripts_copied = 0usize;
        for (index, path) in transcript_files.iter().enumerate() {
            if let Ok(bytes) = std::fs::read(path)
                && write_private_file_result(
                    &directory.join(format!("codex_rollout_{index:02}.jsonl")),
                    &bytes,
                )
                .is_ok()
            {
                transcripts_copied += 1;
            }
        }
        let status = json!({
            "status":"INCOMPLETE_OR_FAILED",
            "session_id":self.session_id,
            "persisted_turns":turns.len(),
            "captured_provider_requests":self.captured.lock().map(|items| items.len()).unwrap_or(0),
            "captured_provider_responses":self.responses.lock().map(|items| items.len()).unwrap_or(0),
            "codex_transcripts_found":transcript_files.len(),
            "codex_transcripts_copied":transcripts_copied,
            "codex_transcript_evidence":if transcript_files.is_empty() {"NOT_CREATED"} else if transcripts_copied == transcript_files.len() {"PRESERVED"} else {"INCOMPLETE"},
        });
        if let Ok(bytes) = serde_json::to_vec_pretty(&status) {
            let _ = write_private_file_result(&directory.join("status.json"), &bytes);
        }
    }
}

fn write_private_file_result(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        create_private_dir(parent);
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    use std::io::Write;
    let mut file = options.open(path)?;
    file.write_all(bytes)
}

#[test]
fn test_cw122b1_failed_attempt_preserves_partial_wire_evidence() {
    let persistent_root = std::env::var_os("CW122B1_F1_EVIDENCE_DIR").map(PathBuf::from);
    let root = persistent_root
        .as_ref()
        .map(|path| path.join("partial-evidence-boundary"))
        .unwrap_or_else(|| {
            std::env::temp_dir().join(format!("cw122b1-partial-{}", uuid::Uuid::new_v4()))
        });
    create_private_dir(&root);
    let session_id = uuid::Uuid::new_v4().to_string();
    let codex_home = root.join("codex-home");
    let transcript_path = codex_home.join("sessions/2026/11/06/rollout-failure.jsonl");
    create_private_dir(transcript_path.parent().unwrap());
    let transcript = b"{\"type\":\"session_meta\",\"payload\":{\"id\":\"failed-thread\"}}\n";
    std::fs::write(&transcript_path, transcript).unwrap();
    let store = CwStore::open_in_memory().unwrap();
    let captured = Arc::new(Mutex::new(vec![CapturedRequest {
        path: "/v1/responses".into(),
        headers: HeaderMap::new(),
        body: br#"{"input":[]}"#.to_vec(),
        json_body: Some(json!({"input":[]})),
    }]));
    let responses = Arc::new(Mutex::new(vec![CapturedResponse {
        status: 200,
        body: b"data: partial".to_vec(),
    }]));
    let evidence = PartialCodexEvidence {
        root: root.clone(),
        label: "f1-r1_M1_session".into(),
        session_id,
        store,
        captured,
        responses,
        codex_home: Some(codex_home.clone()),
        complete: false,
    };
    drop(evidence);
    let failed = root.join("failed/f1-r1_M1_session");
    assert_eq!(
        std::fs::read(failed.join("provider_request_00.json")).unwrap(),
        br#"{"input":[]}"#
    );
    assert_eq!(
        std::fs::read(failed.join("provider_response_00.bin")).unwrap(),
        b"data: partial"
    );
    assert_eq!(
        std::fs::read(failed.join("codex_rollout_00.jsonl")).unwrap(),
        transcript
    );
    let status: Value =
        serde_json::from_slice(&std::fs::read(failed.join("status.json")).unwrap()).unwrap();
    assert_eq!(status["status"], "INCOMPLETE_OR_FAILED");
    assert_eq!(status["codex_transcripts_found"], 1);
    assert_eq!(status["codex_transcripts_copied"], 1);
    assert_eq!(status["codex_transcript_evidence"], "PRESERVED");
    if persistent_root.is_some() {
        let artifact_names = [
            "provider_request_00.json",
            "provider_response_00.bin",
            "status.json",
        ];
        let files: Vec<_> = artifact_names
            .iter()
            .map(|name| {
                let bytes = std::fs::read(failed.join(name)).unwrap();
                json!({"path":name,"sha256":sha256_hex(&bytes)})
            })
            .collect();
        println!(
            "CW122B1_F1_BOUNDARY:{}",
            json!({
                "boundary_id":"synthetic_failed_attempt_guard_fixture",
                "injection_boundary":"synthetic PartialCodexEvidence drop; not a completed Codex task interruption",
                "result":"PASS",
                "evidence_preservation":{"status":"PRESERVED","directory":failed,"files":files}
            })
        );
    } else {
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[tokio::test]
#[ignore = "CW-122B1 local Codex mock campaign; no provider access; run benchmark/cw122b1/run_mock_campaign.py"]
async fn test_cw122b1_codex_mock_campaign() {
    use std::collections::{BTreeMap, BTreeSet};
    use std::time::Duration;

    let codex_bin = require_installed_binary("codex");
    let codex_bin = std::fs::canonicalize(codex_bin).expect("canonical Codex path");
    let version = Command::new(&codex_bin)
        .arg("--version")
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .output()
        .expect("run Codex version")
        .stdout;
    let version = String::from_utf8_lossy(&version).trim().to_owned();
    assert!(
        version.starts_with("codex-cli "),
        "Codex version not identified"
    );

    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let cwa = cw122a_root();
    let campaign: Value =
        serde_json::from_slice(&std::fs::read(cwa.join("campaign.json")).expect("frozen campaign"))
            .expect("valid frozen campaign");
    assert_eq!(campaign["pairs"].as_array().map(Vec::len), Some(6));
    assert_eq!(campaign["runtime"]["max_runs"], 12);
    assert_eq!(campaign["runtime"]["max_requests_per_run"], 30);

    let evidence_dir = PathBuf::from(
        std::env::var_os("CW122B1_EVIDENCE_DIR")
            .expect("run the ignored campaign through benchmark/cw122b1/run_mock_campaign.py"),
    );
    let evidence_dir = std::fs::canonicalize(evidence_dir).expect("existing evidence directory");
    assert!(!evidence_dir.starts_with(std::fs::canonicalize(&repo).unwrap()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&evidence_dir)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700,
            "raw evidence directory must be private"
        );
    }
    let entries: Vec<_> = std::fs::read_dir(&evidence_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(entries, [std::ffi::OsString::from("launch.json")]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(evidence_dir.join("launch.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    let source_files: BTreeMap<_, _> = [
        "campaign.json",
        "trace-f1.json",
        "trace-f2.json",
        "synthetic-solutions.json",
        "oracles/f1.py",
        "oracles/f2.py",
    ]
    .into_iter()
    .map(|name| {
        (
            name.to_owned(),
            sha256_hex(&std::fs::read(cwa.join(name)).unwrap()),
        )
    })
    .collect();

    let guard = SpendGuard::new_with_campaign_request_limit(
        SpendGuardConfig {
            authorized_budget_usd: 0.20,
            max_requests_per_session: 30,
            require_verified_pricing: false,
            request_reservation_usd: 0.005,
            ..Default::default()
        },
        360,
    );
    let campaign_started = tokio::time::Instant::now();
    let mut attempt_summaries = Vec::new();
    let mut pair_initial_hashes = BTreeMap::<String, Value>::new();
    let mut sampling_reference: Option<Value> = None;
    let mut campaign_request_count = 0usize;

    for pair in campaign["pairs"].as_array().unwrap() {
        let pair_id = pair["pair_id"].as_str().unwrap();
        let family = pair["family"].as_str().unwrap();
        let repetition = pair["repetition"].as_u64().unwrap();
        let instructions = campaign["tasks"][family]["instructions"].as_str().unwrap();
        let initial_prompt = format!(
            "{instructions}\n\nFirst inspect the implementation and run relevant diagnostics. Report what needs fixing without editing files yet."
        );
        let task_prompt_hash = sha256_hex(initial_prompt.as_bytes());
        let implementation_prompt = "Implement the requested change now.";

        for mode in pair["order"].as_array().unwrap() {
            assert!(
                campaign_started.elapsed() < Duration::from_secs(7200),
                "campaign deadline expired"
            );
            let mode = mode.as_str().unwrap();
            let session_id = uuid::Uuid::new_v4().to_string();
            let control_token = uuid::Uuid::new_v4().simple().to_string();
            let attempt_root = std::env::temp_dir().join(format!("cw122b1-{session_id}"));
            create_private_dir(&attempt_root);
            let home = attempt_root.join("home");
            let codex_home = attempt_root.join("codex");
            let temp_dir = attempt_root.join("tmp");
            let workspace = attempt_root.join("workspace");
            for path in [&home, &codex_home, &temp_dir, &workspace] {
                create_private_dir(path);
            }
            for entry in std::fs::read_dir(cwa.join("fixtures").join(family)).unwrap() {
                let entry = entry.unwrap();
                if entry.file_type().unwrap().is_file() {
                    std::fs::copy(entry.path(), workspace.join(entry.file_name())).unwrap();
                }
            }
            let initial_files: BTreeMap<_, _> = std::fs::read_dir(&workspace)
                .unwrap()
                .map(|entry| {
                    let entry = entry.unwrap();
                    let bytes = std::fs::read(entry.path()).unwrap();
                    (
                        entry.file_name().to_string_lossy().into_owned(),
                        sha256_hex(&bytes),
                    )
                })
                .collect();
            assert!(
                !cw122_oracle_check(family, &workspace)["passed"]
                    .as_bool()
                    .unwrap()
            );
            if let Some(previous) =
                pair_initial_hashes.insert(pair_id.to_owned(), json!(initial_files))
            {
                assert_eq!(
                    previous,
                    json!(initial_files),
                    "matched arms began from different fixtures"
                );
            }

            let spend_guard = guard.clone();
            let f = QualificationFixture::setup_config(
                Some(spend_guard),
                Some(control_token.clone()),
                true,
            )
            .await;
            let mut partial_evidence = PartialCodexEvidence {
                root: evidence_dir.clone(),
                label: format!("{pair_id}_{mode}_{session_id}"),
                session_id: session_id.clone(),
                store: f.store.clone(),
                captured: f.mock.captured.clone(),
                responses: f.mock.responses.clone(),
                codex_home: Some(codex_home.clone()),
                complete: false,
            };
            f.store
                .set_autopilot_config(&cw_core::autopilot::AutoPilotConfig {
                    enabled: mode == "M2",
                    auto_diet_at_breakpoint: false,
                    local_model_window_guard: false,
                    auto_heal_low_health: false,
                    auto_cache_safe_pin: false,
                    zero_break_tail_hygiene: false,
                    auto_sync_to_disk: false,
                    ..Default::default()
                })
                .unwrap();
            assert!(
                !f.store
                    .get_pin_context_policy(&session_id)
                    .unwrap()
                    .is_enabled_for(&session_id)
            );
            let codex = CodexStep {
                binary: &codex_bin,
                model: "gpt-4o-mini",
                service_tier: None,
                workspace: &workspace,
                home: &home,
                codex_home: &codex_home,
                temp_dir: &temp_dir,
                proxy_port: f.proxy_port,
                session_id: &session_id,
                control_token: &control_token,
            };
            let started = tokio::time::Instant::now();
            let (first_diagnostics, first_ms) = codex
                .run(
                    None,
                    &initial_prompt,
                    remaining_codex_timeout(started, campaign_started)
                        .expect("campaign deadline expired before attempt"),
                )
                .await
            .unwrap_or_else(|error| panic!(
                "{pair_id}/{mode} initial Codex run failed: {error}; provider_requests={}; responses={:?}; guard_requests={}; killswitch={}; turns={:?}",
                f.mock.captured.lock().unwrap().len(),
                f.mock.responses.lock().unwrap().iter().map(|r| r.status).collect::<Vec<_>>(),
                guard.session_request_count(&session_id),
                guard.is_killswitch_tripped(),
                f.store.list_turns(&session_id).unwrap().iter().map(|turn| (&turn.outcome, &turn.observed_usage)).collect::<Vec<_>>()
            ));
            let thread_id = codex_thread_id_from_rollout(&codex_home)
                .expect("Codex persisted native session identity");
            assert!(!thread_id.is_empty());

            let first_captures = f.mock.captured.lock().unwrap().clone();
            let first_responses = f.mock.responses.lock().unwrap().clone();
            let first_turns = f.store.list_turns(&session_id).unwrap();
            assert!(
                first_captures.len() >= 2,
                "Codex did not perform the probe tool round trip"
            );
            assert_eq!(first_turns.len(), first_captures.len());
            assert_eq!(first_responses.len(), first_captures.len());
            let probe_index = first_captures
                .iter()
                .position(|request| {
                    request
                        .json_body
                        .as_ref()
                        .and_then(|body| body.get("input"))
                        .and_then(Value::as_array)
                        .is_some_and(|items| {
                            items
                                .iter()
                                .any(|item| is_function_output(item, "cw122b1_probe"))
                        })
                })
                .expect("completed probe result reached the provider");
            let probe_raw = f
                .store
                .get_wire_request(&first_turns[probe_index].id)
                .unwrap()
                .unwrap();
            let probe_payload: Value =
                serde_json::from_slice(&probe_raw).expect("probe request JSON");
            assert!(
                probe_payload["input"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|item| {
                        is_function_output(item, "cw122b1_probe")
                            && item
                                .get("output")
                                .is_some_and(|output| !output.to_string().is_empty())
                    }),
                "tool result was not completed with output"
            );
            assert!(
                probe_payload["input"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|item| {
                        is_function_output(item, "cw122b1_probe")
                            && item["output"]
                                .as_str()
                                .is_some_and(|output| output.contains("secret-env-present False"))
                    }),
                "Codex tool environment exposed a credential or proxy variable"
            );
            let consumed_response = &first_responses[probe_index].body;
            assert!(String::from_utf8_lossy(consumed_response).contains("Inspection complete."));
            assert!(first_turns[probe_index].applied_mutation_ids.is_empty());

            let target = parse_request_to_nodes(&probe_payload, ProviderKind::OpenAI, 2)
                .expect("parse historical Codex probe result")
                .into_iter()
                .find(|node| matches!(&node.kind, NodeKind::ToolResult { call_id, .. } if call_id == "cw122b1_probe"))
                .expect("probe result has a canonical tool-result node");
            let stub =
                "[cw:pruned completed diagnostic output; consumed by assistant; original retained]";
            if mode == "M2" {
                f.store
                    .save_mutation(&MutationRule {
                        id: "cw122b1-historical-prune".into(),
                        session_id: session_id.clone(),
                        op: MutationOp::PruneOrStub {
                            target_fingerprint: Some(target.fingerprint.0.clone()),
                            target_call_id: Some("cw122b1_probe".into()),
                            target_json_pointer: Some(target.raw_json_pointer.clone()),
                            stub_text: stub.into(),
                        },
                        status: MutationStatus::Committed,
                        estimated_saved_tokens: 0,
                        estimated_kv_break_tokens: 0,
                        created_at: Utc::now(),
                    })
                    .unwrap();
            }

            let (second_diagnostics, coding_ms) = codex
                .run(
                    Some(&thread_id),
                    implementation_prompt,
                    remaining_codex_timeout(started, campaign_started)
                        .expect("campaign deadline expired before resumed attempt"),
                )
                .await
                .unwrap_or_else(|error| {
                    panic!("{pair_id}/{mode} resumed Codex run failed: {error}")
                });
            assert_eq!(
                codex_thread_id_from_rollout(&codex_home).as_deref(),
                Some(thread_id.as_str())
            );
            let acceptance = cw122_oracle_check(family, &workspace);
            assert!(
                acceptance["passed"].as_bool().unwrap(),
                "independent acceptance failed"
            );

            let captures = f.mock.captured.lock().unwrap().clone();
            let responses = f.mock.responses.lock().unwrap().clone();
            let turns = f.store.list_turns(&session_id).unwrap();
            assert!(captures.len() <= 30, "per-run request cap exceeded");
            campaign_request_count = campaign_request_count.saturating_add(captures.len());
            assert!(
                campaign_request_count <= 360,
                "fixed 12-run campaign request cap exceeded"
            );
            assert_eq!(turns.len(), captures.len());
            assert_eq!(responses.len(), captures.len());
            assert_eq!(guard.session_request_count(&session_id), captures.len());
            save_cw122b1_raw_evidence(
                Some(evidence_dir.as_path()),
                &session_id,
                &turns,
                &f.store,
                &responses,
            );
            let mut verified_prunes = 0usize;
            let mut request_records = Vec::new();
            let mut unique_tool_calls = BTreeSet::new();
            let mut unique_tool_outputs = BTreeSet::new();
            for (index, ((captured, response), turn)) in
                captures.iter().zip(&responses).zip(&turns).enumerate()
            {
                assert_eq!(captured.path, "/v1/responses");
                assert!(response.status == StatusCode::OK.as_u16());
                assert!(!captured.headers.contains_key("x-cw-control-token"));
                assert!(!captured.headers.contains_key("x-cw-session-id"));
                assert!(!String::from_utf8_lossy(&captured.body).contains(&control_token));
                assert!(!String::from_utf8_lossy(&captured.body).contains("cw122b1-mock-only-key"));
                let ingress = f.store.get_wire_request(&turn.id).unwrap().unwrap();
                let persisted = f.store.get_wire_egress(&turn.id).unwrap().unwrap();
                assert_eq!(
                    persisted, captured.body,
                    "persisted egress differs from mock provider request"
                );
                assert_eq!(
                    f.store.get_turn_raw_payload(&turn.id).unwrap().unwrap(),
                    serde_json::from_slice::<Value>(&ingress).unwrap()
                );
                let original: Value = serde_json::from_slice(&ingress).expect("ingress JSON");
                let actual: Value =
                    serde_json::from_slice(&captured.body).expect("provider egress JSON");
                assert_eq!(
                    codex_analytics_enabled(&original),
                    Some(false),
                    "Codex analytics metadata was not disabled"
                );
                let historical = has_consumed_historical_probe(&original);
                let output_pointer = verified_tool_output_pointer(
                    &original,
                    &target.raw_json_pointer,
                    "cw122b1_probe",
                );
                let contains_target = output_pointer.is_some();
                let should_prune = mode == "M2" && historical && contains_target;
                let mut expected = original.clone();
                if should_prune {
                    *expected
                        .pointer_mut(output_pointer.as_ref().unwrap())
                        .expect("prune output pointer") = json!(stub);
                    verified_prunes += 1;
                }
                assert_eq!(
                    actual,
                    expected,
                    "unexpected full request transformation at request {index}; differing_paths={:?}; mode={mode}; historical={historical}; contains_target={contains_target}; target_pointer={}",
                    differing_json_paths(&actual, &expected),
                    target.raw_json_pointer
                );
                if mode == "M1" || !should_prune {
                    assert_eq!(captured.body, ingress, "unmodified request bytes changed");
                    assert!(
                        turn.applied_mutation_ids.is_empty(),
                        "unexpected rule application"
                    );
                } else {
                    assert_eq!(turn.applied_mutation_ids, vec!["cw122b1-historical-prune"]);
                }
                if let Some(input) = original.get("input").and_then(Value::as_array) {
                    for item in input {
                        if item.get("type").and_then(Value::as_str) == Some("function_call")
                            && let Some(call_id) = item.get("call_id").and_then(Value::as_str)
                        {
                            unique_tool_calls.insert(call_id.to_owned());
                        }
                        if item.get("type").and_then(Value::as_str) == Some("function_call_output")
                            && let Some(call_id) = item.get("call_id").and_then(Value::as_str)
                        {
                            unique_tool_outputs.insert(call_id.to_owned());
                        }
                    }
                }
                let usage = response
                    .body
                    .split(|byte| *byte == b'\n')
                    .filter_map(|line| {
                        let line = line.strip_prefix(b"data: ")?;
                        serde_json::from_slice::<Value>(line).ok()
                    })
                    .find(|event| {
                        event.get("type").and_then(Value::as_str) == Some("response.completed")
                    })
                    .and_then(|event| event.pointer("/response/usage").cloned());
                let provider_usage = turn.observed_usage.clone();
                assert!(
                    usage.is_some() && provider_usage.is_some(),
                    "mock provider usage missing"
                );
                assert_eq!(provider_usage.as_ref().unwrap(), usage.as_ref().unwrap());
                let request_sampling = json!({
                    "temperature": original.get("temperature"),
                    "top_p": original.get("top_p"),
                    "reasoning": original.get("reasoning"),
                });
                if let Some(previous) = sampling_reference.as_ref() {
                    assert_eq!(
                        previous, &request_sampling,
                        "effective request sampling fields changed"
                    );
                } else {
                    sampling_reference = Some(request_sampling.clone());
                }
                request_records.push(json!({
                    "index":index,
                    "path":captured.path,
                    "status":response.status,
                    "ingress_sha256":sha256_hex(&ingress),
                    "egress_sha256":sha256_hex(&captured.body),
                    "response_sha256":sha256_hex(&response.body),
                    "model":original.get("model"),
                    "sampling":request_sampling,
                    "codex_analytics_enabled":codex_analytics_enabled(&original),
                    "provider_reported_usage":usage,
                    "usage_provenance":"synthetic_local_mock_response",
                    "cached_tokens":turn.observed_usage.as_ref().and_then(|value| value.pointer("/input_tokens_details/cached_tokens").and_then(Value::as_u64)),
                    "first_byte_latency_ms":null,
                    "first_byte_latency_provenance":"NOT_AVAILABLE",
                    "applied_rule_ids":turn.applied_mutation_ids,
                    "mutation_skips":turn.skipped_mutations,
                }));
            }
            if mode == "M2" {
                assert!(
                    verified_prunes > 0,
                    "no verified stateful/historical pruning treatment reached Codex"
                );
            } else {
                assert_eq!(verified_prunes, 0);
            }

            let rollouts = find_rollout_files(&codex_home.join("sessions"));
            assert!(
                !rollouts.is_empty(),
                "Codex did not persist an isolated native transcript"
            );
            let mut rollout_hashes = Vec::new();
            for (index, path) in rollouts.iter().enumerate() {
                let bytes = std::fs::read(path).expect("read isolated Codex transcript");
                let contents = String::from_utf8_lossy(&bytes);
                assert!(!contents.contains(&control_token));
                assert!(!contents.contains("cw122b1-mock-only-key"));
                assert!(
                    !contents.contains(stub),
                    "proxy stub leaked into Codex's persisted transcript"
                );
                rollout_hashes.push(sha256_hex(&bytes));
                write_private_file(
                    &evidence_dir.join(format!("raw/{session_id}/rollout_{index:02}.jsonl")),
                    &bytes,
                );
            }

            #[cfg(feature = "qualification-fault-hooks")]
            if let Some(hook) = f.qualification_fault_hook.as_ref() {
                let turn = turns.last().expect("accepted task has a provider turn");
                let acceptance_bytes = serde_json::to_vec_pretty(&acceptance).unwrap();
                let ingress = f.store.get_wire_request(&turn.id).unwrap().unwrap();
                let egress = f.store.get_wire_egress(&turn.id).unwrap().unwrap();
                let response = &responses.last().expect("accepted task has a response").body;
                hook.pause(
                    "after_task_before_final_manifest",
                    json!({
                        "phase":"Codex coding task returned and independent acceptance passed; per-attempt and campaign manifests not yet written",
                        "pair_id":pair_id,
                        "mode":mode,
                        "family":family,
                        "session_id":session_id,
                        "acceptance_passed":acceptance["passed"],
                        "upstream_dispatch_count":captures.len(),
                        "accepted_admissions":guard.session_request_count(&session_id),
                        "rejected_admissions":0,
                        "settlement":"completed mock usage settled",
                        "applied_mutation_ids":turn.applied_mutation_ids,
                        "skipped_mutations":turn.skipped_mutations,
                        "process_cleanup":"supervisor_verified",
                    }),
                    &[
                        ("task_acceptance.json", acceptance_bytes.as_slice()),
                        ("final_ingress.bin", ingress.as_slice()),
                        ("final_egress.bin", egress.as_slice()),
                        ("final_provider_response.bin", response.as_slice()),
                    ],
                )
                .await;
            }

            let duration_ms = started.elapsed().as_millis();
            let report = json!({
                "pair_id":pair_id,
                "family":family,
                "repetition":repetition,
                "mode":mode,
                "session_id":session_id,
                "codex_path":codex_bin,
                "codex_version":version,
                "provider":"local_mock_only",
                "protocol":"openai_responses_sse",
                "model":"gpt-4o-mini",
                "model_revision":"SYNTHETIC_NOT_A_MODEL_REVISION",
                "codex_thread_id":thread_id,
                "task_instructions_sha256":sha256_hex(instructions.as_bytes()),
                "initial_prompt_sha256":task_prompt_hash,
                "implementation_prompt_sha256":sha256_hex(implementation_prompt.as_bytes()),
                "initial_fixture_hashes":initial_files,
                "acceptance":acceptance,
                "request_count":captures.len(),
                "tool_call_ids":unique_tool_calls,
                "tool_output_ids":unique_tool_outputs,
                "tool_error_count":first_diagnostics["tool_error_count"].as_u64().unwrap_or(0)+second_diagnostics["tool_error_count"].as_u64().unwrap_or(0),
                "provider_error_count":responses.iter().filter(|response| !(200..300).contains(&response.status)).count(),
                "retry_count":null,
                "retry_count_status":"NOT_AVAILABLE",
                "codex_cli_diagnostics":{"probe":first_diagnostics,"implementation":second_diagnostics},
                "bootstrap_wall_ms":first_ms,
                "coding_wall_ms":coding_ms,
                "complete_session_wall_ms":duration_ms,
                "timing_provenance":"local_codex_subprocess_wall_clock",
                "sampling_observed":sampling_reference,
                "requests":request_records,
                "treatment":{"rule_id":"cw122b1-historical-prune","verified_requests":verified_prunes,"status":if mode=="M2" {"verified_mock_wire_treatment"} else {"not_applied_passthrough"}},
                "cache":{"initial_state":"MOCK_PROVIDER_NO_CACHE","cached_token_telemetry":"synthetic_mock_only","cache_fairness":"NOT_EVALUATED_FOR_PROVIDER"},
                "transcript":{"files":rollouts.len(),"sha256":rollout_hashes,"stub_absent":true,"source_fixture_unchanged":true},
                "status":"SYNTHETIC_MOCK_EXECUTION_ACCEPTED",
            });
            write_private_file(
                &evidence_dir.join(format!("attempts/{pair_id}_{mode}.json")),
                &(serde_json::to_vec_pretty(&report).unwrap()),
            );
            partial_evidence.complete();
            attempt_summaries.push(report);

            drop(f);
            std::fs::remove_dir_all(attempt_root).expect("remove isolated run workspace and HOME");
        }
    }
    assert_eq!(attempt_summaries.len(), 12);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(evidence_dir.join("raw"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700,
            "nested raw evidence directory must be private"
        );
    }
    assert_eq!(
        campaign_request_count,
        attempt_summaries
            .iter()
            .map(|r| r["request_count"].as_u64().unwrap() as usize)
            .sum::<usize>()
    );
    assert!(campaign_started.elapsed() < Duration::from_secs(7200));
    assert_eq!(
        source_files,
        [
            "campaign.json",
            "trace-f1.json",
            "trace-f2.json",
            "synthetic-solutions.json",
            "oracles/f1.py",
            "oracles/f2.py",
        ]
        .into_iter()
        .map(|name| (
            name.to_owned(),
            sha256_hex(&std::fs::read(cwa.join(name)).unwrap())
        ))
        .collect()
    );

    let manifest = json!({
        "scope":"SYNTHETIC_CODEX_MOCK_QUALIFICATION",
        "status":"MOCK_CODEX_CAMPAIGN_PASS",
        "hash_algorithm":"SHA-256",
        "live_execution":"NO_GO; this test only routes to local mock",
        "inference_calls":0,
        "provider_spend":0,
        "codex_path":codex_bin,
        "codex_version":version,
        "context_watcher_commit":std::env::var("CW122B1_COMMIT_SHA").unwrap_or_else(|_|"NOT_AVAILABLE".into()),
        "cw122a_freeze_sha256":std::env::var("CW122B1_CW122A_FREEZE_SHA256").unwrap_or_else(|_|"NOT_AVAILABLE".into()),
        "proxy_protocol":"OpenAI Responses SSE through authenticated loopback Context-Watcher proxy",
        "codex_analytics_enabled":false,
        "codex_tool_network_access":"DISABLED",
        "campaign_sha256":sha256_hex(&std::fs::read(cwa.join("campaign.json")).unwrap()),
        "requests":campaign_request_count,
        "runs":attempt_summaries.len(),
        "elapsed_ms":campaign_started.elapsed().as_millis(),
        "sampling_observed":sampling_reference,
        "cache_fairness":"NOT_EVALUATED; synthetic mock endpoint has no provider cache",
        "product_value":"NOT_EVALUATED",
        "attempts":attempt_summaries,
    });
    write_private_file(
        &evidence_dir.join("manifest.json"),
        &serde_json::to_vec_pretty(&manifest).unwrap(),
    );
    println!("CW122B1_MOCK_CAMPAIGN: {}", serde_json::to_string(&json!({"status":manifest["status"],"runs":manifest["runs"],"requests":manifest["requests"],"provider_spend":0,"product_value":"NOT_EVALUATED"})).unwrap());
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
            cmd.env_remove("OPENAI_API_KEY")
                .env("CW_PROXY_DUMMY_API_KEY", "cw-local-proxy-dummy");
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
    f.store
        .set_pin_context_policy(&cw_session_id, true, true)
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
            cmd.env_remove("OPENAI_API_KEY")
                .env("CW_PROXY_DUMMY_API_KEY", "cw-local-proxy-dummy");
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
            cmd.env_remove("OPENAI_API_KEY")
                .env("CW_PROXY_DUMMY_API_KEY", "cw-local-proxy-dummy");
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
            cmd.env_remove("OPENAI_API_KEY")
                .env("CW_PROXY_DUMMY_API_KEY", "cw-local-proxy-dummy");
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
            cmd.env_remove("OPENAI_API_KEY")
                .env("CW_PROXY_DUMMY_API_KEY", "cw-local-proxy-dummy");
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
            cmd.env_remove("OPENAI_API_KEY")
                .env("CW_PROXY_DUMMY_API_KEY", "cw-local-proxy-dummy");
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
            cmd.env_remove("OPENAI_API_KEY")
                .env("CW_PROXY_DUMMY_API_KEY", "cw-local-proxy-dummy");
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
    f.store.set_pin_context_policy(&sid_a, true, true).unwrap();

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
            cmd.env_remove("OPENAI_API_KEY")
                .env("CW_PROXY_DUMMY_API_KEY", "cw-local-proxy-dummy");
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
            cmd.env_remove("OPENAI_API_KEY")
                .env("CW_PROXY_DUMMY_API_KEY", "cw-local-proxy-dummy");
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
        f.store.set_pin_context_policy(sid, true, true).unwrap();
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
    let turn1_requests = captured1
        .iter()
        .filter(|request| {
            String::from_utf8_lossy(&request.body).contains("Turn 1 pristine prompt text")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        turn1_requests.len(),
        1,
        "expected one task request among {} OMP provider requests",
        captured1.len()
    );
    assert_eq!(turn1_requests[0].path, "/v1/chat/completions");

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
    f.store
        .set_pin_context_policy(&canonical_sid, true, true)
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
    store.set_pin_context_policy(&sid, true, true).unwrap();

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
