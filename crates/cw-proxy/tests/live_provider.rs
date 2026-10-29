use axum::{body::Body, http::Request};
use cw_proxy::{DaemonState, build_proxy_router};
use cw_store::CwStore;
use serde_json::Value;
use tower::ServiceExt;

#[tokio::test]
#[ignore = "live OpenRouter Gemini qualification; requires OPENROUTER_API_KEY"]
async fn real_openrouter_gemini_reasoning_details_survive_proxy_forwarding() {
    let api_key = std::env::var("OPENROUTER_API_KEY")
        .expect("set OPENROUTER_API_KEY to run the live-provider qualification");
    let model = std::env::var("CW_R7_OPENROUTER_GEMINI_MODEL")
        .unwrap_or_else(|_| "google/gemini-3.8-flash".to_owned());
    let mut state = DaemonState::new(CwStore::open_in_memory().unwrap());
    state.openai_upstream = "https://openrouter.ai/api".to_owned();
    let app = build_proxy_router(state);
    let payload = serde_json::json!({
        "model": model,
        "max_tokens": 384,
        "reasoning": {"effort":"low"},
        "provider": {"order":["google-ai-studio"],"allow_fallbacks":false},
        "messages": [{
            "role": "user",
            "content": "Call lookup with query set to signature passthrough check."
        }],
        "tools": [{
            "type": "function",
            "function": {
                "name": "lookup",
                "description": "No-op used to verify a model tool response.",
                "parameters": {
                    "type": "object",
                    "properties": {"query": {"type": "string"}},
                    "required": ["query"]
                }
            }
        }],
        "tool_choice": {"type":"function","function":{"name":"lookup"}}
    });
    let request = Request::builder()
        .method("POST")
        .uri("/openai/v1/chat/completions")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {api_key}"))
        .body(Body::from(payload.to_string()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert!(
        response.status().is_success(),
        "OpenRouter returned HTTP {}; response body omitted to avoid logging provider data",
        response.status()
    );
    let bytes = axum::body::to_bytes(response.into_body(), 1_000_000)
        .await
        .unwrap();
    let response: Value = serde_json::from_slice(&bytes).expect("OpenRouter response must be JSON");
    let message = response.pointer("/choices/0/message");
    assert!(
        message
            .and_then(|message| message.get("tool_calls"))
            .and_then(Value::as_array)
            .is_some_and(|calls| !calls.is_empty()),
        "OpenRouter response contained no requested tool call"
    );
    assert!(
        message
            .and_then(|message| message.get("reasoning_details"))
            .and_then(Value::as_array)
            .is_some_and(|details| !details.is_empty()),
        "OpenRouter response contained no reasoning_details"
    );
}

#[tokio::test]
#[ignore = "live OpenRouter Anthropic qualification; requires OPENROUTER_API_KEY"]
async fn real_openrouter_anthropic_signature_event_survives_proxy_streaming() {
    let api_key = std::env::var("OPENROUTER_API_KEY")
        .expect("set OPENROUTER_API_KEY to run the live-provider qualification");
    let model = std::env::var("CW_R7_OPENROUTER_ANTHROPIC_MODEL")
        .unwrap_or_else(|_| "anthropic/claude-haiku-5.5".to_owned());
    let mut state = DaemonState::new(CwStore::open_in_memory().unwrap());
    state.anthropic_upstream = "https://openrouter.ai/api".to_owned();
    let app = build_proxy_router(state);
    let payload = serde_json::json!({
        "model": model,
        "stream": true,
        "max_tokens": 1024,
        "thinking": {"type":"adaptive","display":"summarized"},
        "provider": {"order":["anthropic"],"allow_fallbacks":false},
        "messages": [{
            "role": "user",
            "content": "Work through 1937 multiplied by 2941, then return only the integer."
        }]
    });
    let request = Request::builder()
        .method("POST")
        .uri("/anthropic/v1/messages")
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .header("anthropic-version", "2023-06-01")
        .header("authorization", format!("Bearer {api_key}"))
        .body(Body::from(payload.to_string()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert!(
        response.status().is_success(),
        "OpenRouter returned HTTP {}; response body omitted to avoid logging provider data",
        response.status()
    );
    assert!(
        response
            .headers()
            .get("content-type")
            .is_some_and(|value| value.as_bytes().starts_with(b"text/event-stream"))
    );
    let bytes = axum::body::to_bytes(response.into_body(), 1_000_000)
        .await
        .unwrap();
    let stream = String::from_utf8(bytes.to_vec()).expect("OpenRouter SSE must be UTF-8");
    let has_signature = stream
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .any(|event| {
            serde_json::from_str::<Value>(event).is_ok_and(|event| {
                event.pointer("/delta/type").and_then(Value::as_str) == Some("signature_delta")
                    && event
                        .pointer("/delta/signature")
                        .and_then(Value::as_str)
                        .is_some_and(|signature| !signature.is_empty())
            })
        });
    assert!(
        has_signature,
        "OpenRouter response contained no nonempty Anthropic signature_delta event"
    );
}

#[tokio::test]
#[ignore = "live Anthropic qualification; requires ANTHROPIC_API_KEY and CW_R7_ANTHROPIC_MODEL"]
async fn real_anthropic_signature_event_survives_proxy_streaming() {
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .expect("set ANTHROPIC_API_KEY to run the live-provider qualification");
    let model = std::env::var("CW_R7_ANTHROPIC_MODEL")
        .expect("set CW_R7_ANTHROPIC_MODEL to a thinking-capable Anthropic model");
    let thinking_mode =
        std::env::var("CW_R7_ANTHROPIC_THINKING").unwrap_or_else(|_| "adaptive".to_owned());
    let thinking = match thinking_mode.as_str() {
        "adaptive" => serde_json::json!({"type":"adaptive","display":"summarized"}),
        "enabled" => serde_json::json!({"type":"enabled","budget_tokens":1024}),
        _ => panic!("CW_R7_ANTHROPIC_THINKING must be adaptive or enabled"),
    };
    let mut state = DaemonState::new(CwStore::open_in_memory().unwrap());
    state.anthropic_upstream = "https://api.anthropic.com".to_owned();
    let app = build_proxy_router(state);
    let payload = serde_json::json!({
        "model": model,
        "max_tokens": 2048,
        "thinking": thinking,
        "messages": [{ "role": "user", "content": "Calculate 1937 multiplied by 2941 exactly. Return only the integer." }]
    });
    let request = Request::builder()
        .method("POST")
        .uri("/anthropic/v1/messages")
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .header("anthropic-version", "2023-06-01")
        .header("x-api-key", api_key)
        .body(Body::from(payload.to_string()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert!(
        response.status().is_success(),
        "Anthropic returned HTTP {}; response body omitted to avoid logging provider data",
        response.status()
    );
    assert!(
        response
            .headers()
            .get("content-type")
            .is_some_and(|value| value.as_bytes().starts_with(b"text/event-stream"))
    );
    let bytes = axum::body::to_bytes(response.into_body(), 1_000_000)
        .await
        .unwrap();
    let stream = String::from_utf8(bytes.to_vec()).expect("Anthropic SSE must be UTF-8");
    let has_signature = stream
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .any(|event| {
            serde_json::from_str::<Value>(event).is_ok_and(|event| {
                event.pointer("/delta/type").and_then(Value::as_str) == Some("signature_delta")
                    && event
                        .pointer("/delta/signature")
                        .and_then(Value::as_str)
                        .is_some_and(|signature| !signature.is_empty())
            })
        });
    assert!(
        has_signature,
        "provider response contained no nonempty signature_delta event"
    );
}

#[tokio::test]
#[ignore = "live Gemini qualification; requires GEMINI_API_KEY and CW_R7_GEMINI_MODEL"]
async fn real_gemini_function_signature_survives_proxy_forwarding() {
    let api_key = std::env::var("GEMINI_API_KEY")
        .expect("set GEMINI_API_KEY to run the live-provider qualification");
    let model = std::env::var("CW_R7_GEMINI_MODEL")
        .expect("set CW_R7_GEMINI_MODEL to a Gemini model with function-call signatures");
    assert!(
        !model.is_empty()
            && model
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')),
        "CW_R7_GEMINI_MODEL must be a model ID"
    );
    let mut state = DaemonState::new(CwStore::open_in_memory().unwrap());
    state.gemini_upstream = "https://generativelanguage.googleapis.com".to_owned();
    let app = build_proxy_router(state);
    let payload = serde_json::json!({
        "contents": [{
            "role": "user",
            "parts": [{ "text": "Call lookup with query set to 'signature passthrough check'." }]
        }],
        "tools": [{
            "functionDeclarations": [{
                "name": "lookup",
                "description": "A no-op function used only to verify the model response.",
                "parameters": {
                    "type": "OBJECT",
                    "properties": { "query": { "type": "STRING" } },
                    "required": ["query"]
                }
            }]
        }],
        "toolConfig": { "functionCallingConfig": { "mode": "ANY" } }
    });
    let request = Request::builder()
        .method("POST")
        .uri(format!("/gemini/v1beta/models/{model}:generateContent"))
        .header("content-type", "application/json")
        .header("x-goog-api-key", api_key)
        .body(Body::from(payload.to_string()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert!(
        response.status().is_success(),
        "Gemini returned HTTP {}; response body omitted to avoid logging provider data",
        response.status()
    );
    let bytes = axum::body::to_bytes(response.into_body(), 1_000_000)
        .await
        .unwrap();
    let response: Value = serde_json::from_slice(&bytes).expect("Gemini response must be JSON");
    let has_signed_function_call = response
        .pointer("/candidates/0/content/parts")
        .and_then(Value::as_array)
        .is_some_and(|parts| {
            parts.iter().any(|part| {
                part.get("functionCall").is_some()
                    && part
                        .get("thoughtSignature")
                        .and_then(Value::as_str)
                        .is_some_and(|signature| !signature.is_empty())
            })
        });
    assert!(
        has_signed_function_call,
        "provider response contained no functionCall with a nonempty thoughtSignature"
    );
}
