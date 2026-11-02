// crates/cw-proxy/tests/live_pilot_test.rs
//
// CW-119: Controlled Live-Agent Product Value Qualification
//
// Tests:
//   1. SpendGuard integration and fail-closed budget preconditions
//   2. Harness readiness suite across Codex, OMP, and Hermes
//   3. Workload fixture verification and independent acceptance testing
//   4. Opt-in live comparison of M1 passthrough and M2 committed PinContext

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use chrono::Utc;
use cw_core::{
    ir::*,
    spend_guard::{SpendGuard, SpendGuardConfig, SpendGuardError},
};
use cw_harnesses::qualification::{
    HarnessReadinessStatus, audit_all_harness_readiness, check_harness_readiness,
    codex_direct_config_args, codex_proxy_config_args, mark_functionally_verified,
};
use cw_proxy::{DaemonState, build_proxy_router};
use cw_store::CwStore;
use serde_json::{Value, json};
use tokio::net::TcpListener;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

// ============================================================================
// 1. Workload Fixture (F1 Bug Fixing)
// ============================================================================

struct F1Fixture {
    workspace_dir: PathBuf,
}

impl F1Fixture {
    pub fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("cw119_f1_ws_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();

        let math_ops = r#"# math_ops.py - initial buggy code
def divide_or_fallback(a, b, fallback=0.0):
    # Bug: raises ZeroDivisionError when b == 0 instead of returning fallback
    return a / b

def safe_mean(numbers):
    # Bug: raises ZeroDivisionError when numbers is empty
    return sum(numbers) / len(numbers)
"#;

        let test_math_ops = r#"import unittest
from math_ops import divide_or_fallback, safe_mean

class TestMathOps(unittest.TestCase):
    def test_divide(self):
        self.assertEqual(divide_or_fallback(10, 2), 5.0)
        self.assertEqual(divide_or_fallback(10, 0, fallback=99.0), 99.0)

    def test_mean(self):
        self.assertEqual(safe_mean([1, 2, 3]), 2.0)
        self.assertEqual(safe_mean([]), 0.0)

if __name__ == '__main__':
    unittest.main()
"#;

        fs::write(dir.join("math_ops.py"), math_ops).unwrap();
        fs::write(dir.join("test_math_ops.py"), test_math_ops).unwrap();

        Self { workspace_dir: dir }
    }

    pub fn turn1_prompt(&self) -> &'static str {
        "Read math_ops.py and test_math_ops.py. Edit math_ops.py directly to fix the ZeroDivisionError in divide_or_fallback (return fallback when b == 0) and in safe_mean (return 0.0 when numbers is empty) so that python3 -m unittest test_math_ops.py passes with 0 failures."
    }

    pub fn turn2_prompt(&self) -> &'static str {
        "Edit math_ops.py directly to add complete PEP 484 type annotations and docstrings to divide_or_fallback and safe_mean. Verify that python3 -m unittest test_math_ops.py still passes with 0 failures."
    }

    pub fn run_acceptance_test(&self) -> bool {
        let status = Command::new("python3")
            .args(["-m", "unittest", "test_math_ops.py"])
            .current_dir(&self.workspace_dir)
            .status();
        match status {
            Ok(s) => s.success(),
            Err(_) => false,
        }
    }
}

impl Drop for F1Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.workspace_dir);
    }
}

fn extract_thread_id(stdout: &str) -> Option<String> {
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

/// Sums Codex `turn.completed` usage. Every field must be present; a missing field is an
/// error rather than zero. Returns `(input, cached_input, output)`.
fn extract_turn_completed_usage(stdout: &str) -> Result<(u64, u64, u64), String> {
    let mut found = None;
    for line in stdout.lines() {
        if let Ok(v) = serde_json::from_str::<Value>(line)
            && v.get("type").and_then(Value::as_str) == Some("turn.completed")
        {
            let usage = v.get("usage").ok_or("turn.completed without usage")?;
            let f = |k: &str| {
                usage
                    .get(k)
                    .and_then(Value::as_u64)
                    .ok_or_else(|| format!("turn.completed usage missing '{k}'"))
            };
            let (i, c, o) = (
                f("input_tokens")?,
                f("cached_input_tokens")?,
                f("output_tokens")?,
            );
            if c > i {
                return Err("cached_input_tokens exceeds input_tokens".into());
            }
            let acc = found.get_or_insert((0, 0, 0));
            acc.0 += i;
            acc.1 += c;
            acc.2 += o;
        }
    }
    found.ok_or_else(|| "no turn.completed event".into())
}

// ============================================================================
// 2. Deterministic Acceptance Tests
// ============================================================================

#[test]
fn test_cw119_spend_guard_integration_and_fail_closed_preconditions() {
    // 1. Without authorized budget, fails closed
    let unauth_guard = SpendGuard::new(SpendGuardConfig {
        authorized_budget_usd: 0.0,
        ..Default::default()
    });
    assert_eq!(
        unauth_guard.reserve_session().unwrap_err(),
        SpendGuardError::SpendNotAuthorized
    );

    // 2. With authorized budget envelope ($0.25)
    let guard = SpendGuard::new(SpendGuardConfig {
        authorized_budget_usd: 0.25,
        session_reservation_usd: 0.05,
        max_session_cost_usd: 0.10,
        max_requests_per_session: 25,
        require_verified_pricing: true,
        ..Default::default()
    });

    assert_eq!(guard.total_spent_usd(), 0.0);
    assert_eq!(guard.total_reserved_usd(), 0.0);
    assert_eq!(guard.remaining_budget_usd(), 0.25);

    // Session 1: Reserve $0.05
    let res1 = guard.reserve_session().expect("Reservation within budget");
    assert!((guard.total_reserved_usd() - 0.05).abs() < 1e-6);
    assert!((guard.remaining_budget_usd() - 0.20).abs() < 1e-6);

    // Turn 1 usage: 10,000 uncached tokens, 5,000 cached tokens, 500 output tokens on gpt-4o-mini
    // uncached: 10k * $0.15 / 1M = $0.0015
    // cached: 5k * $0.075 / 1M = $0.000375
    // output: 500 * $0.60 / 1M = $0.0003
    // cost = $0.002175
    let turn1_cost = guard
        .record_usage(10_000, 5_000, 500, ProviderKind::OpenAI, "gpt-4o-mini")
        .expect("Record turn 1 usage");
    assert!((turn1_cost - 0.002175).abs() < 1e-6);

    // Reconcile reservation (release reserved funds since record_usage already committed turn cost)
    let committed = res1.commit(0.0).expect("Commit session 1 reservation");
    assert_eq!(committed, 0.0);
    assert_eq!(guard.total_reserved_usd(), 0.0);
    assert!((guard.total_spent_usd() - turn1_cost).abs() < 1e-6);

    // Test iteration limit check
    assert!(guard.check_request_count(25).is_ok());
    assert!(matches!(
        guard.check_request_count(26),
        Err(SpendGuardError::RequestLimitExceeded {
            count: 26,
            limit: 25
        })
    ));

    // Test session cost ceiling check
    assert!(guard.check_session_cost(0.09).is_ok());
    assert!(matches!(
        guard.check_session_cost(0.11),
        Err(SpendGuardError::SessionCostExceeded { .. })
    ));
}

#[test]
fn test_cw119_harness_readiness_suite() {
    let reports = audit_all_harness_readiness();
    assert_eq!(
        reports.len(),
        3,
        "Expected 3 audited harnesses (Codex, OMP, Hermes)"
    );

    for r in &reports {
        // Static readiness never claims functional verification.
        assert!(!r.functionally_verified, "{:?}", r.harness);
        assert!(r.status < HarnessReadinessStatus::FunctionallyVerified);
        if !r.is_installed {
            assert!(r.status < HarnessReadinessStatus::Installed);
        }
        if r.status == HarnessReadinessStatus::Configured {
            assert!(r.proxy_config_available);
        }
    }

    // Verify config argument generators
    let proxy_args = codex_proxy_config_args(4401);
    assert!(
        proxy_args
            .iter()
            .any(|a| a.contains("shell_environment_policy.filters.CW_CONTROL_TOKEN=\"exclude\""))
    );
    let direct_args = codex_direct_config_args();
    assert!(
        direct_args
            .iter()
            .any(|a| a.contains("https://api.openai.com/v1"))
    );
}

#[test]
fn test_cw119_workload_fixture_buggy_and_repaired_acceptance() {
    let fixture = F1Fixture::new();

    // Initial state fails tests
    assert!(
        !fixture.run_acceptance_test(),
        "Initial F1 fixture must fail acceptance tests"
    );

    // Apply known valid repair
    let fixed_code = r#"# math_ops.py - repaired
def divide_or_fallback(a, b, fallback=0.0):
    if b == 0:
        return fallback
    return a / b

def safe_mean(numbers):
    if not numbers:
        return 0.0
    return sum(numbers) / len(numbers)
"#;
    fs::write(fixture.workspace_dir.join("math_ops.py"), fixed_code).unwrap();

    // After repair, passes tests
    assert!(
        fixture.run_acceptance_test(),
        "Repaired F1 fixture must pass acceptance tests"
    );
}

// ============================================================================
// 3. Deterministic in-flight SpendGuard enforcement through the real proxy
// ============================================================================

/// Minimal mock Responses upstream. Counts requests; optionally omits usage.
async fn spawn_usage_mock(include_usage: bool) -> (String, Arc<Mutex<usize>>) {
    let hits = Arc::new(Mutex::new(0usize));
    let h = hits.clone();
    let app = axum::Router::new().fallback(move || {
        let h = h.clone();
        async move {
            *h.lock().unwrap() += 1;
            let mut body =
                json!({"id":"resp_1","object":"response","status":"completed","output":[]});
            if include_usage {
                body["usage"] = json!({"input_tokens":100_000,"output_tokens":10,
                    "input_tokens_details":{"cached_tokens":0}});
            }
            axum::Json(body)
        }
    });
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(l, app).await;
    });
    (url, hits)
}

async fn post_responses(app: &axum::Router, token: &str, sid: &str) -> axum::http::StatusCode {
    use tower::ServiceExt;
    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/openai/v1/responses")
        .header("content-type", "application/json")
        .header("x-cw-session-id", sid)
        .header("x-cw-control-token", token)
        .body(axum::body::Body::from(
            json!({"model":"gpt-4o-mini","input":[{"role":"user","content":"hi"}]}).to_string(),
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let _ = axum::body::to_bytes(resp.into_body(), 1 << 20).await;
    status
}

#[tokio::test]
async fn test_cw119_proxy_spend_guard_blocks_requests_before_dispatch() {
    let (url, hits) = spawn_usage_mock(true).await;
    let mut state = DaemonState::new(CwStore::open_in_memory().unwrap());
    state.openai_upstream = url;
    let guard = SpendGuard::new(SpendGuardConfig {
        authorized_budget_usd: 1.0,
        max_requests_per_session: 2,
        max_session_cost_usd: 1.0,
        ..Default::default()
    });
    state.spend_guard = Some(guard.clone());
    let token = state.control_token.clone();
    let app = build_proxy_router(state);

    for _ in 0..2 {
        assert!(post_responses(&app, &token, "s1").await.is_success());
    }
    // Third request is refused by the proxy and never reaches the provider.
    assert_eq!(
        post_responses(&app, &token, "s1").await,
        axum::http::StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(*hits.lock().unwrap(), 2);
    // Incremental accounting from provider usage: 2 x (100k uncached + 10 out) gpt-4o-mini.
    assert!((guard.session_spent_usd("s1") - 0.030012).abs() < 1e-9);
    assert_eq!(guard.total_reserved_usd(), 0.0);
}

#[tokio::test]
async fn test_cw119_proxy_spend_guard_fails_closed_without_usage() {
    let (url, hits) = spawn_usage_mock(false).await;
    let mut state = DaemonState::new(CwStore::open_in_memory().unwrap());
    state.openai_upstream = url;
    let guard = SpendGuard::new(SpendGuardConfig {
        authorized_budget_usd: 1.0,
        ..Default::default()
    });
    state.spend_guard = Some(guard.clone());
    let token = state.control_token.clone();
    let app = build_proxy_router(state);

    assert!(post_responses(&app, &token, "s1").await.is_success());
    assert!(guard.is_killswitch_tripped(), "missing usage must trip");
    assert_eq!(
        post_responses(&app, &token, "s1").await,
        axum::http::StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(*hits.lock().unwrap(), 1);
}

// ============================================================================
// 4. Controlled Live Pilot (Opt-in with CW_QUALIFY_LIVE_PILOT=1)
// ============================================================================

const SUBPROCESS_TIMEOUT: Duration = Duration::from_secs(300);
const PIN_DIRECTIVE: &str = "OPTIMIZATION_ACTIVE: keep helper signatures concise and clean.";

/// Runs a harness subprocess with a hard wall-clock bound; the child is killed on timeout.
async fn run_bounded(mut cmd: tokio::process::Command) -> Result<std::process::Output, String> {
    run_bounded_for(&mut cmd, SUBPROCESS_TIMEOUT).await
}

async fn run_bounded_for(
    cmd: &mut tokio::process::Command,
    timeout: Duration,
) -> Result<std::process::Output, String> {
    cmd.kill_on_drop(true).stdin(std::process::Stdio::null());
    match tokio::time::timeout(timeout, cmd.output()).await {
        Ok(Ok(out)) => Ok(out),
        Ok(Err(e)) => Err(format!("spawn failed: {e}")),
        Err(_) => Err(format!("killed after {timeout:?} timeout")),
    }
}

#[tokio::test]
async fn test_cw119c_bounded_subprocess_kills_timed_out_child() {
    let marker = std::env::temp_dir().join(format!("cw119_timeout_{}", uuid::Uuid::new_v4()));
    let mut child = tokio::process::Command::new("sh");
    child.args(["-c", "sleep 0.3; touch \"$1\"", "sh"]);
    child.arg(&marker);
    assert!(
        run_bounded_for(&mut child, Duration::from_millis(25))
            .await
            .unwrap_err()
            .contains("killed after")
    );
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(!marker.exists(), "timed out child was not terminated");
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode {
    M1,
    M2,
}

fn codex_cmd(
    bin: &Path,
    ws: &Path,
    home: &Path,
    api_key: &str,
    route: &[String],
    tail: &[&str],
    resume: bool,
) -> tokio::process::Command {
    let mut c = tokio::process::Command::new(bin);
    c.current_dir(ws);
    c.arg("exec");
    if resume {
        c.arg("resume");
    }
    c.args([
        "--json",
        "--skip-git-repo-check",
        "--ignore-user-config",
        "-c",
        "approval_policy=\"never\"",
        "-c",
        "sandbox_mode=\"workspace-write\"",
    ]);
    c.args(route).args(tail);
    c.env("OPENAI_API_KEY", api_key).env("CODEX_HOME", home);
    c
}

/// Verifies that every Turn-2 request carried the pinned directive on the wire and that
/// all original items before the final (tail) item are byte-identical and in order.
fn verify_m2_treatment(store: &CwStore, sid: &str, first_t2_seq: u32, rule_id: &str) -> Value {
    let turns = store.list_turns(sid).expect("list M2 turns");
    let committed_rule = store
        .list_mutations(sid)
        .expect("list M2 mutation rules")
        .into_iter()
        .find(|rule| rule.id == rule_id && rule.status == MutationStatus::Committed);
    let mut total_after_commit = 0usize;
    let mut eligible_requests = 0usize;
    let mut stateful_requests = 0usize;
    let mut request_diagnostics = Vec::new();
    let mut eligible_mutated = 0usize;
    let mut idempotent_already_present = 0usize;
    let mut ineligible_abstained = 0usize;
    let mut unexpected_misses = 0usize;
    let mut invalid_mutations = 0usize;
    let mut wire_preservation_failures = 0usize;
    let mut failures = Vec::new();
    for t in turns.iter().filter(|t| t.sequence_number < first_t2_seq) {
        if t.applied_mutation_ids.iter().any(|id| id == rule_id) {
            failures.push(format!(
                "pre-commit turn {} recorded the rule as applied",
                t.sequence_number
            ));
        }
        if let Ok(Some(e)) = store.get_wire_egress(&t.id)
            && String::from_utf8_lossy(&e).contains(PIN_DIRECTIVE)
        {
            failures.push(format!(
                "pre-commit turn {} carried directive",
                t.sequence_number
            ));
        }
    }
    for t in turns.iter().filter(|t| t.sequence_number >= first_t2_seq) {
        total_after_commit += 1;
        let raw = store.get_turn_raw_payload(&t.id).ok().flatten();
        let egress = store
            .get_wire_egress(&t.id)
            .ok()
            .flatten()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
        let (Some(raw), Some(egress)) = (raw, egress) else {
            failures.push(format!(
                "turn {} missing raw or egress capture",
                t.sequence_number
            ));
            continue;
        };
        let eligibility =
            cw_core::normalizer::classify_mutation_eligibility(&raw, ProviderKind::OpenAI);
        request_diagnostics.push(json!({
            "sequence": t.sequence_number,
            "eligibility": eligibility,
            "raw_equals_egress": raw == egress,
            "applied_rule_ids": t.applied_mutation_ids,
            "skipped_rules": t.skipped_mutations,
        }));
        match eligibility {
            cw_core::normalizer::MutationEligibility::StatefulTailOnlyEligible
            | cw_core::normalizer::MutationEligibility::StatelessEligible => {
                if eligibility == cw_core::normalizer::MutationEligibility::StatefulTailOnlyEligible
                {
                    stateful_requests += 1;
                }
                eligible_requests += 1;
                let mut expected_egress = raw.clone();
                let expected_report = committed_rule.as_ref().map(|rule| {
                    cw_core::mutation::apply_mutations_to_payload(
                        &mut expected_egress,
                        ProviderKind::OpenAI,
                        std::slice::from_ref(rule),
                        t.sequence_number,
                    )
                });
                let expected_applied = expected_report
                    .as_ref()
                    .is_some_and(|report| report.applied_rule_ids.iter().any(|id| id == rule_id));
                if !expected_applied
                    && eligibility == cw_core::normalizer::MutationEligibility::StatelessEligible
                {
                    eligible_requests -= 1;
                    ineligible_abstained += 1;
                    if raw != egress || t.applied_mutation_ids.iter().any(|id| id == rule_id) {
                        failures.push(format!(
                            "stateless turn {} without a pin target changed",
                            t.sequence_number
                        ));
                        wire_preservation_failures += 1;
                    }
                    continue;
                }
                let already_present = expected_applied && expected_egress == raw;
                cw_core::local_llm::strip_internal_cw_fields(&mut expected_egress);
                let exact_wire_match = expected_applied && expected_egress == egress;
                if already_present {
                    if t.applied_mutation_ids.iter().any(|id| id == rule_id)
                        && !t
                            .skipped_mutations
                            .iter()
                            .any(|skip| skip.rule_id == rule_id)
                        && exact_wire_match
                    {
                        idempotent_already_present += 1;
                    } else {
                        invalid_mutations += 1;
                        failures.push(format!(
                            "eligible turn {} had pre-existing pin but was not an unchanged idempotent application",
                            t.sequence_number
                        ));
                    }
                    continue;
                }
                let rule_was_recorded = t.applied_mutation_ids.iter().any(|id| id == rule_id);
                let rule_was_skipped = t
                    .skipped_mutations
                    .iter()
                    .any(|skip| skip.rule_id == rule_id);
                let directive_on_wire = egress.to_string().contains(PIN_DIRECTIVE);
                if !rule_was_recorded || rule_was_skipped || !directive_on_wire {
                    unexpected_misses += 1;
                    failures.push(format!(
                        "eligible turn {} was not mutated and recorded (ledger={rule_was_recorded}, wire={directive_on_wire})",
                        t.sequence_number
                    ));
                    continue;
                }
                eligible_mutated += 1;
                if raw["previous_response_id"] != egress["previous_response_id"] {
                    invalid_mutations += 1;
                    failures.push(format!(
                        "turn {} changed previous_response_id",
                        t.sequence_number
                    ));
                }
                let tail_preserved = pin_tail_preserves_input(&raw, &egress);
                if !tail_preserved || !exact_wire_match {
                    invalid_mutations += 1;
                    wire_preservation_failures += 1;
                    failures.push(format!(
                        "turn {} wire differs from the exact authorized transformation or changed original input",
                        t.sequence_number
                    ));
                }
            }
            _ => {
                ineligible_abstained += 1;
                let expected_skip = eligibility
                    .is_op_eligible(&MutationOp::PinContext {
                        label: String::new(),
                        content: String::new(),
                        source_node_id: None,
                    })
                    .err()
                    .unwrap_or("PayloadIneligible");
                let skip_recorded = t
                    .skipped_mutations
                    .iter()
                    .any(|skip| skip.rule_id == rule_id && skip.reason == expected_skip);
                if t.applied_mutation_ids.iter().any(|id| id == rule_id)
                    || !skip_recorded
                    || raw != egress
                {
                    invalid_mutations += 1;
                    wire_preservation_failures += 1;
                    failures.push(format!(
                        "ineligible turn {} was mutated or changed on egress",
                        t.sequence_number
                    ));
                }
            }
        }
    }
    let status = if !failures.is_empty() {
        "FAIL"
    } else if eligible_requests == 0 {
        "BLOCKED_NO_ELIGIBLE_TREATMENT"
    } else if eligible_mutated > 0
        && eligible_mutated + idempotent_already_present == eligible_requests
    {
        "PASS"
    } else {
        "FAIL"
    };
    json!({
        "stateful_eligible_requests": stateful_requests,
        "stateful_treatment_verified": status == "PASS" && stateful_requests > 0,
        "request_diagnostics": request_diagnostics,
        "total_requests_after_commit": total_after_commit,
        "eligible_requests": eligible_requests,
        "eligible_requests_mutated": eligible_mutated,
        "eligible_requests_idempotent_pin_already_present": idempotent_already_present,
        "ineligible_requests_abstained": ineligible_abstained,
        "unexpectedly_unmodified_eligible_requests": unexpected_misses,
        "invalid_or_unsafe_mutations": invalid_mutations,
        "provider_wire_preservation_failures": wire_preservation_failures,
        "status": status,
        "treatment_verified": status == "PASS",
        "failures": failures,
    })
}

fn pin_tail_preserves_input(raw: &Value, egress: &Value) -> bool {
    match (&raw["input"], &egress["input"]) {
        (Value::String(original), Value::String(outbound)) => {
            outbound.starts_with(original) && outbound[original.len()..].contains(PIN_DIRECTIVE)
        }
        (Value::Array(original), Value::Array(outbound)) => {
            if original.is_empty() || original.len() != outbound.len() {
                return false;
            }
            let tail_index = original.len() - 1;
            if original[..tail_index] != outbound[..tail_index] {
                return false;
            }
            let (Some(raw_content), Some(egress_content)) = (
                original[tail_index]["content"].as_array(),
                outbound[tail_index]["content"].as_array(),
            ) else {
                let (Some(raw_text), Some(egress_text)) = (
                    original[tail_index]["content"].as_str(),
                    outbound[tail_index]["content"].as_str(),
                ) else {
                    return false;
                };
                let mut restored_tail = outbound[tail_index].clone();
                restored_tail["content"] = Value::String(raw_text.to_owned());
                return egress_text.starts_with(raw_text)
                    && egress_text[raw_text.len()..].contains(PIN_DIRECTIVE)
                    && restored_tail == original[tail_index];
            };
            if egress_content.len() != raw_content.len() + 1
                || egress_content[..raw_content.len()] != *raw_content
            {
                return false;
            }
            let mut restored_tail = outbound[tail_index].clone();
            restored_tail["content"] = original[tail_index]["content"].clone();
            restored_tail == original[tail_index]
                && egress_content.last().is_some_and(|block| {
                    block["type"] == "input_text"
                        && block["text"]
                            .as_str()
                            .is_some_and(|text| text.contains(PIN_DIRECTIVE))
                })
        }
        _ => false,
    }
}

#[test]
fn m2_treatment_verifier_accepts_only_real_eligible_mutation_and_safe_abstention() {
    let no_type_raw = json!({
        "previous_response_id":"resp_string_tail",
        "input":[{"role":"user","content":"original string content"}]
    });
    let no_type_egress = json!({
        "previous_response_id":"resp_string_tail",
        "input":[{"role":"user","content":format!("original string content{PIN_DIRECTIVE}")}]
    });
    assert!(pin_tail_preserves_input(&no_type_raw, &no_type_egress));

    let store = CwStore::open_in_memory().unwrap();
    let session_id = "cw119b_verifier";
    let now = Utc::now();
    store
        .upsert_session(&Session {
            id: session_id.into(),
            parent_session_id: None,
            harness: HarnessKind::GenericProxy,
            workspace_root: None,
            model: "gpt-4o-mini".into(),
            provider: ProviderKind::OpenAI,
            title: Some("verifier fixture".into()),
            created_at: now,
            updated_at: now,
        })
        .unwrap();
    let rule = MutationRule {
        id: "pin-verifier".into(),
        session_id: session_id.into(),
        op: MutationOp::PinContext {
            label: "fixture".into(),
            content: PIN_DIRECTIVE.into(),
            source_node_id: None,
        },
        status: MutationStatus::Committed,
        estimated_saved_tokens: 0,
        estimated_kv_break_tokens: 0,
        created_at: now,
    };
    store.save_mutation(&rule).unwrap();

    let eligible_raw = json!({"model":"gpt-4o-mini","previous_response_id":"resp_1","input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"continue"}]}]});
    let mut eligible_egress = eligible_raw.clone();
    let report = cw_core::mutation::apply_mutations_to_payload(
        &mut eligible_egress,
        ProviderKind::OpenAI,
        &[rule],
        1,
    );
    let mut eligible_turn = cw_core::analyze_turn_from_payload(
        "turn_1".into(),
        session_id.into(),
        None,
        1,
        &eligible_egress,
        ProviderKind::OpenAI,
        None,
        None,
        None,
    )
    .unwrap()
    .0;
    eligible_turn.applied_mutation_ids = report.applied_rule_ids;
    eligible_turn.skipped_mutations = report.skipped_rules;
    store
        .save_turn(&eligible_turn, Some(&eligible_raw), None, None)
        .unwrap();
    store
        .save_wire_egress(
            &eligible_turn.id,
            serde_json::to_string(&eligible_egress).unwrap().as_bytes(),
        )
        .unwrap();

    let tool_raw = json!({"model":"gpt-4o-mini","previous_response_id":"resp_1","input":[{"type":"function_call_output","call_id":"call_1","output":"done"}]});
    let tool_report = cw_core::mutation::apply_mutations_to_payload(
        &mut tool_raw.clone(),
        ProviderKind::OpenAI,
        &[MutationRule {
            id: "pin-verifier".into(),
            session_id: session_id.into(),
            op: MutationOp::PinContext {
                label: "fixture".into(),
                content: PIN_DIRECTIVE.into(),
                source_node_id: None,
            },
            status: MutationStatus::Committed,
            estimated_saved_tokens: 0,
            estimated_kv_break_tokens: 0,
            created_at: now,
        }],
        2,
    );
    let mut tool_turn = cw_core::analyze_turn_from_payload(
        "turn_2".into(),
        session_id.into(),
        Some("turn_1".into()),
        2,
        &tool_raw,
        ProviderKind::OpenAI,
        None,
        None,
        None,
    )
    .unwrap()
    .0;
    tool_turn.skipped_mutations = tool_report.skipped_rules;
    store
        .save_turn(&tool_turn, Some(&tool_raw), None, None)
        .unwrap();
    store
        .save_wire_egress(
            &tool_turn.id,
            serde_json::to_string(&tool_raw).unwrap().as_bytes(),
        )
        .unwrap();

    let result = verify_m2_treatment(&store, session_id, 1, "pin-verifier");
    assert_eq!(result["status"], "PASS", "{result}");
    assert_eq!(result["eligible_requests"], 1);
    assert_eq!(result["eligible_requests_mutated"], 1);
    assert_eq!(result["ineligible_requests_abstained"], 1);

    let mut stateless_raw = eligible_raw.clone();
    stateless_raw
        .as_object_mut()
        .unwrap()
        .remove("previous_response_id");
    let mut stateless_egress = eligible_egress.clone();
    stateless_egress
        .as_object_mut()
        .unwrap()
        .remove("previous_response_id");
    store
        .save_turn(&eligible_turn, Some(&stateless_raw), None, None)
        .unwrap();
    store
        .save_wire_egress(
            &eligible_turn.id,
            &serde_json::to_vec(&stateless_egress).unwrap(),
        )
        .unwrap();
    let stateless = verify_m2_treatment(&store, session_id, 1, "pin-verifier");
    assert_eq!(stateless["status"], "PASS", "{stateless}");
    assert_eq!(stateless["stateful_treatment_verified"], false);
    assert_eq!(stateless["provider_wire_preservation_failures"], 0);
    let mut stateless_tool = tool_raw.clone();
    stateless_tool
        .as_object_mut()
        .unwrap()
        .remove("previous_response_id");
    tool_turn.skipped_mutations.clear();
    store
        .save_turn(&tool_turn, Some(&stateless_tool), None, None)
        .unwrap();
    store
        .save_wire_egress(&tool_turn.id, &serde_json::to_vec(&stateless_tool).unwrap())
        .unwrap();
    let tool_followup = verify_m2_treatment(&store, session_id, 1, "pin-verifier");
    assert_eq!(tool_followup["status"], "PASS", "{tool_followup}");
    assert_eq!(tool_followup["eligible_requests_mutated"], 1);
    assert_eq!(tool_followup["ineligible_requests_abstained"], 1);

    store
        .save_turn(&eligible_turn, Some(&eligible_raw), None, None)
        .unwrap();
    store
        .save_wire_egress(
            &eligible_turn.id,
            &serde_json::to_vec(&eligible_egress).unwrap(),
        )
        .unwrap();

    let mut valid_eligible_egress = eligible_egress.clone();
    eligible_egress["unrelated_upstream_field"] = json!("unexpected");
    store
        .save_wire_egress(
            "turn_1",
            serde_json::to_string(&eligible_egress).unwrap().as_bytes(),
        )
        .unwrap();
    let tampered = verify_m2_treatment(&store, session_id, 1, "pin-verifier");
    assert_eq!(tampered["status"], "FAIL", "{tampered}");
    assert_eq!(tampered["provider_wire_preservation_failures"], 1);

    let blocks = valid_eligible_egress["input"][0]["content"]
        .as_array_mut()
        .unwrap();
    let duplicate_block = blocks.last().unwrap().clone();
    blocks.push(duplicate_block);
    store
        .save_wire_egress(
            "turn_1",
            serde_json::to_string(&valid_eligible_egress)
                .unwrap()
                .as_bytes(),
        )
        .unwrap();
    let duplicate = verify_m2_treatment(&store, session_id, 1, "pin-verifier");
    assert_eq!(duplicate["status"], "FAIL", "{duplicate}");

    store
        .save_wire_egress(
            "turn_1",
            serde_json::to_string(&valid_eligible_egress)
                .unwrap()
                .as_bytes(),
        )
        .unwrap();

    let suffix_raw = json!({
        "model":"gpt-4o-mini",
        "previous_response_id":"resp_string",
        "input":[{"role":"user","content":"continue"}]
    });
    let rule = store
        .list_mutations(session_id)
        .unwrap()
        .into_iter()
        .find(|rule| rule.id == "pin-verifier")
        .unwrap();
    let mut suffix_egress = suffix_raw.clone();
    let suffix_report = cw_core::mutation::apply_mutations_to_payload(
        &mut suffix_egress,
        ProviderKind::OpenAI,
        &[rule],
        3,
    );
    let mut suffix_turn = cw_core::analyze_turn_from_payload(
        "turn_3".into(),
        session_id.into(),
        Some("turn_2".into()),
        3,
        &suffix_egress,
        ProviderKind::OpenAI,
        None,
        None,
        None,
    )
    .unwrap()
    .0;
    suffix_turn.applied_mutation_ids = suffix_report.applied_rule_ids;
    suffix_turn.skipped_mutations = suffix_report.skipped_rules;
    store
        .save_turn(&suffix_turn, Some(&suffix_raw), None, None)
        .unwrap();
    suffix_egress["input"][0]["content"] = json!(format!(
        "{}unexpected",
        suffix_egress["input"][0]["content"].as_str().unwrap()
    ));
    store
        .save_wire_egress(
            "turn_3",
            serde_json::to_string(&suffix_egress).unwrap().as_bytes(),
        )
        .unwrap();
    let suffix = verify_m2_treatment(&store, session_id, 1, "pin-verifier");
    assert_eq!(suffix["status"], "FAIL", "{suffix}");

    let blocked_store = CwStore::open_in_memory().unwrap();
    let blocked_session = "cw119b_verifier_no_treatment";
    blocked_store
        .upsert_session(&Session {
            id: blocked_session.into(),
            parent_session_id: None,
            harness: HarnessKind::GenericProxy,
            workspace_root: None,
            model: "gpt-4o-mini".into(),
            provider: ProviderKind::OpenAI,
            title: None,
            created_at: now,
            updated_at: now,
        })
        .unwrap();
    let blocked_raw = json!({"model":"gpt-4o-mini","previous_response_id":"resp_1","input":[{"type":"function_call_output","call_id":"call_1","output":"done"}]});
    let mut blocked_turn = cw_core::analyze_turn_from_payload(
        "blocked_turn".into(),
        blocked_session.into(),
        None,
        1,
        &blocked_raw,
        ProviderKind::OpenAI,
        None,
        None,
        None,
    )
    .unwrap()
    .0;
    blocked_turn.skipped_mutations = vec![cw_core::ir::SkippedMutation {
        rule_id: "pin-verifier".into(),
        reason: "SkippedNoEligibleUserTail".into(),
    }];
    blocked_store
        .save_turn(&blocked_turn, Some(&blocked_raw), None, None)
        .unwrap();
    blocked_store
        .save_wire_egress(
            &blocked_turn.id,
            serde_json::to_string(&blocked_raw).unwrap().as_bytes(),
        )
        .unwrap();
    let blocked = verify_m2_treatment(&blocked_store, blocked_session, 1, "pin-verifier");
    assert_eq!(blocked["status"], "BLOCKED_NO_ELIGIBLE_TREATMENT");

    let mut changed_tool_egress = blocked_raw;
    changed_tool_egress["unexpected"] = json!(true);
    blocked_store
        .save_wire_egress(
            "blocked_turn",
            serde_json::to_string(&changed_tool_egress)
                .unwrap()
                .as_bytes(),
        )
        .unwrap();
    let failed = verify_m2_treatment(&blocked_store, blocked_session, 1, "pin-verifier");
    assert_eq!(failed["status"], "FAIL", "{failed}");
    assert_eq!(failed["provider_wire_preservation_failures"], 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "opt-in real Codex live pilot; requires CW_QUALIFY_LIVE_PILOT=1, CW_LIVE_AUTH_USD and OPENAI_API_KEY"]
async fn test_cw119c_controlled_live_treatment_codex_m1_m2() {
    if std::env::var("CW_QUALIFY_LIVE_PILOT").as_deref() != Ok("1") {
        panic!("SKIPPED: Live pilot requires explicit opt-in with CW_QUALIFY_LIVE_PILOT=1");
    }
    // Explicit authorization is mandatory: no default, invalid values rejected
    // before any paid request.
    let auth_usd = cw_core::spend_guard::parse_authorization(
        std::env::var("CW_LIVE_AUTH_USD").ok().as_deref(),
    )
    .expect("CW_LIVE_AUTH_USD must be an explicit, finite, positive USD amount");
    let api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY required");
    let mut readiness = check_harness_readiness(HarnessKind::Codex);
    assert_eq!(
        readiness.status,
        HarnessReadinessStatus::Configured,
        "Codex must be installed and configurable"
    );
    let codex_bin = readiness.binary_path.clone().unwrap();
    let model = "gpt-4o-mini";
    let guard = SpendGuard::new(SpendGuardConfig {
        authorized_budget_usd: auth_usd,
        session_reservation_usd: 0.15,
        max_session_cost_usd: 0.15,
        max_requests_per_session: 25,
        require_verified_pricing: true,
        request_reservation_usd: 0.01,
    });

    let evidence_dir = workspace_root()
        .join("target")
        .join(format!("cw119c_evidence_{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&evidence_dir).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&evidence_dir, fs::Permissions::from_mode(0o700)).unwrap();
    }
    println!("Private evidence directory: {}", evidence_dir.display());
    let mut sessions = Vec::new();
    for mode in [Mode::M1, Mode::M2] {
        println!("\n=== Executing {mode:?} ===");
        let fixture = F1Fixture::new();
        let home =
            std::env::temp_dir().join(format!("codex_home_{mode:?}_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&home).unwrap();
        let sid = format!("cw119_{mode:?}_{}", uuid::Uuid::new_v4().simple());

        // Proxy (M1/M2) with in-flight per-request SpendGuard enforcement.
        let store = CwStore::open(evidence_dir.join(format!("{mode:?}.db"))).unwrap();
        let (route, token, proxy_task) = {
            let mut cfg = store.get_autopilot_config().unwrap();
            if mode == Mode::M2 {
                // Activate the mutation path, but only for explicitly committed rules.
                cfg.enabled = true;
                cfg.zero_break_tail_hygiene = false;
                cfg.auto_cache_safe_pin = false;
                cfg.auto_diet_at_breakpoint = false;
                cfg.local_model_window_guard = false;
                cfg.auto_heal_low_health = false;
                cfg.auto_sync_to_disk = false;
                cfg.shadow_policy.enabled = false;
                cfg.shadow_policy.strict_shadow_only = false;
            } else {
                cfg.enabled = false;
            }
            store.set_autopilot_config(&cfg).unwrap();
            let mut st = DaemonState::new(store.clone());
            st.openai_upstream = "https://api.openai.com".into();
            st.capture_wire_egress = true;
            st.spend_guard = Some(guard.clone());
            let token = st.control_token.clone();
            let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = l.local_addr().unwrap().port();
            let app = build_proxy_router(st);
            let task = tokio::spawn(async move {
                let _ = axum::serve(l, app).await;
            });
            (codex_proxy_config_args(port), Some(token), Some(task))
        };

        let started = Instant::now();
        let with_env = |mut c: tokio::process::Command| {
            if let Some(ref t) = token {
                c.env("CW_SESSION_ID", &sid).env("CW_CONTROL_TOKEN", t);
            }
            c
        };
        let t1 = run_bounded(with_env(codex_cmd(
            &codex_bin,
            &fixture.workspace_dir,
            &home,
            &api_key,
            &route,
            &["-m", model, fixture.turn1_prompt()],
            false,
        )))
        .await
        .expect("Turn 1 subprocess");
        let out1 = String::from_utf8_lossy(&t1.stdout).to_string();
        assert!(
            t1.status.success(),
            "{mode:?} T1: {}",
            String::from_utf8_lossy(&t1.stderr)
        );
        let tid = extract_thread_id(&out1).expect("thread_id");

        let mut rule_id = None;
        let first_t2_seq = store
            .list_turns(&sid)
            .ok()
            .and_then(|t| t.iter().map(|t| t.sequence_number).max())
            .unwrap_or(0)
            + 1;
        if mode == Mode::M2 {
            let id = format!("live_pin_directive_{sid}");
            store
                .save_mutation(&MutationRule {
                    id: id.clone(),
                    session_id: sid.clone(),
                    op: MutationOp::PinContext {
                        label: "ActiveDirective".into(),
                        content: PIN_DIRECTIVE.into(),
                        source_node_id: None,
                    },
                    status: MutationStatus::Committed,
                    estimated_saved_tokens: 0,
                    estimated_kv_break_tokens: 0,
                    created_at: Utc::now(),
                })
                .unwrap();
            rule_id = Some(id);
        }

        let t2 = run_bounded(with_env(codex_cmd(
            &codex_bin,
            &fixture.workspace_dir,
            &home,
            &api_key,
            &route,
            &["-m", model, &tid, fixture.turn2_prompt()],
            true,
        )))
        .await
        .expect("Turn 2 subprocess");
        let out2 = String::from_utf8_lossy(&t2.stdout).to_string();
        let wall_ms = started.elapsed().as_millis() as u64;
        let t2_ok = t2.status.success();
        let acceptance = fixture.run_acceptance_test();
        if let Some(task) = proxy_task {
            task.abort();
        }

        let (i1, c1, o1) = extract_turn_completed_usage(&out1).expect("T1 usage");
        let (i2, c2, o2) = extract_turn_completed_usage(&out2).expect("T2 usage");
        let (inp, cached, out) = (i1 + i2, c1 + c2, o1 + o2);
        let cost = guard.session_spent_usd(&sid);
        let requests = guard.session_request_count(&sid);
        let enforcement = "in_flight_per_request_admission_and_settlement";
        let treatment = rule_id
            .as_deref()
            .map(|r| verify_m2_treatment(&store, &sid, first_t2_seq, r));
        if mode == Mode::M1 && t2_ok && acceptance {
            mark_functionally_verified(&mut readiness, format!("proxied session {sid}"));
        }
        let _ = fs::remove_dir_all(&home);

        println!(
            "{mode:?}: wall={wall_ms}ms in={inp} cached={cached} out={out} cost=${cost:.5} accept={acceptance} treatment={treatment:?}"
        );
        sessions.push(json!({
            "mode": format!("{mode:?}"),
            "wall_time_ms": wall_ms,
            "input_tokens": inp,
            "cached_tokens": cached,
            "output_tokens": out,
            "provider_cost_usd": cost,
            "cost_source": "proxy_observed_provider_usage",
            "codex_reported_usage": {"input": inp, "cached": cached, "output": out},
            "proxy_admitted_requests": requests,
            "spend_enforcement": enforcement,
            "turn2_exit_success": t2_ok,
            "acceptance_passed": acceptance,
            "treatment": treatment,
        }));
        fs::write(
            evidence_dir.join("sessions.json"),
            serde_json::to_vec_pretty(&json!({
                "sessions": sessions, "total_campaign_spend_usd": guard.total_spent_usd(),
                "killswitch_tripped": guard.is_killswitch_tripped(), "spend_envelope_usd": auth_usd,
            }))
            .unwrap(),
        )
        .unwrap();
        assert!(
            t2_ok,
            "{mode:?} T2: {}",
            String::from_utf8_lossy(&t2.stderr)
        );
        assert!(
            acceptance,
            "{mode:?} failed the independent F1 acceptance test"
        );
        assert!(
            !guard.is_killswitch_tripped(),
            "spend guard kill switch tripped"
        );
        assert!(
            requests > 0,
            "no provider requests were admitted through the proxy"
        );
        if mode == Mode::M2 {
            let treatment = treatment.as_ref().expect("M2 treatment report");
            assert_eq!(
                treatment["status"], "PASS",
                "M2 treatment unverified: {treatment}"
            );
        }
        if let Some(t) = treatment {
            assert_eq!(
                t["treatment_verified"], true,
                "M2 treatment unverified: {t}"
            );
        }
    }

    let report = json!({
        "milestone": "CW-119",
        "run_kind": "single_replicate_pilot",
        "timestamp": Utc::now().to_rfc3339(),
        "provider": "OpenAI",
        "model": model,
        "harness_readiness": readiness,
        "total_campaign_spend_usd": guard.total_spent_usd(),
        "spend_envelope_usd": auth_usd,
        "killswitch_tripped": guard.is_killswitch_tripped(),
        "note": "Wall time includes provider latency and agent stochasticity; it is not proxy overhead.",
        "sessions": sessions,
    });
    let out_path = std::env::var("CW119_PILOT_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| workspace_root().join("target/cw119_pilot_run.json"));
    if let Some(p) = out_path.parent() {
        let _ = fs::create_dir_all(p);
    }
    fs::write(&out_path, serde_json::to_string_pretty(&report).unwrap()).unwrap();
    println!("\nSaved pilot dataset to {}", out_path.display());
}
