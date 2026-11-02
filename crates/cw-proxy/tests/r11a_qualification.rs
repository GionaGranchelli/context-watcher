use axum::{Router, body::Body, body::to_bytes, http::Request};
use chrono::{TimeZone, Utc};
use cw_core::{
    analyze_turn_from_payload, calibration::ShadowDecisionEvidence, ir::*,
    tokenizer::estimate_tokens,
};
use cw_proxy::{DaemonState, build_api_router, build_proxy_router};
use cw_store::CwStore;
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use tower::ServiceExt;

const SAMPLE_COUNT: usize = 20;
const PRIMARY_SESSION: &str = "r11a-primary";
const LONG_SESSION: &str = "r11a-long-context";

struct TempDb(PathBuf);

impl TempDb {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("cw_r11a_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        Self(dir.join("cw.db"))
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        if let Some(dir) = self.0.parent() {
            let _ = fs::remove_dir_all(dir);
        }
    }
}

fn timestamp(seconds: i64) -> chrono::DateTime<Utc> {
    Utc.timestamp_opt(1_700_000_000 + seconds, 0)
        .single()
        .unwrap()
}

fn add_session(store: &CwStore, id: &str, seconds: i64) {
    let time = timestamp(seconds);
    store
        .upsert_session(&Session {
            id: id.into(),
            parent_session_id: None,
            harness: HarnessKind::GenericProxy,
            workspace_root: Some("/tmp/cw-r11a-fixture".into()),
            model: "gpt-4o-mini".into(),
            provider: ProviderKind::OpenAI,
            title: Some("generated R11A fixture".into()),
            created_at: time,
            updated_at: time,
        })
        .unwrap();
}

fn add_turn(store: &CwStore, session_id: &str, content: &str) -> String {
    // Keep fixture generation linear; history graph analysis is outside this storage and IO baseline.
    let sequence = store.reserve_turn_sequence(session_id).unwrap();
    let raw = json!({
        "model":"gpt-4o-mini",
        "messages":[{"role":"user","content":content}]
    });
    let (mut turn, _) = analyze_turn_from_payload(
        format!("turn_{session_id}_{sequence}"),
        session_id.into(),
        None,
        sequence,
        &raw,
        ProviderKind::OpenAI,
        None,
        None,
        None,
    )
    .unwrap();
    turn.created_at = timestamp(sequence as i64);
    store.save_turn(&turn, Some(&raw), None, None).unwrap();
    turn.id
}

fn generated_context(target_tokens: u32, seed: usize) -> String {
    let unit = format!("fn generated_{seed}() {{ let value_{seed} = input_{seed}; }}\n");
    let per_unit = estimate_tokens(&unit).max(1);
    let text = unit.repeat((target_tokens / per_unit + 1) as usize);
    assert!(estimate_tokens(&text) >= target_tokens);
    text
}

async fn request(
    app: &Router,
    token: &str,
    uri: &str,
    method: &str,
    body: Body,
) -> (Duration, usize, bool) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "127.0.0.1:4400")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(body)
        .unwrap();
    let started = Instant::now();
    let response = app.clone().oneshot(req).await.unwrap();
    let success = response.status().is_success();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (started.elapsed(), body.len(), success)
}

async fn request_array(
    app: &Router,
    token: &str,
    uri: &str,
) -> (Duration, usize, bool, Vec<Value>) {
    let req = Request::builder()
        .method("GET")
        .uri(uri)
        .header("host", "127.0.0.1:4400")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::empty())
        .unwrap();
    let started = Instant::now();
    let response = app.clone().oneshot(req).await.unwrap();
    let success = response.status().is_success();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let items = serde_json::from_slice::<Value>(&body)
        .ok()
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    (started.elapsed(), body.len(), success, items)
}

async fn proxy_dry_run(app: &Router, token: &str, content: &str) -> (Duration, bool) {
    let req = Request::builder()
        .method("POST")
        .uri("/openai/v1/responses")
        .header("host", "127.0.0.1:4401")
        .header("content-type", "application/json")
        .header("x-cw-session-id", PRIMARY_SESSION)
        .header("x-cw-control-token", token)
        .header("x-cw-dry-run", "1")
        .body(Body::from(
            json!({"model":"gpt-4o-mini","input":[{"role":"user","content":content}]}).to_string(),
        ))
        .unwrap();
    let started = Instant::now();
    let response = app.clone().oneshot(req).await.unwrap();
    let success = response.status().is_success();
    let _ = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (started.elapsed(), success)
}

fn distribution(samples: &[Duration]) -> Value {
    let mut values: Vec<_> = samples.iter().map(Duration::as_secs_f64).collect();
    values.sort_by(f64::total_cmp);
    let percentile = |p: f64| {
        values[((values.len() as f64 * p).ceil() as usize)
            .saturating_sub(1)
            .min(values.len() - 1)]
    };
    json!({
        "samples": values.len(),
        "median_ms": percentile(0.50) * 1000.0,
        "p95_ms": percentile(0.95) * 1000.0,
        "min_ms": values[0] * 1000.0,
        "max_ms": values[values.len() - 1] * 1000.0
    })
}

fn distribution_usize(samples: &[usize]) -> Value {
    let mut values = samples.to_vec();
    values.sort_unstable();
    let percentile = |p: f64| {
        values[((values.len() as f64 * p).ceil() as usize)
            .saturating_sub(1)
            .min(values.len() - 1)]
    };
    json!({"samples":values.len(),"median":percentile(0.50),"p95":percentile(0.95),"min":values[0],"max":values[values.len()-1]})
}

fn query_sql_ms(conn: &Connection, session_count: usize, turn_count: usize) -> Value {
    let mut session_times = Vec::with_capacity(SAMPLE_COUNT);
    let mut turn_times = Vec::with_capacity(SAMPLE_COUNT);
    let mut mutation_times = Vec::with_capacity(SAMPLE_COUNT);
    let mut evidence_times = Vec::with_capacity(SAMPLE_COUNT);
    for _ in 0..SAMPLE_COUNT {
        let started = Instant::now();
        let mut stmt = conn.prepare(
            "SELECT id,parent_session_id,harness,workspace_root,model,provider,title,created_at,updated_at FROM sessions ORDER BY updated_at DESC LIMIT ?1 OFFSET ?2",
        ).unwrap();
        let rows = stmt
            .query_map(
                params![
                    session_count.min(1000) as i64,
                    session_count.saturating_sub(1000) as i64
                ],
                |row| row.get::<_, String>(0),
            )
            .unwrap();
        let _ids: Vec<String> = rows.collect::<Result<_, _>>().unwrap();
        session_times.push(started.elapsed());

        let started = Instant::now();
        let mut stmt = conn.prepare(
            "SELECT turn_json FROM turns WHERE session_id=?1 ORDER BY seq_num DESC,created_at DESC LIMIT ?2 OFFSET ?3",
        ).unwrap();
        let rows = stmt
            .query_map(
                params![
                    PRIMARY_SESSION,
                    turn_count.min(1000) as i64,
                    turn_count.saturating_sub(1000) as i64
                ],
                |row| row.get::<_, String>(0),
            )
            .unwrap();
        let _turns: Vec<String> = rows.collect::<Result<_, _>>().unwrap();
        turn_times.push(started.elapsed());

        let started = Instant::now();
        let mut stmt = conn.prepare(
            "SELECT rule_json FROM mutations WHERE session_id=?1 ORDER BY created_at ASC,id ASC",
        ).unwrap();
        let rows = stmt
            .query_map(params![PRIMARY_SESSION], |row| row.get::<_, String>(0))
            .unwrap();
        let _mutations: Vec<String> = rows.collect::<Result<_, _>>().unwrap();
        mutation_times.push(started.elapsed());

        let started = Instant::now();
        let mut stmt = conn.prepare(
            "SELECT evidence_json FROM shadow_decision_evidence WHERE session_id=?1 ORDER BY request_index ASC",
        ).unwrap();
        let rows = stmt
            .query_map(params![PRIMARY_SESSION], |row| row.get::<_, String>(0))
            .unwrap();
        let _evidence: Vec<String> = rows.collect::<Result<_, _>>().unwrap();
        evidence_times.push(started.elapsed());
    }
    json!({
        "session_page_sql":distribution(&session_times),
        "turn_page_sql":distribution(&turn_times),
        "mutation_query_sql":distribution(&mutation_times),
        "evidence_query_sql":distribution(&evidence_times)
    })
}

fn explain_query_plans(conn: &Connection) -> Value {
    let explain = |sql: &str| {
        let mut statement = conn.prepare(sql).unwrap();
        statement
            .query_map([], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    json!({
        "session_page": explain("EXPLAIN QUERY PLAN SELECT id FROM sessions ORDER BY updated_at DESC LIMIT 100 OFFSET 0"),
        "turn_page": explain("EXPLAIN QUERY PLAN SELECT turn_json FROM turns WHERE session_id='r11a-primary' ORDER BY seq_num DESC,created_at DESC LIMIT 100 OFFSET 0"),
        "mutations": explain("EXPLAIN QUERY PLAN SELECT rule_json FROM mutations WHERE session_id='r11a-primary' ORDER BY created_at ASC,id ASC"),
        "shadow_evidence": explain("EXPLAIN QUERY PLAN SELECT evidence_json FROM shadow_decision_evidence WHERE session_id='r11a-primary' ORDER BY request_index ASC")
    })
}

fn evidence_record(session_id: &str, index: u32) -> ShadowDecisionEvidence {
    use cw_core::{
        calibration::PolicyEvaluationStatus,
        policy::{CacheState, TelemetryProvenance},
    };
    ShadowDecisionEvidence {
        record_id: format!("r11a_evidence_{index}"),
        session_id: session_id.into(),
        request_index: index,
        turn_index: index,
        provider: ProviderKind::OpenAI,
        model: "gpt-4o-mini".into(),
        cache_state: CacheState::Unknown,
        provenance: TelemetryProvenance::None,
        prefix_fingerprint: None,
        payload_fingerprint: None,
        candidate_id: None,
        plan_id: None,
        policy_evaluation_status: PolicyEvaluationStatus::NotEvaluatedMissingCandidates,
        decision: None,
        reason_code: None,
        explanation: "generated R11A evidence record".into(),
        estimated_saved_tokens: None,
        estimated_kv_break_tokens: None,
        predicted_baseline_cost_usd: None,
        predicted_optimized_cost_usd: None,
        predicted_net_benefit_usd: None,
        forecast_horizon_turns: 1,
        forecast_assumptions: "synthetic storage fixture".into(),
        pricing_model_pattern: None,
        pricing_version: "fixture".into(),
        policy_version: "fixture".into(),
        decision_timestamp: timestamp(index as i64),
    }
}

fn proc_memory_kib() -> usize {
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status.lines().find_map(|line| {
                line.strip_prefix("VmHWM:")
                    .and_then(|value| value.split_whitespace().next())
                    .and_then(|value| value.parse().ok())
            })
        })
        .unwrap_or(0)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "opt-in R11A stress benchmark; writes only generated data to a temporary SQLite database"]
async fn cw_r11a_large_history_baseline() {
    let temp = TempDb::new();
    let store = CwStore::open(&temp.0).unwrap();
    let mut state = DaemonState::new(store.clone());
    state.capture_wire_egress = false;
    let token = state.control_token.clone();
    let api = build_api_router(state.clone());
    let proxy = build_proxy_router(state);
    let baseline_session_time = timestamp(1);
    store
        .upsert_session(&Session {
            id: PRIMARY_SESSION.into(),
            parent_session_id: None,
            harness: HarnessKind::GenericProxy,
            workspace_root: Some("/tmp/cw-r11a-fixture".into()),
            model: "gpt-4o-mini".into(),
            provider: ProviderKind::OpenAI,
            title: Some("generated R11A primary".into()),
            created_at: baseline_session_time,
            updated_at: baseline_session_time,
        })
        .unwrap();

    let lock_samples = Arc::new(Mutex::new(Vec::<Duration>::new()));
    let stop_probe = Arc::new(AtomicBool::new(false));
    let probe_store = store.clone();
    let probe_samples = lock_samples.clone();
    let probe_stop = stop_probe.clone();
    let probe = thread::spawn(move || {
        while !probe_stop.load(Ordering::Relaxed) {
            if let Ok(wait) = probe_store.qualification_lock_wait()
                && let Ok(mut samples) = probe_samples.lock()
            {
                samples.push(wait);
            }
            thread::sleep(Duration::from_millis(1));
        }
    });

    let mut reports = Vec::new();
    let mut primary_turn_count = 0usize;
    let mut aux_session_count = 0usize;
    let mut seed_time = Duration::ZERO;
    for target in [10usize, 100, 1_000, 10_000] {
        let started = Instant::now();
        while primary_turn_count < target {
            primary_turn_count += 1;
            add_turn(
                &store,
                PRIMARY_SESSION,
                &format!("generated primary turn {primary_turn_count}: read and update fixture."),
            );
        }
        while aux_session_count + 1 < target {
            aux_session_count += 1;
            let id = format!("r11a-session-{aux_session_count:05}");
            add_session(&store, &id, aux_session_count as i64 + 10);
            add_turn(
                &store,
                &id,
                &format!("generated session {aux_session_count}"),
            );
        }
        seed_time += started.elapsed();

        let mut api_rows = Vec::with_capacity(SAMPLE_COUNT);
        let mut api_bytes = Vec::with_capacity(SAMPLE_COUNT);
        let mut graph_sizes = Vec::with_capacity(SAMPLE_COUNT);
        let mut turns_sizes = Vec::with_capacity(SAMPLE_COUNT);
        let mut latest = Vec::with_capacity(SAMPLE_COUNT);
        let page_limit = target.min(1000);
        let page_offset = target.saturating_sub(page_limit);
        for _ in 0..SAMPLE_COUNT {
            let (elapsed, bytes, success, items) = request_array(
                &api,
                &token,
                &format!("/api/v1/sessions?limit={page_limit}&offset={page_offset}"),
            )
            .await;
            assert!(success, "session list API failed");
            assert_eq!(
                items.len(),
                page_limit.min(target.saturating_sub(page_offset))
            );
            api_rows.push(elapsed);
            api_bytes.push(bytes);

            let (elapsed, bytes, success) = request(
                &api,
                &token,
                &format!("/api/v1/sessions/{PRIMARY_SESSION}/graph"),
                "GET",
                Body::empty(),
            )
            .await;
            assert!(success, "session graph API failed");
            graph_sizes.push((elapsed, bytes));

            let (elapsed, bytes, success, items) = request_array(
                &api,
                &token,
                &format!("/api/v1/sessions/{PRIMARY_SESSION}/turns?limit={page_limit}&offset={page_offset}"),
            )
            .await;
            assert!(success, "turn page API failed");
            assert_eq!(
                items.len(),
                page_limit.min(target.saturating_sub(page_offset))
            );
            turns_sizes.push((elapsed, bytes));

            let started = Instant::now();
            assert!(store.get_latest_turn(PRIMARY_SESSION).unwrap().is_some());
            latest.push(started.elapsed());
        }
        let graph_size = graph_sizes[graph_sizes.len() - 1].1;
        let turn_size = turns_sizes[turns_sizes.len() - 1].1;
        let sqlite = Connection::open(&temp.0).unwrap();
        reports.push(json!({
            "sessions": target,
            "primary_turns": target,
            "session_list_api": {"latency":distribution(&api_rows),"response_bytes":distribution_usize(&api_bytes)},
            "latest_turn_store_total": distribution(&latest),
            "graph_api": {"median_ms":distribution(&graph_sizes.iter().map(|v|v.0).collect::<Vec<_>>())["median_ms"], "p95_ms":distribution(&graph_sizes.iter().map(|v|v.0).collect::<Vec<_>>())["p95_ms"], "response_bytes":graph_size},
            "turn_page_api": {"median_ms":distribution(&turns_sizes.iter().map(|v|v.0).collect::<Vec<_>>())["median_ms"], "p95_ms":distribution(&turns_sizes.iter().map(|v|v.0).collect::<Vec<_>>())["p95_ms"], "response_bytes":turn_size},
            "sqlite_pages": query_sql_ms(&sqlite, target, target),
        }));
    }

    let mut session_ids = HashSet::new();
    let mut session_page_times = Vec::new();
    let mut session_page_bytes = Vec::new();
    let mut turn_sequences = HashSet::new();
    let mut turn_page_times = Vec::new();
    let mut turn_page_bytes = Vec::new();
    for offset in (0..10_000).step_by(1000) {
        let (elapsed, bytes, success, items) = request_array(
            &api,
            &token,
            &format!("/api/v1/sessions?limit=1000&offset={offset}"),
        )
        .await;
        assert!(success, "full session page {offset} failed");
        assert_eq!(items.len(), 1000);
        for item in items {
            assert!(session_ids.insert(item["id"].as_str().unwrap().to_owned()));
        }
        session_page_times.push(elapsed);
        session_page_bytes.push(bytes);

        let (elapsed, bytes, success, items) = request_array(
            &api,
            &token,
            &format!("/api/v1/sessions/{PRIMARY_SESSION}/turns?limit=1000&offset={offset}"),
        )
        .await;
        assert!(success, "full turn page {offset} failed");
        assert_eq!(items.len(), 1000);
        for item in items {
            assert!(turn_sequences.insert(item["sequence_number"].as_u64().unwrap()));
        }
        turn_page_times.push(elapsed);
        turn_page_bytes.push(bytes);
    }
    assert_eq!(session_ids.len(), 10_000);
    assert_eq!(turn_sequences, (1..=10_000).collect());
    let full_history_pagination = json!({
        "session_pages":session_page_times.len(),
        "unique_sessions":session_ids.len(),
        "session_api_latency":distribution(&session_page_times),
        "session_response_bytes":distribution_usize(&session_page_bytes),
        "turn_pages":turn_page_times.len(),
        "unique_turn_sequences":turn_sequences.len(),
        "turn_api_latency":distribution(&turn_page_times),
        "turn_response_bytes":distribution_usize(&turn_page_bytes)
    });

    add_session(&store, LONG_SESSION, 20_000);
    let mut generated_tokens = 0u64;
    for sequence in 1..=100 {
        let content = generated_context(3_500, sequence);
        add_turn(&store, LONG_SESSION, &content);
        generated_tokens += u64::from(estimate_tokens(&content));
    }
    assert!(generated_tokens >= 350_000);

    for index in 1..=1000 {
        store
            .save_shadow_decision_evidence(&evidence_record(PRIMARY_SESSION, index))
            .unwrap();
    }
    for index in 1..=100 {
        store
            .save_mutation(&MutationRule {
                id: format!("r11a-mutation-{index}"),
                session_id: PRIMARY_SESSION.into(),
                op: MutationOp::PinContext {
                    label: "fixture".into(),
                    content: format!("generated pin {index}"),
                    source_node_id: None,
                },
                status: MutationStatus::Committed,
                estimated_saved_tokens: 0,
                estimated_kv_break_tokens: 0,
                created_at: timestamp(index as i64),
            })
            .unwrap();
    }
    let long_graph = request(
        &api,
        &token,
        &format!("/api/v1/sessions/{LONG_SESSION}/graph"),
        "GET",
        Body::empty(),
    )
    .await;
    let long_latest = store.get_latest_turn(LONG_SESSION).unwrap().unwrap();
    assert!(generated_tokens >= 350_000 && long_latest.token_usage.input_tokens >= 3_500);
    let long_raw = store
        .get_turn_raw_payload(&long_latest.id)
        .unwrap()
        .unwrap();
    assert_eq!(
        long_raw["messages"][0]["content"].as_str().unwrap().len(),
        generated_context(3_500, 100).len()
    );
    let evidence_timer = Instant::now();
    let evidence_count = store
        .list_shadow_decision_evidence(Some(PRIMARY_SESSION))
        .unwrap()
        .len();
    let evidence_read = evidence_timer.elapsed();
    let mutation_timer = Instant::now();
    let mutation_count = store.list_mutations(PRIMARY_SESSION).unwrap().len();
    let mutation_read = mutation_timer.elapsed();
    assert_eq!(evidence_count, 1000);
    assert_eq!(mutation_count, 100);

    let latest_id = long_latest.id.clone();
    let mut mutation_api_times = Vec::with_capacity(SAMPLE_COUNT);
    let mut mutation_api_bytes = 0usize;
    let mut effectiveness_api_times = Vec::with_capacity(SAMPLE_COUNT);
    let mut raw_api_times = Vec::with_capacity(SAMPLE_COUNT);
    let mut raw_api_bytes = 0usize;
    let mut evidence_times = Vec::with_capacity(SAMPLE_COUNT);
    for _ in 0..SAMPLE_COUNT {
        let (elapsed, bytes, success) = request(
            &api,
            &token,
            &format!("/api/v1/sessions/{PRIMARY_SESSION}/mutations"),
            "GET",
            Body::empty(),
        )
        .await;
        assert!(success, "mutation listing API failed");
        mutation_api_times.push(elapsed);
        mutation_api_bytes = bytes;
        let (elapsed, _, success) = request(
            &api,
            &token,
            &format!("/api/v1/sessions/{PRIMARY_SESSION}/effectiveness"),
            "GET",
            Body::empty(),
        )
        .await;
        assert!(success, "effectiveness API failed");
        effectiveness_api_times.push(elapsed);
        let (elapsed, bytes, success) = request(
            &api,
            &token,
            &format!("/api/v1/sessions/{LONG_SESSION}/turns/{latest_id}/raw"),
            "GET",
            Body::empty(),
        )
        .await;
        assert!(success, "raw payload API failed");
        raw_api_times.push(elapsed);
        raw_api_bytes = bytes;
        let started = Instant::now();
        assert_eq!(
            store
                .list_shadow_decision_evidence(Some(PRIMARY_SESSION))
                .unwrap()
                .len(),
            evidence_count
        );
        evidence_times.push(started.elapsed());
    }

    let throughput_times = Arc::new(Mutex::new(Vec::<Duration>::new()));
    let ingest_times = Arc::new(Mutex::new(Vec::<Duration>::new()));
    let proxy_write_times = Arc::new(Mutex::new(Vec::<Duration>::new()));
    let mutation_write_times = Arc::new(Mutex::new(Vec::<Duration>::new()));
    let success_count = Arc::new(AtomicUsize::new(0));
    let errors = Arc::new(AtomicUsize::new(0));
    let mut tasks = Vec::new();
    let concurrent_started = Instant::now();
    for reader in 0..4 {
        let app = api.clone();
        let token = token.clone();
        let times = throughput_times.clone();
        let ok = success_count.clone();
        let failures = errors.clone();
        tasks.push(tokio::spawn(async move {
            for index in 0..50 {
                let uri = match reader {
                    0 => format!("/api/v1/sessions?limit=100&offset={index}"),
                    1 => format!("/api/v1/sessions/{PRIMARY_SESSION}/graph"),
                    2 => format!("/api/v1/sessions/{PRIMARY_SESSION}/mutations"),
                    _ => format!("/api/v1/sessions/{PRIMARY_SESSION}/effectiveness"),
                };
                let (elapsed, _, success) = request(&app, &token, &uri, "GET", Body::empty()).await;
                if success {
                    ok.fetch_add(1, Ordering::Relaxed);
                } else {
                    failures.fetch_add(1, Ordering::Relaxed);
                }
                times.lock().unwrap().push(elapsed);
            }
        }));
    }
    {
        let app = api.clone();
        let token = token.clone();
        let times = throughput_times.clone();
        let write_times = ingest_times.clone();
        let ok = success_count.clone();
        let failures = errors.clone();
        tasks.push(tokio::spawn(async move {
            for index in 0..50 {
                let body = Body::from(json!({
                    "session_id":PRIMARY_SESSION,
                    "provider":"openai",
                    "model":"gpt-4o-mini",
                    "payload":{"model":"gpt-4o-mini","messages":[{"role":"user","content":format!("concurrent generated ingest {index}")}]}
                }).to_string());
                let (elapsed, _, success) = request(&app, &token, "/api/v1/ingest", "POST", body).await;
                if success { ok.fetch_add(1, Ordering::Relaxed); } else { failures.fetch_add(1, Ordering::Relaxed); }
                times.lock().unwrap().push(elapsed);
                write_times.lock().unwrap().push(elapsed);
            }
        }));
    }
    {
        let app = proxy.clone();
        let token = token.clone();
        let times = throughput_times.clone();
        let write_times = proxy_write_times.clone();
        let ok = success_count.clone();
        let failures = errors.clone();
        tasks.push(tokio::spawn(async move {
            for index in 0..50 {
                let (elapsed, success) =
                    proxy_dry_run(&app, &token, &format!("concurrent proxy request {index}")).await;
                if success {
                    ok.fetch_add(1, Ordering::Relaxed);
                } else {
                    failures.fetch_add(1, Ordering::Relaxed);
                }
                times.lock().unwrap().push(elapsed);
                write_times.lock().unwrap().push(elapsed);
            }
        }));
    }
    {
        let evidence_store = store.clone();
        let times = throughput_times.clone();
        let ok = success_count.clone();
        let failures = errors.clone();
        tasks.push(tokio::spawn(async move {
            for _ in 0..50 {
                let started = Instant::now();
                let success = evidence_store
                    .list_shadow_decision_evidence(Some(PRIMARY_SESSION))
                    .is_ok_and(|records| records.len() == 1000);
                if success {
                    ok.fetch_add(1, Ordering::Relaxed);
                } else {
                    failures.fetch_add(1, Ordering::Relaxed);
                }
                times.lock().unwrap().push(started.elapsed());
            }
        }));
    }
    {
        let latest_store = store.clone();
        let times = throughput_times.clone();
        let ok = success_count.clone();
        let failures = errors.clone();
        tasks.push(tokio::spawn(async move {
            for _ in 0..50 {
                let started = Instant::now();
                let success = latest_store
                    .get_latest_turn(PRIMARY_SESSION)
                    .is_ok_and(|turn| turn.is_some());
                if success {
                    ok.fetch_add(1, Ordering::Relaxed);
                } else {
                    failures.fetch_add(1, Ordering::Relaxed);
                }
                times.lock().unwrap().push(started.elapsed());
            }
        }));
    }
    {
        let app = api.clone();
        let token = token.clone();
        let turn_id = latest_id.clone();
        let times = throughput_times.clone();
        let ok = success_count.clone();
        let failures = errors.clone();
        tasks.push(tokio::spawn(async move {
            for _ in 0..50 {
                let (elapsed, _, success) = request(
                    &app,
                    &token,
                    &format!("/api/v1/sessions/{LONG_SESSION}/turns/{turn_id}/raw"),
                    "GET",
                    Body::empty(),
                )
                .await;
                if success {
                    ok.fetch_add(1, Ordering::Relaxed);
                } else {
                    failures.fetch_add(1, Ordering::Relaxed);
                }
                times.lock().unwrap().push(elapsed);
            }
        }));
    }
    {
        let mutation_store = store.clone();
        let times = throughput_times.clone();
        let write_times = mutation_write_times.clone();
        let ok = success_count.clone();
        let failures = errors.clone();
        tasks.push(tokio::spawn(async move {
            for index in 0..50 {
                let started = Instant::now();
                let success = mutation_store
                    .save_mutation(&MutationRule {
                        id: format!("r11a-concurrent-mutation-{index}"),
                        session_id: PRIMARY_SESSION.into(),
                        op: MutationOp::PinContext {
                            label: "concurrent fixture".into(),
                            content: format!("generated concurrent pin {index}"),
                            source_node_id: None,
                        },
                        status: MutationStatus::Committed,
                        estimated_saved_tokens: 0,
                        estimated_kv_break_tokens: 0,
                        created_at: timestamp(30_000 + index as i64),
                    })
                    .is_ok();
                let elapsed = started.elapsed();
                if success {
                    ok.fetch_add(1, Ordering::Relaxed);
                } else {
                    failures.fetch_add(1, Ordering::Relaxed);
                }
                times.lock().unwrap().push(elapsed);
                write_times.lock().unwrap().push(elapsed);
            }
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
    let concurrent_wall = concurrent_started.elapsed();
    stop_probe.store(true, Ordering::Relaxed);
    probe.join().unwrap();
    let lock_waits = lock_samples.lock().unwrap().clone();
    let concurrent_times = throughput_times.lock().unwrap().clone();
    let ingest_samples = ingest_times.lock().unwrap().clone();
    let proxy_write_samples = proxy_write_times.lock().unwrap().clone();
    let mutation_write_samples = mutation_write_times.lock().unwrap().clone();
    let db_bytes = fs::metadata(&temp.0).map(|m| m.len()).unwrap_or(0);
    let wal_path = PathBuf::from(format!("{}-wal", temp.0.display()));
    let wal_bytes = fs::metadata(wal_path).map(|m| m.len()).unwrap_or(0);
    let sqlite = Connection::open(&temp.0).unwrap();
    let concurrent_turn_count: i64 = sqlite
        .query_row(
            "SELECT COUNT(*) FROM turns WHERE session_id=?1",
            params![PRIMARY_SESSION],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(concurrent_turn_count, 10_100);
    let concurrent_mutations = store.list_mutations(PRIMARY_SESSION).unwrap();
    assert_eq!(concurrent_mutations.len(), 150);
    assert!(
        concurrent_mutations
            .iter()
            .all(|rule| rule.status == MutationStatus::Committed)
    );
    assert_eq!(
        store
            .list_shadow_decision_evidence(Some(PRIMARY_SESSION))
            .unwrap()
            .len(),
        1000
    );
    let sqlite_version: String = sqlite
        .query_row("SELECT sqlite_version()", [], |row| row.get(0))
        .unwrap();
    let report = json!({
        "fixture_base_commit":"d5e45a08f1ffb29b753f9b1094859823aed96fb4",
        "measured_revision":std::env::var("CW_R11A_REVISION").unwrap_or_else(|_| "working tree".into()),
        "build":"release",
        "sqlite_version":sqlite_version,
        "samples_per_read_cell":SAMPLE_COUNT,
        "seed_elapsed_seconds":seed_time.as_secs_f64(),
        "scales":reports,
        "full_history_pagination":full_history_pagination,
        "long_context":{"turns":100,"generated_tokens":generated_tokens,"graph_status_success":long_graph.2,"graph_elapsed_ms":long_graph.0.as_secs_f64()*1000.0,"graph_response_bytes":long_graph.1},
        "mutation_read":{"rows":mutation_count,"elapsed_ms":mutation_read.as_secs_f64()*1000.0},
        "evidence_read":{"rows":evidence_count,"elapsed_ms":evidence_read.as_secs_f64()*1000.0},
        "mutation_api":{"rows":mutation_count,"latency":distribution(&mutation_api_times),"response_bytes":mutation_api_bytes},
        "effectiveness_api":{"latency":distribution(&effectiveness_api_times)},
        "raw_payload_api":{"latency":distribution(&raw_api_times),"response_bytes":raw_api_bytes},
        "evidence_store_read":{"rows":evidence_count,"latency":distribution(&evidence_times)},
        "concurrent":{"successful_operations":success_count.load(Ordering::Relaxed),"errors":errors.load(Ordering::Relaxed),"operations":concurrent_times.len(),"wall_seconds":concurrent_wall.as_secs_f64(),"latency":distribution(&concurrent_times),"throughput_ops_per_second":concurrent_times.len() as f64 / concurrent_wall.as_secs_f64(),"api_ingest_writes":{"count":ingest_samples.len(),"latency":distribution(&ingest_samples),"throughput_ops_per_second":ingest_samples.len() as f64 / concurrent_wall.as_secs_f64()},"proxy_dry_run_writes":{"count":proxy_write_samples.len(),"latency":distribution(&proxy_write_samples),"throughput_ops_per_second":proxy_write_samples.len() as f64 / concurrent_wall.as_secs_f64()},"mutation_store_writes":{"count":mutation_write_samples.len(),"latency":distribution(&mutation_write_samples),"throughput_ops_per_second":mutation_write_samples.len() as f64 / concurrent_wall.as_secs_f64()}},
        "mutex_lock_wait_probe":distribution(&lock_waits),
        "memory":{"process_peak_rss_kib":proc_memory_kib()},
        "database":{"db_bytes":db_bytes,"wal_bytes":wal_bytes},
        "query_plans":explain_query_plans(&sqlite),
        "note":"All rows and payload text are generated. No provider is contacted. Lock wait is sampled by a separate 1ms probe thread during the workload; the probe itself briefly acquires and releases the shared connection mutex."
    });
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/cw-r11a-baseline.json");
    fs::create_dir_all(out.parent().unwrap()).unwrap();
    fs::write(out, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
}
