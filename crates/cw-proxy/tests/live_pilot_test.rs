// crates/cw-proxy/tests/live_pilot_test.rs
//
// CW-119: Controlled Live-Agent Product Value Qualification
//
// Tests:
//   1. SpendGuard integration and fail-closed budget preconditions
//   2. Harness readiness suite across Codex, OMP, and Hermes
//   3. Workload fixture verification and independent acceptance testing
//   4. Controlled live pilot across Modes M0 (Native), M1 (Passthrough), and M2 (Optimized)

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};

use chrono::Utc;
use cw_core::{
    ir::*,
    spend_guard::{SpendGuard, SpendGuardConfig, SpendGuardError},
};
use cw_harnesses::qualification::{
    HarnessReadinessStatus, audit_all_harness_readiness, check_harness_readiness,
    codex_direct_config_args, codex_proxy_config_args,
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

fn extract_turn_completed_usage(stdout: &str) -> Option<(u32, u32, u32)> {
    for line in stdout.lines() {
        if let Ok(v) = serde_json::from_str::<Value>(line)
            && v.get("type").and_then(Value::as_str) == Some("turn.completed")
            && let Some(usage) = v.get("usage")
        {
            let input = usage
                .get("input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0) as u32;
            let cached = usage
                .get("cached_input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0) as u32;
            let output = usage
                .get("output_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0) as u32;
            return Some((input, cached, output));
        }
    }
    None
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
        // Assert each harness is installed on this qualification host
        assert!(
            r.is_installed,
            "Expected harness {:?} to be installed on host",
            r.harness
        );
        assert_eq!(
            r.status,
            HarnessReadinessStatus::Ready,
            "Harness {:?} must be in Ready status",
            r.harness
        );
        assert!(
            r.credential_isolation_supported,
            "Harness {:?} must support credential isolation",
            r.harness
        );
        assert!(
            r.proxy_routing_supported,
            "Harness {:?} must support proxy routing",
            r.harness
        );
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
// 3. Controlled Live Pilot (Opt-in with CW_QUALIFY_LIVE_PILOT=1)
// ============================================================================

async fn run_codex_blocking(mut cmd: Command) -> std::process::Output {
    tokio::task::spawn_blocking(move || cmd.output().expect("Failed to execute codex process"))
        .await
        .expect("Task join error")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "opt-in real Codex live pilot; requires CW_QUALIFY_LIVE_PILOT=1 and CW_LIVE_AUTH_USD and OPENAI_API_KEY"]
async fn test_cw119_controlled_live_pilot_codex_m0_m1_m2() {
    let opt_in = std::env::var("CW_QUALIFY_LIVE_PILOT").unwrap_or_default();
    if opt_in != "1" {
        panic!("SKIPPED: Live pilot requires explicit opt-in with CW_QUALIFY_LIVE_PILOT=1");
    }

    let api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY required for live pilot");
    let codex_report = check_harness_readiness(HarnessKind::Codex);
    assert!(
        codex_report.is_installed,
        "Codex CLI must be installed for live pilot"
    );
    let codex_bin = codex_report.binary_path.expect("Codex binary path");

    // SpendGuard setup: envelope $0.50 USD
    let auth_usd = std::env::var("CW_LIVE_AUTH_USD")
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .unwrap_or(0.50);

    let spend_guard = SpendGuard::new(SpendGuardConfig {
        authorized_budget_usd: auth_usd,
        session_reservation_usd: 0.08,
        max_session_cost_usd: 0.15,
        max_requests_per_session: 25,
        require_verified_pricing: true,
    });

    let model = "gpt-4o-mini";
    let mut session_results = Vec::new();

    // ------------------------------------------------------------------------
    // Mode M0: Native Baseline
    // ------------------------------------------------------------------------
    println!("\n=== Executing Mode M0: Native Baseline ===");
    let m0_fixture = F1Fixture::new();
    let m0_home = std::env::temp_dir().join(format!("codex_home_m0_{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&m0_home).unwrap();
    let m0_res = spend_guard
        .reserve_session()
        .expect("M0 admission reservation");

    let m0_start = Instant::now();

    // Turn 1
    let mut cmd_m0_t1 = Command::new(&codex_bin);
    cmd_m0_t1.current_dir(&m0_fixture.workspace_dir);
    cmd_m0_t1.args([
        "exec",
        "--json",
        "--skip-git-repo-check",
        "--ignore-user-config",
        "-c",
        "approval_policy=\"never\"",
        "-c",
        "sandbox_mode=\"workspace-write\"",
    ]);
    cmd_m0_t1.args(codex_direct_config_args());
    cmd_m0_t1.args(["-m", model, m0_fixture.turn1_prompt()]);
    cmd_m0_t1.stdin(std::process::Stdio::null());
    cmd_m0_t1.env("OPENAI_API_KEY", &api_key);
    cmd_m0_t1.env("CODEX_HOME", &m0_home);

    let out_m0_t1 = run_codex_blocking(cmd_m0_t1).await;
    let stdout_m0_t1 = String::from_utf8_lossy(&out_m0_t1.stdout);
    assert!(
        out_m0_t1.status.success(),
        "M0 Turn 1 failed: {}",
        String::from_utf8_lossy(&out_m0_t1.stderr)
    );
    let tid_m0 = extract_thread_id(&stdout_m0_t1).expect("M0 Turn 1 thread_id");
    let (in_m0_t1, c_m0_t1, out_m0_t1_tok) =
        extract_turn_completed_usage(&stdout_m0_t1).expect("M0 Turn 1 usage");

    // Turn 2
    let mut cmd_m0_t2 = Command::new(&codex_bin);
    cmd_m0_t2.current_dir(&m0_fixture.workspace_dir);
    cmd_m0_t2.args([
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
    cmd_m0_t2.args(codex_direct_config_args());
    cmd_m0_t2.args(["-m", model, &tid_m0, m0_fixture.turn2_prompt()]);
    cmd_m0_t2.stdin(std::process::Stdio::null());
    cmd_m0_t2.env("OPENAI_API_KEY", &api_key);
    cmd_m0_t2.env("CODEX_HOME", &m0_home);

    let out_m0_t2 = run_codex_blocking(cmd_m0_t2).await;
    let stdout_m0_t2 = String::from_utf8_lossy(&out_m0_t2.stdout);
    assert!(
        out_m0_t2.status.success(),
        "M0 Turn 2 failed: {}",
        String::from_utf8_lossy(&out_m0_t2.stderr)
    );
    let (in_m0_t2, c_m0_t2, out_m0_t2_tok) =
        extract_turn_completed_usage(&stdout_m0_t2).expect("M0 Turn 2 usage");

    let m0_elapsed_ms = m0_start.elapsed().as_millis() as u64;
    let m0_acceptance = m0_fixture.run_acceptance_test();
    assert!(m0_acceptance, "M0 acceptance tests must pass");

    let m0_total_in = in_m0_t1 + in_m0_t2;
    let m0_total_c = c_m0_t1 + c_m0_t2;
    let m0_total_out = out_m0_t1_tok + out_m0_t2_tok;
    let m0_cost = spend_guard
        .record_usage(
            m0_total_in - m0_total_c,
            m0_total_c,
            m0_total_out,
            ProviderKind::OpenAI,
            model,
        )
        .expect("M0 record usage");
    m0_res.commit(0.0).expect("Commit M0 reservation");
    let _ = fs::remove_dir_all(&m0_home);

    println!(
        "M0 Completed: WallTime={}ms, In={}, Cached={}, Out={}, Cost=${:.5}, Acceptance={}",
        m0_elapsed_ms, m0_total_in, m0_total_c, m0_total_out, m0_cost, m0_acceptance
    );

    session_results.push(json!({
        "mode": "M0",
        "description": "Native Baseline",
        "wall_time_ms": m0_elapsed_ms,
        "input_tokens": m0_total_in,
        "cached_tokens": m0_total_c,
        "cache_hit_ratio": if m0_total_in > 0 { m0_total_c as f64 / m0_total_in as f64 } else { 0.0 },
        "output_tokens": m0_total_out,
        "provider_cost_usd": m0_cost,
        "acceptance_passed": m0_acceptance,
    }));

    // ------------------------------------------------------------------------
    // Mode M1: ContextWatcher Passthrough Control
    // ------------------------------------------------------------------------
    println!("\n=== Executing Mode M1: Passthrough Control ===");
    let m1_fixture = F1Fixture::new();
    let m1_home = std::env::temp_dir().join(format!("codex_home_m1_{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&m1_home).unwrap();
    let m1_res = spend_guard
        .reserve_session()
        .expect("M1 admission reservation");

    let store_m1 = CwStore::open_in_memory().unwrap();
    let mut state_m1 = DaemonState::new(store_m1.clone());
    state_m1.openai_upstream = "https://api.openai.com".to_string();
    state_m1.capture_wire_egress = true;
    let token_m1 = state_m1.control_token.clone();
    let sid_m1 = format!("cw119_pilot_m1_{}", uuid::Uuid::new_v4());

    let app_m1 = build_proxy_router(state_m1);
    let listener_m1 = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_port_m1 = listener_m1.local_addr().unwrap().port();
    let proxy_task_m1 = tokio::spawn(async move {
        let _ = axum::serve(listener_m1, app_m1).await;
    });

    let m1_start = Instant::now();

    // Turn 1
    let mut cmd_m1_t1 = Command::new(&codex_bin);
    cmd_m1_t1.current_dir(&m1_fixture.workspace_dir);
    cmd_m1_t1.args([
        "exec",
        "--json",
        "--skip-git-repo-check",
        "--ignore-user-config",
        "-c",
        "approval_policy=\"never\"",
        "-c",
        "sandbox_mode=\"workspace-write\"",
    ]);
    cmd_m1_t1.args(codex_proxy_config_args(proxy_port_m1));
    cmd_m1_t1.args(["-m", model, m1_fixture.turn1_prompt()]);
    cmd_m1_t1.stdin(std::process::Stdio::null());
    cmd_m1_t1.env("OPENAI_API_KEY", &api_key);
    cmd_m1_t1.env("CODEX_HOME", &m1_home);
    cmd_m1_t1.env("CW_SESSION_ID", &sid_m1);
    cmd_m1_t1.env("CW_CONTROL_TOKEN", &token_m1);

    let out_m1_t1 = run_codex_blocking(cmd_m1_t1).await;
    let stdout_m1_t1 = String::from_utf8_lossy(&out_m1_t1.stdout);
    assert!(
        out_m1_t1.status.success(),
        "M1 Turn 1 failed: {}",
        String::from_utf8_lossy(&out_m1_t1.stderr)
    );
    let tid_m1 = extract_thread_id(&stdout_m1_t1).expect("M1 Turn 1 thread_id");
    let (in_m1_t1, c_m1_t1, out_m1_t1_tok) =
        extract_turn_completed_usage(&stdout_m1_t1).expect("M1 Turn 1 usage");

    // Turn 2
    let mut cmd_m1_t2 = Command::new(&codex_bin);
    cmd_m1_t2.current_dir(&m1_fixture.workspace_dir);
    cmd_m1_t2.args([
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
    cmd_m1_t2.args(codex_proxy_config_args(proxy_port_m1));
    cmd_m1_t2.args(["-m", model, &tid_m1, m1_fixture.turn2_prompt()]);
    cmd_m1_t2.stdin(std::process::Stdio::null());
    cmd_m1_t2.env("OPENAI_API_KEY", &api_key);
    cmd_m1_t2.env("CODEX_HOME", &m1_home);
    cmd_m1_t2.env("CW_SESSION_ID", &sid_m1);
    cmd_m1_t2.env("CW_CONTROL_TOKEN", &token_m1);

    let out_m1_t2 = run_codex_blocking(cmd_m1_t2).await;
    let stdout_m1_t2 = String::from_utf8_lossy(&out_m1_t2.stdout);
    assert!(
        out_m1_t2.status.success(),
        "M1 Turn 2 failed: {}",
        String::from_utf8_lossy(&out_m1_t2.stderr)
    );
    let (in_m1_t2, c_m1_t2, out_m1_t2_tok) =
        extract_turn_completed_usage(&stdout_m1_t2).expect("M1 Turn 2 usage");

    let m1_elapsed_ms = m1_start.elapsed().as_millis() as u64;
    let m1_acceptance = m1_fixture.run_acceptance_test();
    assert!(m1_acceptance, "M1 acceptance tests must pass");
    proxy_task_m1.abort();

    let m1_total_in = in_m1_t1 + in_m1_t2;
    let m1_total_c = c_m1_t1 + c_m1_t2;
    let m1_total_out = out_m1_t1_tok + out_m1_t2_tok;
    let m1_cost = spend_guard
        .record_usage(
            m1_total_in - m1_total_c,
            m1_total_c,
            m1_total_out,
            ProviderKind::OpenAI,
            model,
        )
        .expect("M1 record usage");
    m1_res.commit(0.0).expect("Commit M1 reservation");
    let _ = fs::remove_dir_all(&m1_home);

    println!(
        "M1 Completed: WallTime={}ms, In={}, Cached={}, Out={}, Cost=${:.5}, Acceptance={}",
        m1_elapsed_ms, m1_total_in, m1_total_c, m1_total_out, m1_cost, m1_acceptance
    );

    session_results.push(json!({
        "mode": "M1",
        "description": "Passthrough Control",
        "wall_time_ms": m1_elapsed_ms,
        "input_tokens": m1_total_in,
        "cached_tokens": m1_total_c,
        "cache_hit_ratio": if m1_total_in > 0 { m1_total_c as f64 / m1_total_in as f64 } else { 0.0 },
        "output_tokens": m1_total_out,
        "provider_cost_usd": m1_cost,
        "acceptance_passed": m1_acceptance,
    }));

    // ------------------------------------------------------------------------
    // Mode M2: ContextWatcher Cache-Safe Optimization (Tail Pin Directive)
    // ------------------------------------------------------------------------
    println!("\n=== Executing Mode M2: Cache-Safe Optimization ===");
    let m2_fixture = F1Fixture::new();
    let m2_home = std::env::temp_dir().join(format!("codex_home_m2_{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&m2_home).unwrap();
    let m2_res = spend_guard
        .reserve_session()
        .expect("M2 admission reservation");

    let store_m2 = CwStore::open_in_memory().unwrap();
    let mut state_m2 = DaemonState::new(store_m2.clone());
    state_m2.openai_upstream = "https://api.openai.com".to_string();
    state_m2.capture_wire_egress = true;
    let token_m2 = state_m2.control_token.clone();
    let sid_m2 = format!("cw119_pilot_m2_{}", uuid::Uuid::new_v4());

    let app_m2 = build_proxy_router(state_m2);
    let listener_m2 = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_port_m2 = listener_m2.local_addr().unwrap().port();
    let proxy_task_m2 = tokio::spawn(async move {
        let _ = axum::serve(listener_m2, app_m2).await;
    });

    let m2_start = Instant::now();

    // Turn 1 (unmutated baseline)
    let mut cmd_m2_t1 = Command::new(&codex_bin);
    cmd_m2_t1.current_dir(&m2_fixture.workspace_dir);
    cmd_m2_t1.args([
        "exec",
        "--json",
        "--skip-git-repo-check",
        "--ignore-user-config",
        "-c",
        "approval_policy=\"never\"",
        "-c",
        "sandbox_mode=\"workspace-write\"",
    ]);
    cmd_m2_t1.args(codex_proxy_config_args(proxy_port_m2));
    cmd_m2_t1.args(["-m", model, m2_fixture.turn1_prompt()]);
    cmd_m2_t1.stdin(std::process::Stdio::null());
    cmd_m2_t1.env("OPENAI_API_KEY", &api_key);
    cmd_m2_t1.env("CODEX_HOME", &m2_home);
    cmd_m2_t1.env("CW_SESSION_ID", &sid_m2);
    cmd_m2_t1.env("CW_CONTROL_TOKEN", &token_m2);

    let out_m2_t1 = run_codex_blocking(cmd_m2_t1).await;
    let stdout_m2_t1 = String::from_utf8_lossy(&out_m2_t1.stdout);
    assert!(
        out_m2_t1.status.success(),
        "M2 Turn 1 failed: {}",
        String::from_utf8_lossy(&out_m2_t1.stderr)
    );
    let tid_m2 = extract_thread_id(&stdout_m2_t1).expect("M2 Turn 1 thread_id");
    let (in_m2_t1, c_m2_t1, out_m2_t1_tok) =
        extract_turn_completed_usage(&stdout_m2_t1).expect("M2 Turn 1 usage");

    // Commit cache-safe tail directive mutation before Turn 2
    store_m2
        .save_mutation(&MutationRule {
            id: format!("live_pin_directive_{}", sid_m2),
            session_id: sid_m2.clone(),
            op: MutationOp::PinContext {
                label: "ActiveDirective".to_string(),
                content: "OPTIMIZATION_ACTIVE: keep helper signatures concise and clean."
                    .to_string(),
                source_node_id: None,
            },
            status: MutationStatus::Committed,
            estimated_saved_tokens: 0,
            estimated_kv_break_tokens: 0,
            created_at: Utc::now(),
        })
        .expect("Save M2 PinContext mutation");

    // Turn 2: Resume with active overlay
    let mut cmd_m2_t2 = Command::new(&codex_bin);
    cmd_m2_t2.current_dir(&m2_fixture.workspace_dir);
    cmd_m2_t2.args([
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
    cmd_m2_t2.args(codex_proxy_config_args(proxy_port_m2));
    cmd_m2_t2.args(["-m", model, &tid_m2, m2_fixture.turn2_prompt()]);
    cmd_m2_t2.stdin(std::process::Stdio::null());
    cmd_m2_t2.env("OPENAI_API_KEY", &api_key);
    cmd_m2_t2.env("CODEX_HOME", &m2_home);
    cmd_m2_t2.env("CW_SESSION_ID", &sid_m2);
    cmd_m2_t2.env("CW_CONTROL_TOKEN", &token_m2);

    let out_m2_t2 = run_codex_blocking(cmd_m2_t2).await;
    let stdout_m2_t2 = String::from_utf8_lossy(&out_m2_t2.stdout);
    assert!(
        out_m2_t2.status.success(),
        "M2 Turn 2 failed: {}",
        String::from_utf8_lossy(&out_m2_t2.stderr)
    );
    let (in_m2_t2, c_m2_t2, out_m2_t2_tok) =
        extract_turn_completed_usage(&stdout_m2_t2).expect("M2 Turn 2 usage");

    let m2_elapsed_ms = m2_start.elapsed().as_millis() as u64;
    let m2_acceptance = m2_fixture.run_acceptance_test();
    assert!(m2_acceptance, "M2 acceptance tests must pass");
    proxy_task_m2.abort();

    let m2_total_in = in_m2_t1 + in_m2_t2;
    let m2_total_c = c_m2_t1 + c_m2_t2;
    let m2_total_out = out_m2_t1_tok + out_m2_t2_tok;
    let m2_cost = spend_guard
        .record_usage(
            m2_total_in - m2_total_c,
            m2_total_c,
            m2_total_out,
            ProviderKind::OpenAI,
            model,
        )
        .expect("M2 record usage");
    m2_res.commit(0.0).expect("Commit M2 reservation");
    let _ = fs::remove_dir_all(&m2_home);

    println!(
        "M2 Completed: WallTime={}ms, In={}, Cached={}, Out={}, Cost=${:.5}, Acceptance={}",
        m2_elapsed_ms, m2_total_in, m2_total_c, m2_total_out, m2_cost, m2_acceptance
    );

    session_results.push(json!({
        "mode": "M2",
        "description": "Cache-Safe Optimization",
        "wall_time_ms": m2_elapsed_ms,
        "input_tokens": m2_total_in,
        "cached_tokens": m2_total_c,
        "cache_hit_ratio": if m2_total_in > 0 { m2_total_c as f64 / m2_total_in as f64 } else { 0.0 },
        "output_tokens": m2_total_out,
        "provider_cost_usd": m2_cost,
        "acceptance_passed": m2_acceptance,
    }));

    // Output dataset
    let report_payload = json!({
        "milestone": "CW-119",
        "title": "Controlled Live-Agent Product Value Qualification Pilot",
        "timestamp": Utc::now().to_rfc3339(),
        "provider": "OpenAI",
        "model": model,
        "harness": "Codex CLI 0.166.0",
        "workload": "F1BugFixing",
        "total_campaign_spend_usd": spend_guard.total_spent_usd(),
        "spend_envelope_usd": auth_usd,
        "sessions": session_results,
    });

    let out_dir = workspace_root().join("docs");
    let out_path = out_dir.join("cw119_pilot_benchmark_data.json");
    let _ = fs::write(
        &out_path,
        serde_json::to_string_pretty(&report_payload).unwrap(),
    );
    println!("\nSaved pilot benchmark dataset to: {}", out_path.display());
    println!(
        "Total Campaign Spend: ${:.5} USD (Limit: ${:.2} USD)",
        spend_guard.total_spent_usd(),
        auth_usd
    );
}
