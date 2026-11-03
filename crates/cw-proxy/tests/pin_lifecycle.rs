//! In-process API checks only: no sockets, provider, harness binary, or credentials.
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use chrono::Utc;
use cw_core::ir::{HarnessKind, MutationOp, MutationRule, MutationStatus, ProviderKind, Session};
use cw_proxy::{DaemonState, build_api_router};
use cw_store::CwStore;
use serde_json::{Value, json};
use tower::ServiceExt;

const SESSION: &str = "omp_offline_lifecycle";
const RULE: &str = "pin-lifecycle";
const BLOCK: &str =
    "\n<cw_pinned_context>\n- [constraint]: Preserve the public API.\n</cw_pinned_context>";

fn fixture() -> (Router, String, CwStore) {
    let store = CwStore::open_in_memory().unwrap();
    store
        .upsert_session(&Session {
            id: SESSION.into(),
            parent_session_id: None,
            harness: HarnessKind::Omp,
            workspace_root: None,
            model: "offline-no-provider".into(),
            provider: ProviderKind::OpenAI,
            title: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        })
        .unwrap();
    store
        .save_mutations_at_revision(
            SESSION,
            None,
            &[MutationRule {
                id: RULE.into(),
                session_id: SESSION.into(),
                op: MutationOp::PinContext {
                    label: "constraint".into(),
                    content: "Preserve the public API.".into(),
                    source_node_id: None,
                },
                status: MutationStatus::Committed,
                estimated_saved_tokens: 0,
                estimated_kv_break_tokens: 0,
                created_at: Utc::now(),
            }],
        )
        .unwrap();
    let state = DaemonState::new(store.clone());
    let token = state.control_token.clone();
    (build_api_router(state), token, store)
}

async fn transform(app: &Router, token: &str, original: &[Value]) -> Value {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/sessions/{SESSION}/transform"))
                .header("host", "127.0.0.1")
                .header("x-cw-control-token", token)
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({"messages": original, "provider": "openai"}))
                        .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 1_000_000).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["status"], "ok");
    body
}

fn history() -> Vec<Value> {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../benchmark/cw104b/fixtures/offline-lifecycle.json"
    ))
    .unwrap();
    assert_eq!(fixture["kind"], "sanitized-offline-lifecycle");
    fixture["messages"].as_array().unwrap().clone()
}

fn append_tool_turn(history: &mut Vec<Value>, id: &str) {
    history.push(json!({"role":"assistant", "content":null, "tool_calls":[{
        "id":id, "type":"function", "function":{"name":"read", "arguments":"{}"}
    }]}));
    history.push(json!({"role":"tool", "tool_call_id":id, "content":"unchanged tool bytes"}));
}

#[tokio::test]
async fn pin_lifecycle_uses_fresh_history_and_reappears_on_a_new_user_turn() {
    let (app, token, store) = fixture();
    let mut original = history();
    let shared_prefix = serde_json::to_vec(&original[..2]).unwrap();
    let source_bytes = serde_json::to_vec(&original).unwrap();
    let first = transform(&app, &token, &original).await;
    let mut expected = original.clone();
    expected[2]["content"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"text", "text":BLOCK}));
    assert_eq!(first["messages"], json!(expected));
    assert_eq!(first["applied_rules"], json!([RULE]));
    assert_eq!(serde_json::to_vec(&original).unwrap(), source_bytes);
    let retry = transform(&app, &token, first["messages"].as_array().unwrap()).await;
    assert_eq!(retry["messages"], first["messages"]);
    assert_eq!(retry["applied_rules"], json!([RULE]));

    // OMP continues from its original history, not the returned request overlay.
    for index in 1..=8 {
        append_tool_turn(&mut original, &format!("read-{index}"));
        let before = serde_json::to_vec(&original).unwrap();
        let followup = transform(&app, &token, &original).await;
        assert_eq!(followup["messages"], json!(original));
        assert_eq!(followup["applied_rules"], json!([]));
        assert!(
            !serde_json::to_string(&followup["messages"])
                .unwrap()
                .contains("[constraint]")
        );
        assert_eq!(serde_json::to_vec(&original).unwrap(), before);
        assert_eq!(serde_json::to_vec(&original[..2]).unwrap(), shared_prefix);
    }

    original.push(json!({"role":"assistant", "content":"The reads are complete."}));
    original.push(json!({"role":"user", "content":"Continue."}));
    let next_user = transform(&app, &token, &original).await;
    let mut expected = original.clone();
    expected.last_mut().unwrap()["content"] = json!(format!("Continue.{BLOCK}"));
    assert_eq!(next_user["messages"], json!(expected));
    assert_eq!(next_user["applied_rules"], json!([RULE]));
    assert_eq!(serde_json::to_vec(&original[..2]).unwrap(), shared_prefix);
    assert!(store.get_latest_turn(SESSION).unwrap().is_none());
    let rules = store.list_mutations(SESSION).unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].status, MutationStatus::Committed);
}

#[tokio::test]
async fn pin_lifecycle_preserves_an_existing_user_pin_without_claiming_a_new_application() {
    let (app, token, _) = fixture();
    let first = transform(&app, &token, &history()).await;
    let mut hypothetical_history = first["messages"].as_array().unwrap().clone();
    append_tool_turn(&mut hypothetical_history, "read-1");
    let followup = transform(&app, &token, &hypothetical_history).await;
    assert_eq!(followup["messages"], json!(hypothetical_history));
    assert_eq!(followup["applied_rules"], json!([]));
    assert_eq!(followup["messages"][2]["content"][1]["text"], BLOCK);
    // This counterfactual input is not a persistence implementation or transcript write.
}
