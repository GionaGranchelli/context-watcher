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
    set_policy(
        &app,
        &token,
        json!({"enabled":true, "acknowledge_risk":true}),
    )
    .await;
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
    set_policy(
        &app,
        &token,
        json!({"enabled":true, "acknowledge_risk":true}),
    )
    .await;
    let first = transform(&app, &token, &history()).await;
    let mut hypothetical_history = first["messages"].as_array().unwrap().clone();
    append_tool_turn(&mut hypothetical_history, "read-1");
    let followup = transform(&app, &token, &hypothetical_history).await;
    assert_eq!(followup["messages"], json!(hypothetical_history));
    assert_eq!(followup["applied_rules"], json!([]));
    assert_eq!(followup["messages"][2]["content"][1]["text"], BLOCK);
    // This counterfactual input is not a persistence implementation or transcript write.
}

async fn policy_request(app: &Router, token: &str, body: Value) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/sessions/{SESSION}/pin-context-policy"))
                .header("host", "127.0.0.1")
                .header("x-cw-control-token", token)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1_000_000).await.unwrap();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

async fn set_policy(app: &Router, token: &str, body: Value) -> Value {
    let (status, value) = policy_request(app, token, body).await;
    assert_eq!(status, StatusCode::OK);
    value
}

#[tokio::test]
async fn legacy_committed_pin_requires_opt_in_and_revocation_preserves_rules_and_source() {
    let (app, token, store) = fixture();
    let original = history();
    let source_bytes = serde_json::to_vec(&original).unwrap();
    for rule_id in ["ap_pin_legacy", "strategy-pin-legacy"] {
        let mut rule = store.list_mutations(SESSION).unwrap()[0].clone();
        rule.id = rule_id.into();
        store.save_mutation(&rule).unwrap();
    }
    let all_rules = serde_json::to_value(store.list_mutations(SESSION).unwrap()).unwrap();
    let disabled = transform(&app, &token, &original).await;
    assert_eq!(disabled["messages"], json!(original));
    assert_eq!(disabled["applied_rules"], json!([]));
    assert_eq!(disabled["skipped_rules"].as_array().unwrap().len(), 3);
    assert!(
        disabled["skipped_rules"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["reason"] == "PinContextOptInRequired")
    );
    assert!(
        store
            .list_pin_context_policy_events(SESSION)
            .unwrap()
            .is_empty()
    );
    for body in [
        json!({"enabled":true}),
        json!({"enabled":true,"acknowledge_risk":false}),
    ] {
        assert_eq!(
            policy_request(&app, &token, body).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        policy_request(
            &app,
            "incorrect-token",
            json!({"enabled":true,"acknowledge_risk":true})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert!(
        store
            .list_pin_context_policy_events(SESSION)
            .unwrap()
            .is_empty()
    );
    let granted = set_policy(
        &app,
        &token,
        json!({"enabled":true,"acknowledge_risk":true}),
    )
    .await;
    assert_eq!(granted["enabled"], true);
    assert!(
        granted["risk_warning"]
            .as_str()
            .unwrap()
            .contains("cloud cost")
    );
    let enabled = transform(&app, &token, &original).await;
    assert_ne!(enabled["messages"], json!(original));
    assert_eq!(enabled["applied_rules"].as_array().unwrap().len(), 3);
    let revoked = set_policy(&app, &token, json!({"enabled":false})).await;
    assert_eq!(revoked["enabled"], false);
    assert_eq!(revoked["events"].as_array().unwrap().len(), 2);
    for _ in 0..2 {
        let result = transform(&app, &token, &original).await;
        assert_eq!(result["messages"], json!(original));
        assert_eq!(result["applied_rules"], json!([]));
    }
    assert_eq!(
        serde_json::to_value(store.list_mutations(SESSION).unwrap()).unwrap(),
        all_rules
    );
    assert_eq!(serde_json::to_vec(&original).unwrap(), source_bytes);
}

#[tokio::test]
async fn proxy_final_boundary_and_ledger_follow_current_policy_without_provider_dispatch() {
    let (api, token, store) = fixture();
    store
        .set_autopilot_config(&cw_core::autopilot::AutoPilotConfig {
            enabled: true,
            auto_diet_at_breakpoint: false,
            local_model_window_guard: false,
            auto_heal_low_health: false,
            ..Default::default()
        })
        .unwrap();
    let mut state = DaemonState::new(store.clone());
    state.control_token = token.clone();
    // Dry-run responds before any upstream connection, including on assertion failure.
    state.openai_upstream = "http://127.0.0.1:1".into();
    let proxy = cw_proxy::build_proxy_router(state);
    let original =
        json!({"model":"offline-model","messages":[{"role":"user","content":"Continue."}]});
    for (step, enabled) in [false, true, false].into_iter().enumerate() {
        if step > 0 {
            set_policy(
                &api,
                &token,
                json!({"enabled":enabled,"acknowledge_risk":enabled}),
            )
            .await;
        }
        let response = proxy
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/openai/v1/chat/completions")
                    .header("host", "127.0.0.1")
                    .header("x-cw-session-id", SESSION)
                    .header("x-cw-control-token", &token)
                    .header("x-cw-dry-run", "true")
                    .header("content-type", "application/json")
                    .body(Body::from(original.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1_000_000).await.unwrap())
                .unwrap();
        assert_eq!(body["cw_dry_run"], true);
        let turn = store.get_latest_turn(SESSION).unwrap().unwrap();
        let raw = store.get_turn_raw_payload(&turn.id).unwrap().unwrap();
        assert_eq!(raw, original);
        if enabled {
            assert_ne!(body["mutated_payload"], original);
            assert_eq!(turn.applied_mutation_ids, [RULE]);
        } else {
            assert_eq!(body["mutated_payload"], original);
            assert!(turn.applied_mutation_ids.is_empty());
            assert_eq!(turn.skipped_mutations[0].rule_id, RULE);
            assert_eq!(turn.skipped_mutations[0].reason, "PinContextOptInRequired");
        }
    }
}

#[tokio::test]
async fn strategy_draft_and_inactive_commit_report_creation_separately_from_application() {
    for status in ["draft", "committed"] {
        let (app, token, store) = fixture();
        let raw = json!({"system":"<RULE[AGENTS.md]>Preserve public API.</RULE[AGENTS.md]>","messages":[{"role":"user","content":"Implement the task."}]});
        let (mut turn, _) = cw_core::analyze_turn_from_payload(
            "strategy-fixture".into(),
            SESSION.into(),
            None,
            1,
            &raw,
            ProviderKind::Anthropic,
            None,
            None,
            None,
        )
        .unwrap();
        // Exercise the long-context strategy threshold without a model or large fixture.
        turn.token_usage.input_tokens = 30_001;
        for node in &mut turn.nodes {
            node.attention_salience = 0.1;
        }
        store.save_turn(&turn, Some(&raw), None, None).unwrap();
        let plans = store.optimization_plans(&turn).unwrap();
        let plan = plans
            .iter()
            .find(|plan| plan.id == "cache_safe_boost")
            .unwrap();
        assert_eq!(plan.items.len(), 1);
        assert!(matches!(
            plan.items[0].mutation_op,
            MutationOp::PinContext { .. }
        ));
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/sessions/{SESSION}/optimize"))
                    .header("host", "127.0.0.1")
                    .header("x-cw-control-token", &token)
                    .header("if-match", store.session_revision(SESSION).unwrap())
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"strategy_id":"cache_safe_boost","status":status}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let result: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1_000_000).await.unwrap())
                .unwrap();
        assert_eq!(result["created_count"], 1);
        assert_eq!(result["applied_count"], 0);
        assert_eq!(result["applied_mutations"], json!([]));
        assert_eq!(result["created_mutations"][0]["status"], status);
        assert_eq!(store.list_mutations(SESSION).unwrap().len(), 2);
        assert_eq!(
            result["skipped_rules"].as_array().unwrap().len(),
            if status == "draft" { 1 } else { 2 }
        );
        assert!(
            result["skipped_rules"]
                .as_array()
                .unwrap()
                .iter()
                .all(|r| r["reason"] == "PinContextOptInRequired")
        );
        assert!(
            store
                .list_pin_context_policy_events(SESSION)
                .unwrap()
                .is_empty()
        );
        assert_eq!(store.get_turn_raw_payload(&turn.id).unwrap().unwrap(), raw);
    }
}
