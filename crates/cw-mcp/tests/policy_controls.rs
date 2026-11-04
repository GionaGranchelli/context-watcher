//! Agent-callable inspection cannot grant or revoke consent, even with forged acknowledgement fields.
use axum::Router;
use cw_core::ir::{HarnessKind, MutationOp, ProviderKind, Session};
use cw_mcp::{handle_mcp_request, mcp_tool_definitions};
use cw_proxy::{DaemonState, build_api_router};
use cw_store::CwStore;
use serde_json::{Value, json};

async fn call(client: &reqwest::Client, base: &str, name: &str, args: Value) -> Value {
    handle_mcp_request(
        client,
        base,
        Some(json!(1)),
        "tools/call",
        &json!({"name":name,"arguments":args}),
    )
    .await
    .unwrap()["result"]
        .clone()
}
async fn serve(app: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    (
        base,
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        }),
    )
}
#[tokio::test]
async fn inspection_and_manual_pin_creation_never_grant_consent() {
    let store = CwStore::open_in_memory().unwrap();
    store
        .upsert_session(&Session {
            id: "mcp-session".into(),
            parent_session_id: None,
            harness: HarnessKind::GenericProxy,
            workspace_root: None,
            model: "offline".into(),
            provider: ProviderKind::OpenAI,
            title: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        })
        .unwrap();
    let mut state = DaemonState::new(store.clone());
    state.control_token = "offline-token".into();
    let (base, server) = serve(build_api_router(state)).await;
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("x-cw-control-token", "offline-token".parse().unwrap());
    let client = reqwest::Client::builder()
        .default_headers(headers)
        .no_proxy()
        .build()
        .unwrap();
    let args = json!({"session_id":"mcp-session"});
    let pin=call(&client,&base,"cw_pin_context",json!({"session_id":"mcp-session","label":"Invariant","content":"Do not change public API."})).await;
    assert_eq!(pin["isError"], false, "{pin}");
    assert!(
        pin["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Consent is unchanged")
    );
    let status = call(&client, &base, "cw_pin_policy_status", args.clone()).await;
    assert_eq!(status["isError"], false, "{status}");
    let body: Value = serde_json::from_str(status["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(body["policy"]["enabled"], false);
    assert_eq!(body["policy"]["reinforcement_supported"], true);
    assert_eq!(
        body["policy"]["stored_pins"][0]["content"],
        "Do not change public API."
    );
    assert_eq!(body["policy"]["stored_pins"][0]["status"], "committed");
    assert_eq!(
        body["policy"]["stored_pins"][0]["eligible_in_latest_request_preview_after_opt_in"],
        Value::Null
    );
    assert!(
        body["policy"]["risk_warning"]
            .as_str()
            .unwrap()
            .contains("Benefits are unproven")
    );
    assert!(body["policy"]["automatic_generation"].is_object());
    for name in [
        "cw_pin_policy_enable",
        "cw_pin_policy_disable",
        "cw_pin_policy_revoke",
    ] {
        assert_eq!(
            call(
                &client,
                &base,
                name,
                json!({"session_id":"mcp-session","acknowledge_risk":true,"enabled":true})
            )
            .await["isError"],
            true
        );
    }
    assert_eq!(
        call(
            &client,
            &base,
            "cw_pin_policy_status",
            json!({"session_id":"mcp-session","acknowledge_risk":true,"enabled":true})
        )
        .await["isError"],
        true
    );
    assert!(
        store
            .list_pin_context_policy_events("mcp-session")
            .unwrap()
            .is_empty()
    );
    for args in [
        json!({"session_id": 123}),
        json!([]),
        json!({"session_id":"bad/id"}),
    ] {
        assert_eq!(
            call(&client, &base, "cw_pin_policy_status", args).await["isError"],
            true
        );
    }
    // Existing non-pin mutation behavior is preserved under default-off consent.
    assert_eq!(
        call(&client, &base, "cw_shadow_dedupe", args.clone()).await["isError"],
        false
    );
    assert!(
        store
            .list_mutations("mcp-session")
            .unwrap()
            .iter()
            .any(|r| matches!(r.op, MutationOp::ShadowDedupe { .. }))
    );
    store
        .set_pin_context_policy("mcp-session", true, true)
        .unwrap();
    let enabled = call(&client, &base, "cw_pin_policy_status", args.clone()).await;
    assert!(
        enabled["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("\"enabled\": true")
    );
    // Unpin deletes rules but neither grants nor revokes the human's session consent.
    assert_eq!(
        call(&client, &base, "cw_unpin_context", args.clone()).await["isError"],
        false
    );
    assert!(
        store
            .get_pin_context_policy("mcp-session")
            .unwrap()
            .is_enabled_for("mcp-session")
    );
    assert_eq!(store.list_mutations("mcp-session").unwrap().len(), 1);
    let unauthorized = reqwest::Client::new();
    assert_eq!(
        call(&unauthorized, &base, "cw_pin_policy_status", args).await["isError"],
        true
    );
    server.abort();
    let _ = server.await;
}
#[tokio::test]
async fn malformed_unavailable_and_invalid_policy_queries_fail_closed() {
    for body in ["invalid JSON", "null", "{\"enabled\":true}"] {
        let (base, server) = serve(Router::new().route(
            "/api/v1/sessions/mcp-session/pin-context-policy",
            axum::routing::get(move || async move { body }),
        ))
        .await;
        let client = reqwest::Client::new();
        assert_eq!(
            call(
                &client,
                &base,
                "cw_pin_policy_status",
                json!({"session_id":"mcp-session"})
            )
            .await["isError"],
            true
        );
        assert_eq!(
            call(
                &client,
                &base,
                "cw_pin_policy_status",
                json!({"session_id":"bad/id"})
            )
            .await["isError"],
            true
        );
        server.abort();
        let _ = server.await;
        assert_eq!(
            call(
                &client,
                &base,
                "cw_pin_policy_status",
                json!({"session_id":"mcp-session"})
            )
            .await["isError"],
            true
        );
    }
}
#[test]
fn tool_registration_is_inspection_only_and_truthful() {
    let tools = mcp_tool_definitions();
    let tools = tools.as_array().unwrap();
    assert!(tools.iter().any(|t| t["name"] == "cw_pin_policy_status"));
    assert!(
        !tools
            .iter()
            .any(|t| t["name"].as_str().unwrap().contains("policy_enable"))
    );
    let pin = tools
        .iter()
        .find(|t| t["name"] == "cw_pin_context")
        .unwrap();
    assert!(
        pin["description"]
            .as_str()
            .unwrap()
            .contains("without granting consent")
    );
}
