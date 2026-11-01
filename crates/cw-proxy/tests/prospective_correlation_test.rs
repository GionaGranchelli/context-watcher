use axum::{
    Json, Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    routing::post,
};
use chrono::Utc;
use cw_core::calibration::*;
use cw_core::ir::*;
use cw_core::policy::{CacheState, DecisionReasonCode, OptimizationDecision, TelemetryProvenance};
use cw_proxy::{DaemonState, build_proxy_router};
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

fn anthropic_request(body: &str, session_id: Option<&str>, token: &str) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/anthropic/v1/messages")
        .header("host", "127.0.0.1")
        .header("content-type", "application/json");
    if let Some(id) = session_id {
        builder = builder
            .header("x-cw-session-id", id)
            .header("x-cw-control-token", token);
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

#[allow(dead_code)]
fn openai_request(body: &str, session_id: Option<&str>, token: &str) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/openai/v1/chat/completions")
        .header("host", "127.0.0.1")
        .header("content-type", "application/json");
    if let Some(id) = session_id {
        builder = builder
            .header("x-cw-session-id", id)
            .header("x-cw-control-token", token);
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

// -------------------------------------------------------------------------------------------------
// Scenario 1: Pre-dispatch decision generation with multiple candidates & plan on same turn
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn test_prospective_correlation_sc1_multiple_decisions_single_turn() {
    let (url, _task) = mock(Router::new().route(
        "/v1/messages",
        post(|| async {
            Json(json!({
                "id": "msg_01",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "OK"}],
                "usage": {
                    "input_tokens": 5000,
                    "cache_read_input_tokens": 4000,
                    "cache_creation_input_tokens": 0,
                    "output_tokens": 50
                }
            }))
        }),
    ))
    .await;

    let store = CwStore::open_in_memory().unwrap();
    let session_id = "sess_sc1_multidecision";
    store
        .upsert_session(&Session {
            id: session_id.to_string(),
            parent_session_id: None,
            harness: HarnessKind::GenericProxy,
            workspace_root: None,
            model: "claude-3-5-sonnet-20241022".to_string(),
            provider: ProviderKind::Anthropic,
            title: Some("Scenario 1 Multi-Decision".to_string()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        })
        .unwrap();

    let mut state = DaemonState::new(store.clone());
    state.anthropic_upstream = url;
    let token = state.control_token.clone();
    let app = build_proxy_router(state);

    let raw = json!({
        "model": "claude-3-5-sonnet-20241022",
        "max_tokens": 100,
        "messages": [
            {"role": "user", "content": "Hello ContextWatcher multi-decision"}
        ]
    })
    .to_string();

    let response = app
        .oneshot(anthropic_request(&raw, Some(session_id), &token))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    to_bytes(response.into_body(), 10000).await.unwrap();

    // Give background recording task time to finalize
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Verify turn saved
    let turns = store.list_turns(session_id).unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].token_usage.cached_tokens, 4000);
}

// -------------------------------------------------------------------------------------------------
// Scenario 2: Standard successful response with valid usage (immediate correlation)
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn test_prospective_correlation_sc2_successful_response_immediate_match() {
    let store = CwStore::open_in_memory().unwrap();
    let session_id = "sess_sc2_match";
    let model = "claude-3-5-sonnet-20241022";

    // Create a pre-dispatch shadow decision evidence record
    let decision = ShadowDecisionEvidence {
        record_id: format!("sde_{}_1_cand_prune", session_id),
        session_id: session_id.to_string(),
        request_index: 1,
        turn_index: 1,
        provider: ProviderKind::Anthropic,
        model: model.to_string(),
        cache_state: CacheState::WarmVerified,
        provenance: TelemetryProvenance::ObservedProvider,
        prefix_fingerprint: Some("pfx_sc2".to_string()),
        payload_fingerprint: Some("pfp_sc2".to_string()),
        candidate_id: Some("cand_prune_01".to_string()),
        plan_id: None,
        policy_evaluation_status: PolicyEvaluationStatus::EvaluatedPreDispatch,
        decision: Some(OptimizationDecision::Apply),
        reason_code: Some(DecisionReasonCode::ModeledEconomicBenefitPositive),
        explanation: "Tail prune preserves warm prefix".to_string(),
        estimated_saved_tokens: Some(400),
        estimated_kv_break_tokens: Some(0),
        predicted_baseline_cost_usd: Some(0.005),
        predicted_optimized_cost_usd: Some(0.0045),
        predicted_net_benefit_usd: Some(0.0005),
        forecast_horizon_turns: 1,
        forecast_assumptions: "Single turn matching".to_string(),
        pricing_model_pattern: Some("claude-3-5-sonnet*".to_string()),
        pricing_version: "2026-10-v1".to_string(),
        policy_version: "0.2.4-cw117c".to_string(),
        decision_timestamp: Utc::now(),
    };
    store.save_shadow_decision_evidence(&decision).unwrap();

    let (url, _task) = mock(Router::new().route(
        "/v1/messages",
        post(|| async {
            Json(json!({
                "id": "msg_sc2",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "Response"}],
                "usage": {
                    "input_tokens": 5000,
                    "cache_read_input_tokens": 4500,
                    "output_tokens": 100
                }
            }))
        }),
    ))
    .await;

    store
        .upsert_session(&Session {
            id: session_id.to_string(),
            parent_session_id: None,
            harness: HarnessKind::GenericProxy,
            workspace_root: None,
            model: model.to_string(),
            provider: ProviderKind::Anthropic,
            title: Some("Scenario 2".to_string()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        })
        .unwrap();

    let mut state = DaemonState::new(store.clone());
    state.anthropic_upstream = url;
    let token = state.control_token.clone();
    let app = build_proxy_router(state);

    let raw = json!({
        "model": model,
        "max_tokens": 100,
        "messages": [{"role": "user", "content": "Hello"}]
    })
    .to_string();

    let res = app
        .oneshot(anthropic_request(&raw, Some(session_id), &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    to_bytes(res.into_body(), 10000).await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Check correlated decision outcome in SQLite
    let corrs = store
        .list_correlated_decision_outcomes(Some(session_id))
        .unwrap();
    assert_eq!(corrs.len(), 1);
    assert_eq!(
        corrs[0].correlation_status,
        CorrelationStatus::MatchedImmediate
    );
    assert_eq!(corrs[0].cache_state_consistent, Some(true));
    assert!(corrs[0].single_turn_cost_delta_usd.is_some());

    // Verify multi-turn forecast (H=3) omits single-turn cost delta to prevent apples-to-oranges comparison
    let mut dec_multi = decision.clone();
    dec_multi.forecast_horizon_turns = 3;
    let corr_multi = correlate_decision_with_outcome(&dec_multi, corrs[0].outcome.as_ref());
    assert!(corr_multi.single_turn_cost_delta_usd.is_none());
    assert!(
        corr_multi
            .notes
            .iter()
            .any(|n| n.contains("Decision forecast covers 3-turn horizon"))
    );
}

// -------------------------------------------------------------------------------------------------
// Scenario 3: Chunked SSE streaming response with end-of-stream usage
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn test_prospective_correlation_sc3_sse_streaming_chunked_response() {
    let sse_body = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_sse\",\"role\":\"assistant\",\"usage\":{\"input_tokens\":4000,\"cache_read_input_tokens\":3500}}}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello SSE\"}}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":25}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";

    let (url, _task) = mock(Router::new().route(
        "/v1/messages",
        post(move || async move {
            axum::response::Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "text/event-stream")
                .body(Body::from(sse_body))
                .unwrap()
        }),
    ))
    .await;

    let store = CwStore::open_in_memory().unwrap();
    let session_id = "sess_sc3_sse";
    store
        .upsert_session(&Session {
            id: session_id.to_string(),
            parent_session_id: None,
            harness: HarnessKind::GenericProxy,
            workspace_root: None,
            model: "claude-3-5-sonnet-20241022".to_string(),
            provider: ProviderKind::Anthropic,
            title: Some("Scenario 3 SSE".to_string()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        })
        .unwrap();

    let mut state = DaemonState::new(store.clone());
    state.anthropic_upstream = url;
    let token = state.control_token.clone();
    let app = build_proxy_router(state);

    let raw = json!({
        "model": "claude-3-5-sonnet-20241022",
        "stream": true,
        "messages": [{"role": "user", "content": "Stream test"}]
    })
    .to_string();

    let res = app
        .oneshot(anthropic_request(&raw, Some(session_id), &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    to_bytes(res.into_body(), 10000).await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let turns = store.list_turns(session_id).unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].token_usage.cached_tokens, 3500);
    assert_eq!(turns[0].token_usage.output_tokens, 25);
    assert_eq!(turns[0].outcome.as_deref(), Some("completed"));
}

// -------------------------------------------------------------------------------------------------
// Scenario 4: Provider response with missing or zero usage tokens
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn test_prospective_correlation_sc4_missing_or_zero_usage_tokens() {
    let (url, _task) = mock(Router::new().route(
        "/v1/messages",
        post(|| async {
            Json(json!({
                "id": "msg_sc4",
                "role": "assistant",
                "content": [{"type": "text", "text": "No usage object here"}]
            }))
        }),
    ))
    .await;

    let store = CwStore::open_in_memory().unwrap();
    let session_id = "sess_sc4_nousage";
    store
        .upsert_session(&Session {
            id: session_id.to_string(),
            parent_session_id: None,
            harness: HarnessKind::GenericProxy,
            workspace_root: None,
            model: "claude-3-5-sonnet-20241022".to_string(),
            provider: ProviderKind::Anthropic,
            title: Some("Scenario 4 Missing Usage".to_string()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        })
        .unwrap();

    let mut state = DaemonState::new(store.clone());
    state.anthropic_upstream = url;
    let token = state.control_token.clone();
    let app = build_proxy_router(state);

    let raw = json!({
        "model": "claude-3-5-sonnet-20241022",
        "messages": [{"role": "user", "content": "Test missing usage"}]
    })
    .to_string();

    let res = app
        .oneshot(anthropic_request(&raw, Some(session_id), &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    to_bytes(res.into_body(), 10000).await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let turns = store.list_turns(session_id).unwrap();
    assert_eq!(turns.len(), 1);
    let pricing = cw_core::policy::find_pricing_profile(
        ProviderKind::Anthropic,
        "claude-3-5-sonnet-20241022",
    );
    let outcome = ObservedProviderOutcome::from_completed_turn(
        &turns[0],
        ProviderKind::Anthropic,
        "claude-3-5-sonnet-20241022",
        pricing.as_ref(),
        Utc::now(),
    );
    assert!(!outcome.cache_telemetry_available);
    assert!(outcome.calculated_cost_usd.is_none());
    assert!(
        outcome
            .missing_data_reasons
            .iter()
            .any(|r| r.contains("observed_usage_missing"))
    );

    let dec = ShadowDecisionEvidence {
        record_id: format!("sde_{}_1_cand_test", session_id),
        session_id: session_id.to_string(),
        request_index: 1,
        turn_index: 1,
        provider: ProviderKind::Anthropic,
        model: "claude-3-5-sonnet-20241022".to_string(),
        cache_state: CacheState::WarmVerified,
        provenance: TelemetryProvenance::ObservedProvider,
        prefix_fingerprint: None,
        payload_fingerprint: None,
        candidate_id: Some("cand_01".to_string()),
        plan_id: None,
        policy_evaluation_status: PolicyEvaluationStatus::EvaluatedPreDispatch,
        decision: Some(OptimizationDecision::Apply),
        reason_code: Some(DecisionReasonCode::ModeledEconomicBenefitPositive),
        explanation: "Test missing usage correlation".to_string(),
        estimated_saved_tokens: Some(100),
        estimated_kv_break_tokens: Some(0),
        predicted_baseline_cost_usd: Some(0.003),
        predicted_optimized_cost_usd: Some(0.0028),
        predicted_net_benefit_usd: Some(0.0002),
        forecast_horizon_turns: 1,
        forecast_assumptions: "1-turn".to_string(),
        pricing_model_pattern: Some("claude-3-5-sonnet*".to_string()),
        pricing_version: "2026-10-v1".to_string(),
        policy_version: "0.2.4-cw117c".to_string(),
        decision_timestamp: Utc::now(),
    };

    let corr = correlate_decision_with_outcome(&dec, Some(&outcome));
    assert_eq!(
        corr.correlation_status,
        CorrelationStatus::PairedMissingTelemetry
    );
    assert!(corr.single_turn_cost_delta_usd.is_none());
    assert!(corr.cache_state_consistent.is_none());
}

// -------------------------------------------------------------------------------------------------
// Scenario 5: Early client cancellation before response completion
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn test_prospective_correlation_sc5_client_cancellation_recorded() {
    let store = CwStore::open_in_memory().unwrap();
    let session_id = "sess_sc5_cancel";
    store
        .upsert_session(&Session {
            id: session_id.to_string(),
            parent_session_id: None,
            harness: HarnessKind::GenericProxy,
            workspace_root: None,
            model: "gpt-4o".to_string(),
            provider: ProviderKind::OpenAI,
            title: Some("Scenario 5 Cancel".to_string()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        })
        .unwrap();

    let state = DaemonState::new(store.clone());
    let (turn, _) = cw_core::analyze_turn_from_payload(
        "turn_sc5".to_string(),
        session_id.to_string(),
        None,
        1,
        &json!({"model": "gpt-4o", "messages": [{"role": "user", "content": "Cancel test"}]}),
        ProviderKind::OpenAI,
        None,
        None,
        None,
    )
    .unwrap();

    let mut recorder = cw_proxy::proxy::recorder::Recorder::new(
        state,
        Some(turn),
        ProviderKind::OpenAI,
        None,
        std::time::Instant::now(),
    );
    recorder.sse = true;
    recorder.tap(b"data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n");
    // Client cancels (finish called with "cancelled")
    let handle = recorder.finish("cancelled");
    if let Some(h) = handle {
        h.await.unwrap();
    }

    let turns = store.list_turns(session_id).unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].outcome.as_deref(), Some("cancelled"));
    assert!(turns[0].first_body_byte_ms.is_some());

    let outcome = ObservedProviderOutcome::from_completed_turn(
        &turns[0],
        ProviderKind::OpenAI,
        "gpt-4o",
        None,
        Utc::now(),
    );
    assert_eq!(outcome.execution_outcome, "cancelled");

    let dec = ShadowDecisionEvidence {
        record_id: format!("sde_{}_1_cand_cancel", session_id),
        session_id: session_id.to_string(),
        request_index: 1,
        turn_index: 1,
        provider: ProviderKind::OpenAI,
        model: "gpt-4o".to_string(),
        cache_state: CacheState::WarmVerified,
        provenance: TelemetryProvenance::ObservedProvider,
        prefix_fingerprint: None,
        payload_fingerprint: None,
        candidate_id: Some("cand_cancel".to_string()),
        plan_id: None,
        policy_evaluation_status: PolicyEvaluationStatus::EvaluatedPreDispatch,
        decision: Some(OptimizationDecision::Apply),
        reason_code: Some(DecisionReasonCode::ModeledEconomicBenefitPositive),
        explanation: "Test cancel correlation".to_string(),
        estimated_saved_tokens: Some(100),
        estimated_kv_break_tokens: Some(0),
        predicted_baseline_cost_usd: Some(0.003),
        predicted_optimized_cost_usd: Some(0.0028),
        predicted_net_benefit_usd: Some(0.0002),
        forecast_horizon_turns: 1,
        forecast_assumptions: "1-turn".to_string(),
        pricing_model_pattern: None,
        pricing_version: "2026-10-v1".to_string(),
        policy_version: "0.2.4-cw117c".to_string(),
        decision_timestamp: Utc::now(),
    };

    let corr = correlate_decision_with_outcome(&dec, Some(&outcome));
    assert_eq!(
        corr.correlation_status,
        CorrelationStatus::PairedIncompleteExecution
    );
    assert!(corr.single_turn_cost_delta_usd.is_none());
    assert!(corr.cache_state_consistent.is_none());
}

// -------------------------------------------------------------------------------------------------
// Scenario 6: Provider error response (e.g. HTTP 500/503/429) without corrupting previous decisions
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn test_prospective_correlation_sc6_provider_error_recovery() {
    let call_count = Arc::new(AtomicUsize::new(0));
    let cc = call_count.clone();

    let (url, _task) = mock(Router::new().route(
        "/v1/messages",
        post(move || {
            let cc = cc.clone();
            async move {
                let count = cc.fetch_add(1, Ordering::SeqCst);
                if count == 0 {
                    axum::response::Response::builder()
                        .status(StatusCode::SERVICE_UNAVAILABLE)
                        .body(Body::from("Service Unavailable"))
                        .unwrap()
                } else {
                    axum::response::Response::builder()
                        .status(StatusCode::OK)
                        .header("content-type", "application/json")
                        .body(Body::from(json!({
                            "id": "msg_sc6_recovered",
                            "role": "assistant",
                            "content": [{"type": "text", "text": "Recovered!"}],
                            "usage": {"input_tokens": 1000, "cache_read_input_tokens": 800, "output_tokens": 50}
                        }).to_string()))
                        .unwrap()
                }
            }
        }),
    ))
    .await;

    let store = CwStore::open_in_memory().unwrap();
    let session_id = "sess_sc6_err";
    store
        .upsert_session(&Session {
            id: session_id.to_string(),
            parent_session_id: None,
            harness: HarnessKind::GenericProxy,
            workspace_root: None,
            model: "claude-3-5-sonnet-20241022".to_string(),
            provider: ProviderKind::Anthropic,
            title: Some("Scenario 6 Error Recovery".to_string()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        })
        .unwrap();

    let mut state = DaemonState::new(store.clone());
    state.anthropic_upstream = url;
    let token = state.control_token.clone();
    let app = build_proxy_router(state);

    let raw = json!({
        "model": "claude-3-5-sonnet-20241022",
        "messages": [{"role": "user", "content": "Request 1"}]
    })
    .to_string();

    // First attempt -> 503
    let res1 = app
        .clone()
        .oneshot(anthropic_request(&raw, Some(session_id), &token))
        .await
        .unwrap();
    assert_eq!(res1.status(), StatusCode::SERVICE_UNAVAILABLE);

    // Second attempt -> 200 OK
    let res2 = app
        .oneshot(anthropic_request(&raw, Some(session_id), &token))
        .await
        .unwrap();
    assert_eq!(res2.status(), StatusCode::OK);
    to_bytes(res2.into_body(), 10000).await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let turns = store.list_turns(session_id).unwrap();
    assert!(!turns.is_empty());
}

// -------------------------------------------------------------------------------------------------
// Scenario 7: Repeated responses or retry attempts on same session sequence
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn test_prospective_correlation_sc7_repeated_responses_on_sequence() {
    let store = CwStore::open_in_memory().unwrap();
    let session_id = "sess_sc7_retry";

    let dec = ShadowDecisionEvidence {
        record_id: format!("sde_{}_1_cand_test", session_id),
        session_id: session_id.to_string(),
        request_index: 1,
        turn_index: 1,
        provider: ProviderKind::Anthropic,
        model: "claude-3-5-sonnet-20241022".to_string(),
        cache_state: CacheState::WarmVerified,
        provenance: TelemetryProvenance::ObservedProvider,
        prefix_fingerprint: Some("pfx_sc7".to_string()),
        payload_fingerprint: Some("pfp_sc7".to_string()),
        candidate_id: Some("cand_01".to_string()),
        plan_id: None,
        policy_evaluation_status: PolicyEvaluationStatus::EvaluatedPreDispatch,
        decision: Some(OptimizationDecision::Apply),
        reason_code: Some(DecisionReasonCode::CachePrefixAtRisk),
        explanation: "Test retry".to_string(),
        estimated_saved_tokens: Some(100),
        estimated_kv_break_tokens: Some(0),
        predicted_baseline_cost_usd: Some(0.003),
        predicted_optimized_cost_usd: Some(0.0028),
        predicted_net_benefit_usd: Some(0.0002),
        forecast_horizon_turns: 3,
        forecast_assumptions: "Retry test".to_string(),
        pricing_model_pattern: Some("claude-3-5-sonnet*".to_string()),
        pricing_version: "2026-10-v1".to_string(),
        policy_version: "0.2.4-cw117c".to_string(),
        decision_timestamp: Utc::now(),
    };
    store.save_shadow_decision_evidence(&dec).unwrap();

    // Two outcomes for the same sequence (e.g. retry)
    let outcome1 = ObservedProviderOutcome {
        session_id: session_id.to_string(),
        request_index: 1,
        turn_index: 1,
        provider: ProviderKind::Anthropic,
        model: "claude-3-5-sonnet-20241022".to_string(),
        actual_input_tokens: 2000,
        actual_cached_tokens: 1500,
        actual_uncached_tokens: 500,
        actual_cache_write_tokens: None,
        actual_output_tokens: 50,
        calculated_cost_usd: Some(0.0025),
        reported_cost_usd: None,
        observed_cache_hit_ratio: Some(0.75),
        ttfb_ms: Some(200),
        request_latency_ms: Some(600),
        cache_telemetry_available: true,
        execution_outcome: "completed".to_string(),
        applied_mutations: vec![],
        missing_data_reasons: vec![],
        observed_timestamp: Utc::now(),
    };

    let corr1 = correlate_decision_with_outcome(&dec, Some(&outcome1));
    store.save_correlated_decision_outcome(&corr1).unwrap();

    // Second outcome replaces / updates the first
    let mut outcome2 = outcome1.clone();
    outcome2.actual_cached_tokens = 1800;
    outcome2.observed_cache_hit_ratio = Some(0.90);
    let corr2 = correlate_decision_with_outcome(&dec, Some(&outcome2));
    store.save_correlated_decision_outcome(&corr2).unwrap();

    let loaded = store
        .list_correlated_decision_outcomes(Some(session_id))
        .unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(
        loaded[0].outcome.as_ref().unwrap().actual_cached_tokens,
        1800
    );
}

// -------------------------------------------------------------------------------------------------
// Scenario 8: Concurrent multi-session requests without cross-contamination
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn test_prospective_correlation_sc8_concurrent_multisession_isolation() {
    let (url, _task) = mock(Router::new().route(
        "/v1/messages",
        post(|body: axum::body::Bytes| async move {
            let val: serde_json::Value = serde_json::from_slice(&body).unwrap();
            let marker = val["messages"][0]["content"].as_str().unwrap();
            Json(json!({
                "id": format!("msg_{marker}"),
                "role": "assistant",
                "content": [{"type": "text", "text": marker}],
                "usage": {"input_tokens": 1000, "cache_read_input_tokens": 500, "output_tokens": 10}
            }))
        }),
    ))
    .await;

    let store = CwStore::open_in_memory().unwrap();
    let sess_a = "sess_sc8_a";
    let sess_b = "sess_sc8_b";

    for sid in [sess_a, sess_b] {
        store
            .upsert_session(&Session {
                id: sid.to_string(),
                parent_session_id: None,
                harness: HarnessKind::GenericProxy,
                workspace_root: None,
                model: "claude-3-5-sonnet-20241022".to_string(),
                provider: ProviderKind::Anthropic,
                title: Some(sid.to_string()),
                created_at: Utc::now(),
                updated_at: Utc::now(),
            })
            .unwrap();
    }

    let mut state = DaemonState::new(store.clone());
    state.anthropic_upstream = url;
    let token = state.control_token.clone();
    let app = build_proxy_router(state);

    let raw_a = json!({"model": "claude-3-5-sonnet-20241022", "messages": [{"role": "user", "content": "ALPHA"}]}).to_string();
    let raw_b = json!({"model": "claude-3-5-sonnet-20241022", "messages": [{"role": "user", "content": "BETA"}]}).to_string();

    let app_a = app.clone();
    let app_b = app.clone();
    let token_a = token.clone();
    let token_b = token.clone();

    let t1 = tokio::spawn(async move {
        let res = app_a
            .oneshot(anthropic_request(&raw_a, Some(sess_a), &token_a))
            .await
            .unwrap();
        to_bytes(res.into_body(), 10000).await.unwrap();
    });
    let t2 = tokio::spawn(async move {
        let res = app_b
            .oneshot(anthropic_request(&raw_b, Some(sess_b), &token_b))
            .await
            .unwrap();
        to_bytes(res.into_body(), 10000).await.unwrap();
    });

    let (r1, r2) = tokio::join!(t1, t2);
    r1.unwrap();
    r2.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let turns_a = store.list_turns(sess_a).unwrap();
    let turns_b = store.list_turns(sess_b).unwrap();

    assert_eq!(turns_a.len(), 1);
    assert_eq!(turns_b.len(), 1);
    assert_eq!(turns_a[0].session_id, sess_a);
    assert_eq!(turns_b[0].session_id, sess_b);
}

// -------------------------------------------------------------------------------------------------
// Scenario 9: Unexpected cache miss despite warm expectation (inconsistency detected & noted)
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn test_prospective_correlation_sc9_unexpected_cache_miss_inconsistency() {
    let session_id = "sess_sc9_miss";
    let dec = ShadowDecisionEvidence {
        record_id: format!("sde_{}_2_cand_test", session_id),
        session_id: session_id.to_string(),
        request_index: 2,
        turn_index: 2,
        provider: ProviderKind::Anthropic,
        model: "claude-3-5-sonnet-20241022".to_string(),
        cache_state: CacheState::WarmVerified,
        provenance: TelemetryProvenance::ObservedProvider,
        prefix_fingerprint: Some("pfx_sc9".to_string()),
        payload_fingerprint: Some("pfp_sc9".to_string()),
        candidate_id: Some("cand_01".to_string()),
        plan_id: None,
        policy_evaluation_status: PolicyEvaluationStatus::EvaluatedPreDispatch,
        decision: Some(OptimizationDecision::Apply),
        reason_code: Some(DecisionReasonCode::ModeledEconomicBenefitPositive),
        explanation: "Predicted warm cache hit".to_string(),
        estimated_saved_tokens: Some(200),
        estimated_kv_break_tokens: Some(0),
        predicted_baseline_cost_usd: Some(0.003),
        predicted_optimized_cost_usd: Some(0.0025),
        predicted_net_benefit_usd: Some(0.0005),
        forecast_horizon_turns: 3,
        forecast_assumptions: "Warm cache assumed".to_string(),
        pricing_model_pattern: Some("claude-3-5-sonnet*".to_string()),
        pricing_version: "2026-10-v1".to_string(),
        policy_version: "0.2.4-cw117c".to_string(),
        decision_timestamp: Utc::now(),
    };

    // Provider returned 0 cached tokens despite warm expectation
    let outcome = ObservedProviderOutcome {
        session_id: session_id.to_string(),
        request_index: 2,
        turn_index: 2,
        provider: ProviderKind::Anthropic,
        model: "claude-3-5-sonnet-20241022".to_string(),
        actual_input_tokens: 5000,
        actual_cached_tokens: 0,
        actual_uncached_tokens: 5000,
        actual_cache_write_tokens: Some(5000),
        actual_output_tokens: 100,
        calculated_cost_usd: Some(0.035),
        reported_cost_usd: None,
        observed_cache_hit_ratio: Some(0.0),
        ttfb_ms: Some(400),
        request_latency_ms: Some(1000),
        cache_telemetry_available: true,
        execution_outcome: "completed".to_string(),
        applied_mutations: vec![],
        missing_data_reasons: vec![],
        observed_timestamp: Utc::now(),
    };

    let corr = correlate_decision_with_outcome(&dec, Some(&outcome));
    assert_eq!(corr.correlation_status, CorrelationStatus::MatchedImmediate);
    assert_eq!(corr.cache_state_consistent, Some(false));
    assert!(
        corr.notes
            .iter()
            .any(|n| n.contains("Cache state inconsistency"))
    );
}

// -------------------------------------------------------------------------------------------------
// Scenario 10: Session completion before horizon expires (horizon marked truncated)
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn test_prospective_correlation_sc10_horizon_truncation_on_session_end() {
    let session_id = "sess_sc10_trunc";
    let dec = ShadowDecisionEvidence {
        record_id: format!("sde_{}_1_cand_test", session_id),
        session_id: session_id.to_string(),
        request_index: 1,
        turn_index: 1,
        provider: ProviderKind::Anthropic,
        model: "claude-3-5-sonnet-20241022".to_string(),
        cache_state: CacheState::WarmVerified,
        provenance: TelemetryProvenance::ObservedProvider,
        prefix_fingerprint: Some("pfx_sc10".to_string()),
        payload_fingerprint: Some("pfp_sc10".to_string()),
        candidate_id: Some("cand_01".to_string()),
        plan_id: None,
        policy_evaluation_status: PolicyEvaluationStatus::EvaluatedPreDispatch,
        decision: Some(OptimizationDecision::Apply),
        reason_code: Some(DecisionReasonCode::ModeledEconomicBenefitPositive),
        explanation: "Test horizon truncation".to_string(),
        estimated_saved_tokens: Some(200),
        estimated_kv_break_tokens: Some(0),
        predicted_baseline_cost_usd: Some(0.003),
        predicted_optimized_cost_usd: Some(0.0025),
        predicted_net_benefit_usd: Some(0.0005),
        forecast_horizon_turns: 3,
        forecast_assumptions: "Declared horizon H=3".to_string(),
        pricing_model_pattern: Some("claude-3-5-sonnet*".to_string()),
        pricing_version: "2026-10-v1".to_string(),
        policy_version: "0.2.4-cw117c".to_string(),
        decision_timestamp: Utc::now(),
    };

    // Only 1 subsequent request occurred before session completed (H=3 declared)
    let outcomes = vec![ObservedProviderOutcome {
        session_id: session_id.to_string(),
        request_index: 2,
        turn_index: 2,
        provider: ProviderKind::Anthropic,
        model: "claude-3-5-sonnet-20241022".to_string(),
        actual_input_tokens: 5200,
        actual_cached_tokens: 5000,
        actual_uncached_tokens: 200,
        actual_cache_write_tokens: None,
        actual_output_tokens: 80,
        calculated_cost_usd: Some(0.005),
        reported_cost_usd: None,
        observed_cache_hit_ratio: Some(5000.0 / 5200.0),
        ttfb_ms: Some(200),
        request_latency_ms: Some(600),
        cache_telemetry_available: true,
        execution_outcome: "completed".to_string(),
        applied_mutations: vec![],
        missing_data_reasons: vec![],
        observed_timestamp: Utc::now(),
    }];

    // When session is explicitly terminated (session_terminated = true)
    let pho_ended =
        evaluate_prospective_horizon(&dec, None, &outcomes, dec.forecast_horizon_turns, true);
    assert_eq!(pho_ended.horizon_declared, 3);
    assert_eq!(pho_ended.horizon_observed, 1);
    assert!(!pho_ended.horizon_complete);
    assert_eq!(
        pho_ended.tracking_status,
        HorizonTrackingStatus::TruncatedEarly
    );
    assert_eq!(
        pho_ended.truncation_reason.as_deref(),
        Some("session_ended_before_horizon")
    );
    assert!(pho_ended.horizon_cost_delta_usd.is_none());

    // When session is still active (session_terminated = false)
    let pho_active =
        evaluate_prospective_horizon(&dec, None, &outcomes, dec.forecast_horizon_turns, false);
    assert_eq!(pho_active.horizon_declared, 3);
    assert_eq!(pho_active.horizon_observed, 1);
    assert!(!pho_active.horizon_complete);
    assert_eq!(
        pho_active.tracking_status,
        HorizonTrackingStatus::PendingContinuation
    );
    assert!(pho_active.truncation_reason.is_none());
    assert!(pho_active.horizon_cost_delta_usd.is_none());
}

// -------------------------------------------------------------------------------------------------
// Scenario 11: Strict shadow mode verification (outbound request bytes matching unmutated source)
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn test_prospective_correlation_sc11_strict_shadow_mode_wire_preservation() {
    let captured_payload = Arc::new(Mutex::new(Vec::new()));
    let cp = captured_payload.clone();

    let (url, _task) = mock(Router::new().route(
        "/v1/messages",
        post(move |body: axum::body::Bytes| {
            let cp = cp.clone();
            async move {
                cp.lock().unwrap().extend_from_slice(&body);
                Json(json!({
                    "id": "msg_sc11",
                    "role": "assistant",
                    "content": [{"type": "text", "text": "OK"}],
                    "usage": {"input_tokens": 100, "cache_read_input_tokens": 0, "output_tokens": 10}
                }))
            }
        }),
    ))
    .await;

    let store = CwStore::open_in_memory().unwrap();
    let session_id = "sess_sc11_shadow";
    store
        .upsert_session(&Session {
            id: session_id.to_string(),
            parent_session_id: None,
            harness: HarnessKind::GenericProxy,
            workspace_root: None,
            model: "claude-3-5-sonnet-20241022".to_string(),
            provider: ProviderKind::Anthropic,
            title: Some("Scenario 11 Strict Shadow".to_string()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        })
        .unwrap();

    let mut state = DaemonState::new(store.clone());
    state.anthropic_upstream = url;
    let token = state.control_token.clone();
    let app = build_proxy_router(state);

    let raw = json!({
        "model": "claude-3-5-sonnet-20241022",
        "messages": [
            {"role": "user", "content": "Important unmodified prompt <cw_preserve_literal>"}
        ],
        "metadata": {"custom_client_key": 999}
    })
    .to_string();

    let res = app
        .oneshot(anthropic_request(&raw, Some(session_id), &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    to_bytes(res.into_body(), 10000).await.unwrap();

    let received_bytes = captured_payload.lock().unwrap().clone();
    let received_json: serde_json::Value = serde_json::from_slice(&received_bytes).unwrap();
    let original_json: serde_json::Value = serde_json::from_str(&raw).unwrap();

    // Verify 100% semantic and structural payload preservation with zero mutation
    assert_eq!(received_json, original_json);
}
