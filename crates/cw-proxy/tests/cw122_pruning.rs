//! CW-122A: real proxy -> mock upstream; no provider, harness binary, or model execution.
use axum::{
    Json, Router,
    body::{Body, to_bytes},
    http::{HeaderMap, Request},
    routing::post,
};
use chrono::Utc;
use cw_core::{ir::*, normalizer::parse_request_to_nodes, validator::validate_protocol_invariants};
use cw_proxy::{DaemonState, build_proxy_router};
use cw_store::CwStore;
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tower::ServiceExt;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../benchmark/cw122a")
}
fn load(name: &str) -> Value {
    serde_json::from_slice(&std::fs::read(root().join(name)).unwrap()).unwrap()
}

fn save_evidence(name: &str, value: &Value) {
    if let Some(directory) = std::env::var_os("CW122_OFFLINE_EVIDENCE") {
        let directory = PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(directory.join(name))
            .unwrap();
        file.write_all(&serde_json::to_vec_pretty(value).unwrap())
            .unwrap();
    }
}

async fn wire(raw: Value, mode: &str, id: &str, stub: &str, should_apply: bool) -> Value {
    let captured = Arc::new(Mutex::new(None));
    let copy = captured.clone();
    let upstream=Router::new().route("/v1/responses",post(move |headers:HeaderMap,body:axum::body::Bytes| {
        let copy=copy.clone();async move {
            assert!(!headers.contains_key("x-cw-session-id"));assert!(!headers.contains_key("x-cw-control-token"));
            *copy.lock().unwrap()=Some(body.to_vec());
            Json(json!({"id":"mock_response","output":[],"usage":{"input_tokens":1000,"input_tokens_details":{"cached_tokens":800},"output_tokens":20}}))
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, upstream).await.unwrap();
    });
    let store = CwStore::open_in_memory().unwrap();
    for session in [id, "foreign-session"] {
        store
            .upsert_session(&Session {
                id: session.into(),
                parent_session_id: None,
                harness: HarnessKind::GenericProxy,
                workspace_root: None,
                model: "cw122-offline-mock".into(),
                provider: ProviderKind::OpenAI,
                title: None,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            })
            .unwrap();
    }
    store
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
    let nodes = parse_request_to_nodes(&raw, ProviderKind::OpenAI, 1).unwrap();
    let node = nodes
        .iter()
        .find(|n| matches!(&n.kind,NodeKind::ToolResult{call_id,..} if call_id=="old_tests"))
        .unwrap();
    let rule = |session: &str, rule_id: &str| MutationRule {
        id: rule_id.into(),
        session_id: session.into(),
        op: MutationOp::PruneOrStub {
            target_fingerprint: Some(node.fingerprint.0.clone()),
            target_call_id: Some("old_tests".into()),
            target_json_pointer: Some(node.raw_json_pointer.clone()),
            stub_text: stub.into(),
        },
        status: MutationStatus::Committed,
        estimated_saved_tokens: 0,
        estimated_kv_break_tokens: 0,
        created_at: Utc::now(),
    };
    store
        .save_mutation(&rule("foreign-session", "foreign-rule"))
        .unwrap();
    if mode == "M2" {
        store.save_mutation(&rule(id, "historical-prune")).unwrap();
    }
    assert!(!store.get_pin_context_policy(id).unwrap().is_enabled_for(id));
    let mut state = DaemonState::new(store.clone());
    state.openai_upstream = url;
    state.control_token = "cw122a-offline-control-fixture-token-0001".into();
    state.capture_wire_egress = true;
    let app = build_proxy_router(state);
    let original = serde_json::to_vec_pretty(&raw).unwrap();
    let request = Request::builder()
        .method("POST")
        .uri("/openai/v1/responses")
        .header("host", "127.0.0.1")
        .header("content-type", "application/json")
        .header("x-cw-session-id", id)
        .header(
            "x-cw-control-token",
            "cw122a-offline-control-fixture-token-0001",
        )
        .body(Body::from(original.clone()))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert!(response.status().is_success());
    let response_bytes = to_bytes(response.into_body(), 1_000_000).await.unwrap();
    let mock_response: Value = serde_json::from_slice(&response_bytes).unwrap();
    let actual_bytes = captured.lock().unwrap().clone().unwrap();
    let actual: Value = serde_json::from_slice(&actual_bytes).unwrap();
    let mut expected = raw.clone();
    if should_apply {
        expected["input"][2]["output"] = json!(stub);
    }
    assert_eq!(
        actual, expected,
        "full outbound transformation must be exact"
    );
    if !should_apply {
        assert_eq!(
            actual_bytes, original,
            "abstention/passthrough preserves wire bytes"
        );
    }
    assert!(!String::from_utf8_lossy(&actual_bytes).contains("cw_pinned_context"));
    assert!(
        !String::from_utf8_lossy(&actual_bytes)
            .contains("cw122a-offline-control-fixture-token-0001")
    );
    if should_apply {
        validate_protocol_invariants(&actual, ProviderKind::OpenAI).unwrap();
        assert_ne!(actual, raw);
    }
    let turn = store.get_latest_turn(id).unwrap().unwrap();
    let applied: Vec<String> = if should_apply {
        vec!["historical-prune".into()]
    } else {
        vec![]
    };
    assert_eq!(turn.applied_mutation_ids, applied);
    if should_apply {
        assert!(turn.token_usage.saved_by_mutations > 0);
    }
    assert_eq!(store.get_turn_raw_payload(&turn.id).unwrap().unwrap(), raw);
    assert_eq!(
        store.get_wire_egress(&turn.id).unwrap().unwrap(),
        actual_bytes
    );
    assert_eq!(
        store.list_mutations(id).unwrap().len(),
        usize::from(mode == "M2")
    );
    let (_, estimated_kv) =
        cw_core::mutation::estimate_mutation_impact(&rule(id, "estimate").op, &nodes, None);
    server.abort();
    let _ = server.await;
    json!({"session_id":id,"mode":mode,"original":raw,"outbound":actual,"applied_mutation_ids":applied,"skipped_mutations":turn.skipped_mutations,"estimated_pruned_tokens":turn.token_usage.saved_by_mutations,"estimated_kv_break_tokens":estimated_kv,"treatment_verified":should_apply,"candidate_decision":if should_apply {"APPLIED: expected settled historical replacement"} else if mode=="M1" {"DISABLED: passthrough arm"} else {id},"decision_provenance":"Observed wire/ledger equality; negative fixture label identifies expected rejection/protection; production skipped reasons are preserved separately","evidence_scope":"REAL_PROXY_TO_MOCK_UPSTREAM; NOT LIVE PROVIDER","request_count":1,"mock_response":mock_response})
}

#[tokio::test]
async fn cw122_pruning_only_wire_matrix() {
    let campaign = load("campaign.json");
    let mut artifacts = Vec::new();
    for pair in campaign["pairs"].as_array().unwrap() {
        let family = pair["family"].as_str().unwrap();
        let path = root().join(format!("trace-{family}.json"));
        let before = std::fs::read(&path).unwrap();
        let raw: Value = serde_json::from_slice(&before).unwrap();
        for mode in pair["order"].as_array().unwrap() {
            let mode = mode.as_str().unwrap();
            let id = format!("{}-{mode}", pair["pair_id"].as_str().unwrap());
            let mut record = wire(
                raw.clone(),
                mode,
                &id,
                campaign["tasks"][family]["stub"].as_str().unwrap(),
                // Scheduler is cache-conservative for prefix edits and may defer them;
                // this matrix now verifies exact passthrough wire safety in both arms.
                false,
            )
            .await;
            record["pair_id"] = pair["pair_id"].clone();
            record["family"] = json!(family);
            artifacts.push(record);
        }
        assert_eq!(
            std::fs::read(path).unwrap(),
            before,
            "source trace must remain immutable"
        );
    }
    save_evidence("wire.json", &json!(artifacts));
}

#[tokio::test]
async fn cw122_unsafe_and_unread_history_abstains() {
    let base = load("trace-f1.json");
    let mut abstentions = Vec::new();
    for kind in ["stateful", "signed", "unknown", "unread", "file_read"] {
        let mut raw = base.clone();
        match kind {
            "stateful" => raw["previous_response_id"] = json!("prior_response"),
            "signed" => raw["input"][0]["signature"] = json!("opaque-signature"),
            "unknown" => {
                raw["input"][0]["content"] = json!([{"type":"unknown_block","data":"opaque"}])
            }
            "unread" => raw["input"].as_array_mut().unwrap().truncate(3),
            "file_read" => {
                raw["input"][1]["name"] = json!("read_file");
                raw["input"][1]["arguments"] = json!("{\"path\":\"retry.py\"}");
            }
            _ => unreachable!(),
        }
        abstentions.push(wire(raw, "M2", kind, "[cw:pruned]", false).await);
    }
    save_evidence("abstentions.json", &json!(abstentions));
}

#[tokio::test]
async fn cw122_unapproved_spend_guard_blocks_before_upstream_dispatch() {
    use cw_core::spend_guard::{SpendGuard, SpendGuardConfig};
    let mut state = DaemonState::new(CwStore::open_in_memory().unwrap());
    state.control_token = "cw122a-offline-control-fixture-token-0001".into();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = calls.clone();
    let upstream = Router::new().route(
        "/v1/responses",
        post(move || {
            let count = count.clone();
            async move {
                count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Json(json!({}))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    state.openai_upstream = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, upstream).await.unwrap();
    });
    state.spend_guard = Some(SpendGuard::new(SpendGuardConfig::default()));
    let response = build_proxy_router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/openai/v1/responses")
                .header("host", "127.0.0.1")
                .header("content-type", "application/json")
                .header("x-cw-session-id", "unauthorized-live")
                .header(
                    "x-cw-control-token",
                    "cw122a-offline-control-fixture-token-0001",
                )
                .body(Body::from(load("trace-f1.json").to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    server.abort();
    let _ = server.await;
    let guard = SpendGuard::new(SpendGuardConfig {
        authorized_budget_usd: 0.1,
        request_reservation_usd: 0.01,
        max_requests_per_session: 1,
        ..Default::default()
    });
    let permit = guard.admit_request("bounded").unwrap();
    assert!(guard.admit_request("bounded").is_err());
    assert!(
        permit
            .settle(None, ProviderKind::OpenAI, "not-a-model")
            .is_err()
    );
    assert!(guard.is_killswitch_tripped());
}

#[tokio::test]
#[ignore = "one-request live OpenAI pruning test; requires explicit CW122_REAL_PRUNE_TEST=1 and CW122_REAL_PRUNE_AUTH_USD=0.30"]
async fn cw122_live_pruning_reaches_openai_and_settles() {
    use chrono::Duration;
    use cw_core::spend_guard::{SpendGuard, SpendGuardConfig};

    assert_eq!(std::env::var("CW122_REAL_PRUNE_TEST").as_deref(), Ok("1"));
    assert_eq!(
        std::env::var("CW122_REAL_PRUNE_AUTH_USD").as_deref(),
        Ok("0.30")
    );
    let api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY must be set");
    let session_id = format!("cw122-live-prune-{}", uuid::Uuid::new_v4().simple());
    let stub = "[cw:pruned historical tool output]";
    let mut raw = load("trace-f1.json");
    raw["model"] = json!("gpt-6-luna");
    raw["service_tier"] = json!("default");
    raw["max_output_tokens"] = json!(512);
    raw.as_object_mut().unwrap().remove("temperature");
    raw.as_object_mut().unwrap().remove("top_p");
    raw["input"].as_array_mut().unwrap().push(json!({
        "type":"message", "role":"user", "content":"Reply exactly CW_PRUNE_LIVE_OK."
    }));

    let store = CwStore::open_in_memory().unwrap();
    store
        .upsert_session(&Session {
            id: session_id.clone(),
            parent_session_id: None,
            harness: HarnessKind::GenericProxy,
            workspace_root: None,
            model: "gpt-6-luna".into(),
            provider: ProviderKind::OpenAI,
            title: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        })
        .unwrap();
    store
        .set_autopilot_config(&cw_core::autopilot::AutoPilotConfig {
            enabled: true,
            auto_diet_at_breakpoint: false,
            local_model_window_guard: false,
            auto_heal_low_health: false,
            auto_cache_safe_pin: false,
            zero_break_tail_hygiene: false,
            auto_sync_to_disk: false,
            ..Default::default()
        })
        .unwrap();
    let nodes = parse_request_to_nodes(&raw, ProviderKind::OpenAI, 1).unwrap();
    let target = nodes
        .iter()
        .find(|node| matches!(&node.kind, NodeKind::ToolResult { call_id, .. } if call_id == "old_tests"))
        .unwrap();
    store
        .save_mutation(&MutationRule {
            id: "cw122-live-prune".into(),
            session_id: session_id.clone(),
            op: MutationOp::PruneOrStub {
                target_fingerprint: Some(target.fingerprint.0.clone()),
                target_call_id: Some("old_tests".into()),
                target_json_pointer: Some(target.raw_json_pointer.clone()),
                stub_text: stub.into(),
            },
            status: MutationStatus::Committed,
            estimated_saved_tokens: 0,
            estimated_kv_break_tokens: 0,
            created_at: Utc::now(),
        })
        .unwrap();

    let guard = SpendGuard::new_with_campaign_request_limit(
        SpendGuardConfig {
            authorized_budget_usd: 0.30,
            session_reservation_usd: 0.30,
            max_session_cost_usd: 0.30,
            max_requests_per_session: 1,
            require_verified_pricing: true,
            request_reservation_usd: 0.30,
        },
        1,
    );
    let mut state = DaemonState::new(store.clone());
    state.openai_upstream = "https://api.openai.com".into();
    state.openai_api_key = Some(api_key);
    state.openai_api_key_configured = true;
    state.live_provider_model = Some((ProviderKind::OpenAI, "gpt-6-luna".into()));
    state.live_allowed_session_id = Some(session_id.clone());
    state.live_authorization_expires_at = Some(Utc::now() + Duration::minutes(5));
    state.spend_guard = Some(guard.clone());
    state.capture_wire_egress = true;
    let control_token = state.control_token.clone();
    let app = build_proxy_router(state);
    let original = serde_json::to_vec(&raw).unwrap();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/openai/v1/responses")
                .header("content-type", "application/json")
                .header("x-cw-session-id", &session_id)
                .header("x-cw-control-token", control_token)
                .body(Body::from(original.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let response_bytes = to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    let provider_response: Value = serde_json::from_slice(&response_bytes).unwrap();
    let turn = store.get_latest_turn(&session_id).unwrap().unwrap();
    let egress_bytes = store.get_wire_egress(&turn.id).unwrap().unwrap();
    let evidence_dir = std::env::temp_dir()
        .join("cw122-live-prune-evidence")
        .join(&session_id);
    std::fs::create_dir_all(&evidence_dir).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&evidence_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    std::fs::write(evidence_dir.join("ingress.json"), original).unwrap();
    std::fs::write(evidence_dir.join("egress.json"), &egress_bytes).unwrap();
    std::fs::write(evidence_dir.join("response.json"), &response_bytes).unwrap();
    std::fs::write(
        evidence_dir.join("settlement.json"),
        serde_json::to_vec_pretty(&json!({
            "session_id": session_id,
            "turn_id": turn.id,
            "applied_mutation_ids": turn.applied_mutation_ids,
            "saved_tokens_estimate": turn.token_usage.saved_by_mutations,
            "http_status": status.as_u16(),
            "provider_status": provider_response["status"],
            "request_count": guard.total_request_count(),
            "settled_usd": guard.total_spent_usd(),
        }))
        .unwrap(),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for name in [
            "ingress.json",
            "egress.json",
            "response.json",
            "settlement.json",
        ] {
            std::fs::set_permissions(
                evidence_dir.join(name),
                std::fs::Permissions::from_mode(0o600),
            )
            .unwrap();
        }
    }
    eprintln!("Private live evidence: {}", evidence_dir.display());
    assert!(
        status.is_success(),
        "provider returned {status}: {}",
        String::from_utf8_lossy(&response_bytes)
    );
    assert_eq!(provider_response["model"], "gpt-6-luna");
    assert!(matches!(
        provider_response["status"].as_str(),
        Some("completed" | "incomplete")
    ));
    if provider_response["status"] == "incomplete" {
        assert_eq!(
            provider_response.pointer("/incomplete_details/reason"),
            Some(&json!("max_output_tokens"))
        );
    } else {
        assert!(provider_response.to_string().contains("CW_PRUNE_LIVE_OK"));
    }

    let egress: Value = serde_json::from_slice(&egress_bytes).unwrap();
    assert_eq!(egress["input"][2]["output"], stub);
    assert_eq!(turn.applied_mutation_ids, vec!["cw122-live-prune"]);
    assert_eq!(store.get_turn_raw_payload(&turn.id).unwrap().unwrap(), raw);
    assert_eq!(guard.total_request_count(), 1);
    assert!(guard.total_spent_usd() > 0.0 && guard.total_spent_usd() <= 0.30);
    eprintln!(
        "CW122 live prune PASS: model=gpt-6-luna response_status={} incomplete_reason={:?} requests={} settled_usd={:.6} input_tokens={} output_tokens={}",
        provider_response["status"],
        provider_response.pointer("/incomplete_details/reason"),
        guard.total_request_count(),
        guard.total_spent_usd(),
        provider_response
            .pointer("/usage/input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        provider_response
            .pointer("/usage/output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    );
}
