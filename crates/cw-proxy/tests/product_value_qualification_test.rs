// crates/cw-proxy/tests/product_value_qualification_test.rs
//
// CW-118: Product Value & Economic Qualification Tests
//
// Rigorous deterministic tests exercising REAL production components:
//   1. Randomized balanced scheduling (Latin square mode-order rotation across 45 sessions)
//   2. Identical initial fixture hashes across modes (independent acceptance verification)
//   3. Result persistence, configuration validation, and resume idempotency
//   4. Missing/incompatible provider usage handling (fail-closed on unknown model pricing)
//   5. Correct cached vs uncached billing accuracy (real ModelPricingProfile)
//   6. Failed-task cost accounting (production analyze_campaign_results all-attempt vs success-conditional)
//   7. Matched and unmatched experimental blocks (production analyze_campaign_results)
//   8. Zero-cost and missing-baseline division safety (production analyze_campaign_results)
//   9. Abstention when policy cannot safely optimize (real cw_core::policy::evaluate_decision)
//  10. Zero unauthorized paid execution (Gate B spend authorization and unavailability guards)
//  11. Budget exhaustion and campaign cancellation (real runner --budget-limit)
//  12. Payload, signature, transcript and tool-lifecycle preservation (real normalizer & policy)
//  13. Stable regenerated report output (identical report generation)
//  14. Bounded real mock-provider proxy execution (M0 native vs M1 passthrough vs M2 active mutation)
//  15. Cache-safe policy engine qualification (defer on warm cache vs tail-only PinContext application)

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use axum::{Router, extract::Json as AxumJson, routing::post};
use chrono::Utc;
use cw_core::{
    analyze_turn_from_payload,
    ir::*,
    policy::{
        CacheState, DecisionReasonCode, OptimizationDecision, classify_cache_state,
        compute_prefix_fingerprint, evaluate_decision, find_pricing_profile,
    },
};
use cw_proxy::{DaemonState, build_proxy_router};
use cw_store::CwStore;
use serde::{Deserialize, Serialize};
use serde_json::json;
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
// 1. Data Structures matching CW-118 Specification
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TaskFamily {
    F1,
    F2,
    F3,
    F4,
    F5,
}

impl TaskFamily {
    pub fn all() -> &'static [TaskFamily] {
        &[
            TaskFamily::F1,
            TaskFamily::F2,
            TaskFamily::F3,
            TaskFamily::F4,
            TaskFamily::F5,
        ]
    }

    pub fn code(&self) -> &'static str {
        match self {
            Self::F1 => "F1",
            Self::F2 => "F2",
            Self::F3 => "F3",
            Self::F4 => "F4",
            Self::F5 => "F5",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ExecutionMode {
    M0,
    M1,
    M2,
}

impl ExecutionMode {
    pub fn all() -> &'static [ExecutionMode] {
        &[ExecutionMode::M0, ExecutionMode::M1, ExecutionMode::M2]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledSession {
    pub campaign_id: String,
    pub block_id: String,
    pub session_id: String,
    pub task_family: TaskFamily,
    pub repetition: usize,
    pub mode: ExecutionMode,
    pub execution_order_position: usize,
    pub global_schedule_index: usize,
    pub seed: u64,
}

/// Generate Latin-square balanced block schedule
pub fn generate_schedule(
    tasks: &[TaskFamily],
    repetitions: usize,
    modes: &[ExecutionMode],
    seed: u64,
) -> Vec<ScheduledSession> {
    let mut schedule = Vec::new();
    let mut index = 0;

    let mut ordered_tasks = tasks.to_vec();
    // Deterministic shuffle driven by seed
    let mut state = seed.wrapping_add(1442695040888963407);
    for i in (1..ordered_tasks.len()).rev() {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let j = (state >> 33) as usize % (i + 1);
        ordered_tasks.swap(i, j);
    }

    for &task in &ordered_tasks {
        for rep in 0..repetitions {
            let block_id = format!("block_{}_rep{}", task.code(), rep);
            let offset = rep % modes.len();
            for pos in 0..modes.len() {
                let mode = modes[(pos + offset) % modes.len()];
                let mode_str = match mode {
                    ExecutionMode::M0 => "m0",
                    ExecutionMode::M1 => "m1",
                    ExecutionMode::M2 => "m2",
                };
                let session_id = format!(
                    "cw118_{}_rep{}_{}",
                    task.code().to_lowercase(),
                    rep,
                    mode_str
                );
                schedule.push(ScheduledSession {
                    campaign_id: "cw118_product_value".to_string(),
                    block_id: block_id.clone(),
                    session_id,
                    task_family: task,
                    repetition: rep,
                    mode,
                    execution_order_position: pos,
                    global_schedule_index: index,
                    seed,
                });
                index += 1;
            }
        }
    }
    schedule
}

// ============================================================================
// Production Integration Tests
// ============================================================================

#[test]
fn test_cw118_randomized_balanced_schedule() {
    let tasks = TaskFamily::all();
    let modes = ExecutionMode::all();
    let reps = 3;
    let schedule = generate_schedule(tasks, reps, modes, 42);

    // 5 task families * 3 repetitions * 3 modes = 45 sessions
    assert_eq!(schedule.len(), 45);

    // Verify counterbalancing: across all 3 repetitions of each task,
    // each mode must appear exactly once in position 0, once in position 1, and once in position 2
    for &task in tasks {
        let task_sessions: Vec<_> = schedule.iter().filter(|s| s.task_family == task).collect();
        assert_eq!(task_sessions.len(), 9);

        for &mode in modes {
            let mut positions = Vec::new();
            for s in &task_sessions {
                if s.mode == mode {
                    positions.push(s.execution_order_position);
                }
            }
            positions.sort();
            assert_eq!(
                positions,
                vec![0, 1, 2],
                "Mode {:?} in task {:?} must be balanced across positions [0, 1, 2]",
                mode,
                task
            );
        }
    }

    // Verify that seed actively permutes task block sequencing
    let schedule2 = generate_schedule(tasks, reps, modes, 99);
    assert_eq!(schedule2.len(), 45);
    let order1: Vec<_> = schedule.iter().map(|s| s.task_family).collect();
    let order2: Vec<_> = schedule2.iter().map(|s| s.task_family).collect();
    assert_ne!(
        order1, order2,
        "Different seeds must produce different task block sequencing"
    );
}

#[test]
fn test_cw120a_offline_schedule_and_report_are_reproducible_without_provider_calls() {
    let root = workspace_root();
    let temp_dir = std::env::temp_dir().join(format!("cw120a_readiness_{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp_dir).unwrap();
    let report_path = temp_dir.join("report.json");
    let output = Command::new("python3")
        .current_dir(&root)
        .args([
            "-O",
            "benchmark/cw120a_offline_readiness.py",
            "--output",
            report_path.to_str().unwrap(),
        ])
        .output()
        .expect("failed to run CW-120A offline readiness rehearsal");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(&report_path).unwrap()).unwrap();
    assert_eq!(report["provider_calls"], 0);
    assert_eq!(report["codex_sessions"], 0);
    assert_eq!(report["proxy_mutations"], 0);
    assert_eq!(report["actual_provider_spend_usd"], 0.0);
    assert_eq!(report["synthetic_costs_are_not_product_evidence"], true);
    assert_eq!(report["evidence_scope"], "SYNTHETIC_REHEARSAL_ONLY");
    assert_eq!(report["analysis"]["matched_pairs_m1_vs_m0"], 0);
    assert_eq!(
        report["analysis"]["economic_summary"]["mean_proxy_overhead_ms"],
        serde_json::Value::Null
    );
    assert_eq!(
        report["analysis"]["causal_decomposition"]["proxy_overhead_effect"],
        "NOT_EVALUATED: requires matched M0/M1 sessions"
    );
    let criteria = &report["analysis"]["pre_registered_criteria"];
    assert_eq!(
        criteria["c1_no_correctness_regression"]["status"],
        "SIMULATION_ONLY"
    );
    assert_eq!(
        criteria["c5_no_cache_prefix_invalidation"]["status"],
        "SIMULATION_ONLY"
    );
    assert_eq!(
        criteria["c6_no_reliability_regression"]["status"],
        "NOT_EVALUATED"
    );
    assert_eq!(
        criteria["c7_attributed_to_documented_intervention"]["status"],
        "SIMULATION_ONLY"
    );
    assert!(
        report["analysis"]["causal_decomposition"]["cache_effect"]
            .as_str()
            .unwrap()
            .starts_with("SIMULATION_ONLY:")
    );
    assert!(
        report["analysis"]["causal_decomposition"]["behavioral_steering_effect"]
            .as_str()
            .unwrap()
            .starts_with("SIMULATION_ONLY:")
    );
    assert_eq!(
        report["analysis"]["paired_comparisons"][0]["behavioral_steering"],
        serde_json::Value::Null
    );
    assert_eq!(
        report["analysis"]["paired_comparisons"][0]["behavioral_steering_status"],
        "SIMULATION_ONLY"
    );
    let simulated_m2 = report["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|session| session["mode"] == "M2")
        .unwrap();
    assert_eq!(
        simulated_m2["execution_outcome"]["task_success"],
        serde_json::Value::Null
    );
    assert_eq!(
        simulated_m2["execution_outcome"]["acceptance_test_passed"],
        true
    );
    assert_eq!(
        simulated_m2["execution_outcome"]["acceptance_scope"],
        "predefined_solution_fixture_tests_only; no agent-generated task solution"
    );
    assert_eq!(
        simulated_m2["optimization_evidence"]["evidence_status"],
        "NOT_EVALUATED"
    );
    assert_eq!(
        simulated_m2["optimization_evidence"]["mutation_count"],
        serde_json::Value::Null
    );
    assert_eq!(
        simulated_m2["optimization_evidence"]["egress_verified"],
        false
    );
    assert_eq!(
        simulated_m2["simulated_optimization_evidence"]["mutation_count"],
        1
    );
    assert_eq!(
        simulated_m2["simulated_optimization_evidence"]["applied_to_request"],
        false
    );
    assert_eq!(
        report["independent_mock_test_references"][0]["test"],
        "test_cw118_real_mock_provider_proxy_execution_m0_m1_m2"
    );
    assert_eq!(
        report["independent_mock_test_references"][1]["test"],
        "test_cw118_cache_safe_policy_qualification_defer_and_pin"
    );
    assert_eq!(report["schedule"].as_array().unwrap().len(), 12);
    assert_eq!(report["sessions"].as_array().unwrap().len(), 12);
    assert_eq!(
        report["analysis"]["paired_comparisons"]
            .as_array()
            .unwrap()
            .len(),
        6
    );
    assert_eq!(report["analysis"]["total_sessions"], 12);
    assert_eq!(
        report["analysis"]["accounting"]["success_attempt_total_cost_usd"],
        serde_json::Value::Null
    );
    let first_schedule = report["schedule"].clone();
    let invalid_schedule = Command::new("python3")
        .current_dir(&root)
        .args([
            "-O",
            "-c",
            "import sys; sys.path.insert(0, 'benchmark'); from cw120a_offline_readiness import build_schedule, validate_schedule; schedule = build_schedule(); schedule.pop(); validate_schedule(schedule)",
        ])
        .output()
        .expect("failed to check optimized CW-120A schedule validation");
    assert!(
        !invalid_schedule.status.success(),
        "invalid schedule must fail validation under python -O"
    );

    let failed_acceptance = Command::new("python3")
        .current_dir(&root)
        .args([
            "-O",
            "-c",
            r#"
import sys
sys.path.insert(0, 'benchmark')
import cw118_product_value as cw118
item = {"campaign_id":"test", "block_id":"F1-R1", "session_id":"test", "task_family":"F1", "repetition":0, "mode":"M1", "execution_order_position":0, "seed":1202026}
def verify(responses, expected):
    results = iter(responses)
    cw118.run_acceptance_tests = lambda *_: next(results)
    try:
        cw118.simulate_offline_session(item)
    except RuntimeError as error:
        if expected not in str(error):
            raise
    else:
        raise SystemExit(f"acceptance failure was not detected: {expected}")
verify([(True, "unexpected initial pass")], "must fail acceptance before fix")
verify([(False, "buggy fixture"), (False, "still failing")], "Acceptance tests failed")
"#,
        ])
        .output()
        .expect("failed to test optimized acceptance validation");
    assert!(
        failed_acceptance.status.success(),
        "simulated acceptance failures must be rejected under python -O: {}",
        String::from_utf8_lossy(&failed_acceptance.stderr)
    );

    let rerun = Command::new("python3")
        .current_dir(&root)
        .args([
            "-O",
            "benchmark/cw120a_offline_readiness.py",
            "--output",
            report_path.to_str().unwrap(),
        ])
        .output()
        .expect("failed to rerun CW-120A offline readiness rehearsal");
    assert!(
        rerun.status.success(),
        "optimized readiness rerun failed: {}",
        String::from_utf8_lossy(&rerun.stderr)
    );
    let rerun_report: serde_json::Value =
        serde_json::from_slice(&fs::read(&report_path).unwrap()).unwrap();
    assert_eq!(rerun_report["schedule"], first_schedule);
    fs::remove_dir_all(temp_dir).unwrap();
}

#[test]
fn test_cw118_identical_initial_fixture_hashes() {
    let root = workspace_root();
    let temp_dir = std::env::temp_dir().join(format!("cw118_fixture_{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp_dir).unwrap();
    let report_path = temp_dir.join("fixture_report.json");
    let doc_path = temp_dir.join("fixture_report.md");

    let output = Command::new("python3")
        .current_dir(&root)
        .args([
            "benchmark/cw118_product_value.py",
            "--task",
            "f1",
            "--repetitions",
            "1",
            "--seed",
            "42",
            "--output",
            report_path.to_str().unwrap(),
            "--doc-output",
            doc_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute python runner");

    assert!(output.status.success());
    assert!(report_path.exists());

    let content = fs::read_to_string(&report_path).unwrap();
    let json_data: serde_json::Value = serde_json::from_str(&content).unwrap();

    let sessions = json_data["sessions"].as_array().unwrap();
    assert!(!sessions.is_empty());

    // Fixture hash must be non-empty and consistent across all modes in the block
    let first_hash = sessions[0]["fixture_hash"].as_str().unwrap();
    assert!(!first_hash.is_empty());

    for s in sessions {
        assert_eq!(
            s["fixture_hash"].as_str().unwrap(),
            first_hash,
            "Initial fixture hash must be identical across experimental arms"
        );
    }

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_cw118_result_persistence_and_resume_idempotency() {
    let root = workspace_root();
    let temp_dir = std::env::temp_dir().join(format!("cw118_persist_{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp_dir).unwrap();
    let result_file = temp_dir.join("report.json");
    let doc_file = temp_dir.join("report.md");

    // Run 1: generate 3 sessions for task F1
    let output1 = Command::new("python3")
        .current_dir(&root)
        .args([
            "benchmark/cw118_product_value.py",
            "--task",
            "f1",
            "--repetitions",
            "1",
            "--output",
            result_file.to_str().unwrap(),
            "--doc-output",
            doc_file.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute python runner");
    assert!(output1.status.success());
    let stdout1 = String::from_utf8_lossy(&output1.stdout);
    assert!(stdout1.contains("0 resumed"));

    let content1 = fs::read_to_string(&result_file).unwrap();
    let json1: serde_json::Value = serde_json::from_str(&content1).unwrap();
    let sessions1 = json1["sessions"].as_array().unwrap();
    assert_eq!(sessions1.len(), 3);

    // Run 2: resume with task F1 and F2 (6 sessions total, 3 resumed from previous run)
    let output2 = Command::new("python3")
        .current_dir(&root)
        .args([
            "benchmark/cw118_product_value.py",
            "--task",
            "f1,f2",
            "--repetitions",
            "1",
            "--output",
            result_file.to_str().unwrap(),
            "--doc-output",
            doc_file.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute python runner");
    assert!(output2.status.success());
    let stdout2 = String::from_utf8_lossy(&output2.stdout);
    assert!(
        stdout2.contains("3 resumed"),
        "Runner must resume existing 3 sessions without re-executing"
    );

    let content2 = fs::read_to_string(&result_file).unwrap();
    let json2: serde_json::Value = serde_json::from_str(&content2).unwrap();
    let sessions2 = json2["sessions"].as_array().unwrap();
    assert_eq!(sessions2.len(), 6);

    // Run 3: repeat exact same schedule (all 6 resumed)
    let output3 = Command::new("python3")
        .current_dir(&root)
        .args([
            "benchmark/cw118_product_value.py",
            "--task",
            "f1,f2",
            "--repetitions",
            "1",
            "--output",
            result_file.to_str().unwrap(),
            "--doc-output",
            doc_file.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute python runner");
    assert!(output3.status.success());
    let stdout3 = String::from_utf8_lossy(&output3.stdout);
    assert!(
        stdout3.contains("6 resumed"),
        "Runner must resume all 6 sessions idempotently"
    );

    let content3 = fs::read_to_string(&result_file).unwrap();
    let json3: serde_json::Value = serde_json::from_str(&content3).unwrap();
    assert_eq!(json3["sessions"].as_array().unwrap().len(), 6);

    // Run 4: re-run on same output file but with differing model configuration (--model gpt-4o)
    // Configuration validation must detect model incompatibility and reject reuse!
    let output4 = Command::new("python3")
        .current_dir(&root)
        .args([
            "benchmark/cw118_product_value.py",
            "--task",
            "f1,f2",
            "--repetitions",
            "1",
            "--model",
            "gpt-4o",
            "--output",
            result_file.to_str().unwrap(),
            "--doc-output",
            doc_file.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute python runner");
    assert!(output4.status.success());
    let stdout4 = String::from_utf8_lossy(&output4.stdout);
    assert!(
        stdout4.contains("0 resumed"),
        "Runner must reject cached sessions when model configuration changes"
    );

    let content4 = fs::read_to_string(&result_file).unwrap();
    let json4: serde_json::Value = serde_json::from_str(&content4).unwrap();
    let sessions4 = json4["sessions"].as_array().unwrap();
    assert_eq!(sessions4.len(), 6);
    assert_eq!(sessions4[0]["model"], "gpt-4o");

    // Run 5: re-run with differing seed (--seed 99 vs 42)
    // Configuration validation must detect seed incompatibility and reject reuse!
    let output5 = Command::new("python3")
        .current_dir(&root)
        .args([
            "benchmark/cw118_product_value.py",
            "--task",
            "f1,f2",
            "--repetitions",
            "1",
            "--model",
            "gpt-4o",
            "--seed",
            "99",
            "--output",
            result_file.to_str().unwrap(),
            "--doc-output",
            doc_file.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute python runner");
    assert!(output5.status.success());
    let stdout5 = String::from_utf8_lossy(&output5.stdout);
    assert!(
        stdout5.contains("0 resumed"),
        "Runner must reject cached sessions when randomization seed changes"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_cw118_missing_incompatible_telemetry_handling() {
    let root = workspace_root();
    // Test that the runner fail-closes when given an unknown or unverified model pricing profile
    let script = "import sys; sys.path.insert(0, 'benchmark'); from cw118_product_value import calculate_provider_cost; res = calculate_provider_cost('unknown-unsupported-model', 1000, 500, 100); assert res is None, f'Expected None for unknown model, got {res}'";

    let output = Command::new("python3")
        .current_dir(&root)
        .args(["-c", script])
        .output()
        .expect("Failed to execute python test");

    assert!(
        output.status.success(),
        "Unknown model pricing must return None fail-closed"
    );
}

#[test]
fn test_cw118_cached_vs_uncached_billing_accuracy() {
    let profile = find_pricing_profile(ProviderKind::OpenAI, "gpt-4o-mini").unwrap();

    // Verify rate values directly against production profile
    assert_eq!(profile.uncached_input_per_mtok, 0.150);
    assert_eq!(profile.cached_input_read_per_mtok, 0.075);
    assert_eq!(profile.output_per_mtok, 0.600);

    // Uncached: 1,000,000 tokens @ $0.15
    let uncached_cost = (1_000_000_f64 * profile.uncached_input_per_mtok) / 1_000_000.0;
    assert!((uncached_cost - 0.150).abs() < 1e-6);

    // Cached: 1,000,000 tokens @ $0.075 (50% discount)
    let cached_cost = (1_000_000_f64 * profile.cached_input_read_per_mtok) / 1_000_000.0;
    assert!((cached_cost - 0.075).abs() < 1e-6);
}

#[test]
fn test_cw120a_cache_write_tokens_are_priced_as_a_disjoint_input_bucket() {
    let root = workspace_root();
    let script = r#"
import sys
sys.path.insert(0, 'benchmark')
from cw118_product_value import calculate_provider_cost

input_tokens, cached_tokens, cache_write_tokens, output_tokens = 170, 50, 20, 10
uncached_tokens = input_tokens - cached_tokens - cache_write_tokens
cost = calculate_provider_cost(
    'gpt-6-luna', uncached_tokens, cached_tokens, output_tokens, cache_write_tokens
)
if cost != 0.000018:
    raise SystemExit(f'cache-write buckets double-counted or mispriced: {cost}')
"#;
    let output = Command::new("python3")
        .current_dir(&root)
        .args(["-O", "-c", script])
        .output()
        .expect("failed to test GPT-6 Luna cache-write accounting");
    assert!(
        output.status.success(),
        "cache-write tokens must be excluded from uncached tokens: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_cw118_failed_task_cost_accounting() {
    let root = workspace_root();
    // Test production analyze_campaign_results handles failed runs without dropping incurred costs
    let script = r#"
import sys, json
sys.path.insert(0, 'benchmark')
from cw118_product_value import analyze_campaign_results

sessions = [
    {
        "task_family": "F1", "repetition": 0, "mode": "M1",
        "execution_outcome": {"task_success": False, "model_request_count": 10},
        "provider_economics": {"calculated_cost_usd": 0.040, "total_input_tokens": 10000, "cached_tokens": 8000},
        "performance": {"proxy_overhead_ms": 12.0, "wall_clock_duration_ms": 18000},
        "optimization_evidence": {}
    },
    {
        "task_family": "F1", "repetition": 0, "mode": "M2",
        "execution_outcome": {"task_success": True, "model_request_count": 8},
        "provider_economics": {"calculated_cost_usd": 0.030, "total_input_tokens": 8000, "cached_tokens": 7000},
        "performance": {"proxy_overhead_ms": 14.0, "wall_clock_duration_ms": 15000},
        "optimization_evidence": {"behavioral_steering_injected": False}
    }
]

res = analyze_campaign_results(sessions)
acct = res["accounting"]
assert acct["all_attempt_total_cost_usd"] == 0.070, f"All-attempt cost must sum both: {acct['all_attempt_total_cost_usd']}"
assert acct["success_attempt_total_cost_usd"] == 0.030, f"Success cost must only include successful run: {acct['success_attempt_total_cost_usd']}"
"#;

    let output = Command::new("python3")
        .current_dir(&root)
        .args(["-c", script])
        .output()
        .expect("Failed to execute python test");

    assert!(
        output.status.success(),
        "Failed task accounting must separate all-attempt from success-conditional"
    );
}

#[test]
fn test_cw118_matched_and_unmatched_block_handling() {
    let root = workspace_root();
    // Verify production analyze_campaign_results handles unmatched blocks without panicking
    let script = r#"
import sys
sys.path.insert(0, 'benchmark')
from cw118_product_value import analyze_campaign_results

# Only M1 present, M2 missing
sessions = [
    {
        "task_family": "F1", "repetition": 0, "mode": "M1",
        "execution_outcome": {"task_success": True, "model_request_count": 10},
        "provider_economics": {"calculated_cost_usd": 0.040, "total_input_tokens": 10000, "cached_tokens": 8000},
        "performance": {"proxy_overhead_ms": 12.0, "wall_clock_duration_ms": 18000},
        "optimization_evidence": {}
    }
]

res = analyze_campaign_results(sessions)
assert res["matched_pairs_m2_vs_m1"] == 0, "Unmatched block must yield 0 pairs"
assert len(res["paired_comparisons"]) == 0
"#;

    let output = Command::new("python3")
        .current_dir(&root)
        .args(["-c", script])
        .output()
        .expect("Failed to execute python test");

    assert!(
        output.status.success(),
        "Unmatched blocks must be safely handled without pairing"
    );
}

#[test]
fn test_cw118_zero_cost_and_missing_baseline_division_safety() {
    let root = workspace_root();
    // Test division by zero safety when M1 cost is 0.0
    let script = r#"
import sys
sys.path.insert(0, 'benchmark')
from cw118_product_value import analyze_campaign_results

sessions = [
    {
        "task_family": "F1", "repetition": 0, "mode": "M1",
        "execution_outcome": {"task_success": True, "model_request_count": 0},
        "provider_economics": {"calculated_cost_usd": 0.0, "total_input_tokens": 0, "cached_tokens": 0},
        "performance": {"proxy_overhead_ms": 0.0, "wall_clock_duration_ms": 0},
        "optimization_evidence": {}
    },
    {
        "task_family": "F1", "repetition": 0, "mode": "M2",
        "execution_outcome": {"task_success": True, "model_request_count": 0},
        "provider_economics": {"calculated_cost_usd": 0.020, "total_input_tokens": 5000, "cached_tokens": 4000},
        "performance": {"proxy_overhead_ms": 10.0, "wall_clock_duration_ms": 5000},
        "optimization_evidence": {}
    }
]

res = analyze_campaign_results(sessions)
pair = res["paired_comparisons"][0]
assert pair["valid_division"] is False, "Zero cost baseline must set valid_division=False"
assert pair["relative_cost_change"] is None, "Zero cost baseline must return None relative change"
"#;

    let output = Command::new("python3")
        .current_dir(&root)
        .args(["-c", script])
        .output()
        .expect("Failed to execute python test");

    assert!(
        output.status.success(),
        "Zero cost baseline must avoid ZeroDivisionError"
    );
}

#[test]
fn test_cw118_abstention_when_policy_cannot_safely_optimize() {
    // Exercise REAL PRODUCTION cw_core::policy::evaluate_decision
    // When cache is warm and a mutation targets a historical tool result (breaking prefix),
    // the production policy MUST defer with EarlyHistoricalPruneOnWarmCacheDefers.
    let payload = json!({
        "model": "gpt-4o-mini",
        "messages": [
            { "role": "system", "content": "You are a coding assistant." },
            { "role": "user", "content": "Run tests and inspect src/math.rs" },
            {
                "role": "assistant",
                "content": null,
                "tool_calls": [
                    {
                        "id": "call_exec_1",
                        "type": "function",
                        "function": { "name": "exec_command", "arguments": "{\"command\":\"cargo test\"}" }
                    }
                ]
            },
            {
                "role": "tool",
                "tool_call_id": "call_exec_1",
                "content": "test output from 10 turns ago\n".repeat(30)
            },
            { "role": "assistant", "content": "Tests executed." },
            { "role": "user", "content": "Continue." }
        ]
    });

    let (turn, _) = analyze_turn_from_payload(
        "turn_warm_1".into(),
        "sess_warm_1".into(),
        None,
        2,
        &payload,
        ProviderKind::OpenAI,
        None,
        Some(1200),
        None,
    )
    .expect("failed to analyze synthetic turn");

    let prefix_fp = compute_prefix_fingerprint(&turn.nodes);
    let cache_obs = classify_cache_state(
        &turn.session_id,
        ProviderKind::OpenAI,
        "gpt-4o-mini",
        turn.sequence_number,
        Some(&prefix_fp),
        None,
        Some(&turn.kv_cache_report),
        false,
    );
    assert_eq!(cache_obs.state, CacheState::WarmVerified);

    let op = MutationOp::PruneOrStub {
        target_fingerprint: None,
        target_call_id: Some("call_exec_1".to_string()),
        target_json_pointer: None,
        stub_text: "[pruned]".to_string(),
    };

    let pricing = find_pricing_profile(ProviderKind::OpenAI, "gpt-4o-mini");
    let decision = evaluate_decision(
        Some("cand_prune_history"),
        None,
        &op,
        &turn,
        &cache_obs,
        pricing.as_ref(),
        3,
    );

    // Assert that the production policy explicitly defers to protect the warm cache prefix
    assert_eq!(decision.decision, OptimizationDecision::Defer);
    assert_eq!(decision.reason_code, DecisionReasonCode::CachePrefixAtRisk);
    assert!(decision.estimated_kv_break_tokens > 0);
}

#[test]
fn test_cw118_zero_unauthorized_paid_execution() {
    let root = workspace_root();

    // 1. Running benchmark with --live without explicit spend authorization must exit with code 2
    let output1 = Command::new("python3")
        .current_dir(&root)
        .args(["benchmark/cw118_product_value.py", "--live"])
        .output()
        .expect("Failed to execute python runner");

    assert_eq!(output1.status.code(), Some(2));
    let stderr1 = String::from_utf8_lossy(&output1.stderr);
    assert!(stderr1.contains("BLOCKED_SPEND_AUTHORIZATION"));

    // 2. Running benchmark with --live and --authorize-spend without live environment must exit with code 3 (unavailable)
    let output2 = Command::new("python3")
        .current_dir(&root)
        .args([
            "benchmark/cw118_product_value.py",
            "--live",
            "--authorize-spend",
            "5.0",
        ])
        .output()
        .expect("Failed to execute python runner");

    assert_eq!(output2.status.code(), Some(3));
    let stderr2 = String::from_utf8_lossy(&output2.stderr);
    assert!(stderr2.contains("BLOCKED_LIVE_EXECUTION_UNAVAILABLE"));
}

#[test]
fn test_cw118_budget_exhaustion_and_campaign_cancellation() {
    let root = workspace_root();
    let temp_dir = std::env::temp_dir().join(format!("cw118_budget_{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp_dir).unwrap();
    let report_file = temp_dir.join("budget_report.json");
    let doc_file = temp_dir.join("budget_report.md");

    // Run runner with an extremely low budget limit ($0.0001) so second session exhausts budget
    let output = Command::new("python3")
        .current_dir(&root)
        .args([
            "benchmark/cw118_product_value.py",
            "--task",
            "f1",
            "--repetitions",
            "1",
            "--budget-limit",
            "0.0001",
            "--output",
            report_file.to_str().unwrap(),
            "--doc-output",
            doc_file.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute python runner");

    assert!(output.status.success());
    let content = fs::read_to_string(&report_file).unwrap();
    let json_data: serde_json::Value = serde_json::from_str(&content).unwrap();

    let sessions = json_data["sessions"].as_array().unwrap();
    // At least one session must be canceled due to budget exhaustion
    let canceled = sessions
        .iter()
        .any(|s| s["execution_outcome"]["termination_reason"] == "canceled_budget_exhaustion");
    assert!(
        canceled,
        "Subsequent sessions must be canceled when budget envelope is exhausted"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_cw118_payload_signature_transcript_preservation() {
    // Exercise REAL PRODUCTION cw_core normalizer and policy to verify that
    // thinking signatures are preserved and protected from mutation
    let raw_payload = json!({
        "model": "claude-3-5-sonnet-20241022",
        "messages": [
            {
                "role": "user",
                "content": "Solve math problem"
            },
            {
                "role": "assistant",
                "content": [
                    {
                        "type": "thinking",
                        "thinking": "Calculating 2 + 2...",
                        "signature": "cryptographic_signature_byte_exact_abc123"
                    },
                    {
                        "type": "text",
                        "text": "The answer is 4."
                    }
                ]
            }
        ]
    });

    let (turn, _) = analyze_turn_from_payload(
        "turn_sig_1".into(),
        "sess_sig_1".into(),
        None,
        1,
        &raw_payload,
        ProviderKind::Anthropic,
        None,
        Some(300),
        None,
    )
    .expect("failed to parse turn");

    // Find the thinking node with signature
    let thinking_node = turn.nodes.iter().find(|n| {
        matches!(
            &n.kind,
            NodeKind::AssistantThinking {
                signature: Some(sig),
                ..
            } if sig == "cryptographic_signature_byte_exact_abc123"
        )
    });
    assert!(
        thinking_node.is_some(),
        "Cryptographic signature must be captured in canonical IR"
    );

    // Attempt to evaluate a mutation targeting the signed thinking block
    let node = thinking_node.unwrap();
    let op = MutationOp::PruneOrStub {
        target_fingerprint: Some(node.fingerprint.0.clone()),
        target_call_id: None,
        target_json_pointer: Some(node.raw_json_pointer.clone()),
        stub_text: "[pruned]".to_string(),
    };

    let prefix_fp = compute_prefix_fingerprint(&turn.nodes);
    let cache_obs = classify_cache_state(
        &turn.session_id,
        ProviderKind::Anthropic,
        "claude-3-5-sonnet-20241022",
        turn.sequence_number,
        Some(&prefix_fp),
        None,
        Some(&turn.kv_cache_report),
        false,
    );

    let pricing = find_pricing_profile(ProviderKind::Anthropic, "claude-3-5-sonnet-20241022");
    let decision = evaluate_decision(
        Some("cand_prune_signed"),
        None,
        &op,
        &turn,
        &cache_obs,
        pricing.as_ref(),
        3,
    );

    // Production policy must strictly reject mutations targeting signed thinking content
    assert_eq!(decision.decision, OptimizationDecision::Reject);
    assert_eq!(decision.reason_code, DecisionReasonCode::SafetyViolation);
    assert!(decision.explanation.contains("signed thinking"));
}

#[test]
fn test_cw118_stable_regenerated_report_output() {
    let root = workspace_root();
    let temp_dir = std::env::temp_dir().join(format!("cw118_stable_{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp_dir).unwrap();
    let rep1_path = temp_dir.join("rep1.json");
    let doc1_path = temp_dir.join("rep1.md");
    let rep2_path = temp_dir.join("rep2.json");
    let doc2_path = temp_dir.join("rep2.md");

    let out1 = Command::new("python3")
        .current_dir(&root)
        .args([
            "benchmark/cw118_product_value.py",
            "--task",
            "f1",
            "--repetitions",
            "1",
            "--seed",
            "42",
            "--output",
            rep1_path.to_str().unwrap(),
            "--doc-output",
            doc1_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute runner");
    assert!(out1.status.success());

    let out2 = Command::new("python3")
        .current_dir(&root)
        .args([
            "benchmark/cw118_product_value.py",
            "--task",
            "f1",
            "--repetitions",
            "1",
            "--seed",
            "42",
            "--output",
            rep2_path.to_str().unwrap(),
            "--doc-output",
            doc2_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute runner");
    assert!(out2.status.success());

    let content1 = fs::read_to_string(&rep1_path).unwrap();
    let content2 = fs::read_to_string(&rep2_path).unwrap();

    let json1: serde_json::Value = serde_json::from_str(&content1).unwrap();
    let json2: serde_json::Value = serde_json::from_str(&content2).unwrap();

    assert_eq!(
        json1["analysis"]["economic_summary"],
        json2["analysis"]["economic_summary"]
    );
    assert_eq!(
        json1["analysis"]["by_task_family"],
        json2["analysis"]["by_task_family"]
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

// ============================================================================
// 14. Bounded Real Mock-Provider Proxy Integration (M0 vs M1 vs M2)
// ============================================================================

async fn spawn_mock_openai_upstream() -> (
    String,
    Arc<AtomicUsize>,
    Arc<Mutex<Vec<(axum::http::HeaderMap, serde_json::Value)>>>,
    tokio::task::JoinHandle<()>,
) {
    let call_count = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let count_clone = call_count.clone();
    let requests_clone = requests.clone();
    let response_count = call_count.clone();
    let response_requests = requests.clone();

    let app = Router::new().route(
        "/v1/chat/completions",
        post(
            move |headers: axum::http::HeaderMap, AxumJson(payload): AxumJson<serde_json::Value>| {
                let count = count_clone.clone();
                let req_store = requests_clone.clone();
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    req_store
                        .lock()
                        .unwrap()
                        .push((headers.clone(), payload.clone()));

                    // Production Invariant: proxy must strip internal control credentials before forwarding
                    assert!(
                        headers.get("x-cw-control-token").is_none(),
                        "Proxy must strip internal control token before egress"
                    );

                    let payload_str = serde_json::to_string(&payload).unwrap_or_default();
                    let is_pruned = payload_str.contains("[pruned]");

                    let prompt_tokens = if is_pruned { 45 } else { 150 };
                    axum::Json(json!({
                        "id": if is_pruned { "chatcmpl-mock-m2" } else { "chatcmpl-mock-base" },
                        "object": "chat.completion",
                        "created": 1700000000,
                        "model": "gpt-4o-mini",
                        "choices": [{
                            "index": 0,
                            "message": {
                                "role": "assistant",
                                "content": if is_pruned { "Mutated context processed" } else { "Baseline context processed" }
                            },
                            "finish_reason": "stop"
                        }],
                        "usage": {
                            "prompt_tokens": prompt_tokens,
                            "completion_tokens": 15,
                            "total_tokens": prompt_tokens + 15,
                            "prompt_tokens_details": {
                                "cached_tokens": 0
                            }
                        }
                    }))
                }
            },
        ),
    )
    .route(
        "/v1/responses",
        post(
            move |headers: axum::http::HeaderMap, AxumJson(payload): AxumJson<serde_json::Value>| {
                let count = response_count.clone();
                let requests = response_requests.clone();
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    requests.lock().unwrap().push((headers, payload));
                    axum::Json(json!({
                        "id": "resp_mock_next",
                        "object": "response",
                        "status": "completed",
                        "output": [],
                        "usage": {"input_tokens": 10, "output_tokens": 1}
                    }))
                }
            },
        ),
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    (format!("http://{addr}"), call_count, requests, handle)
}

async fn spawn_real_cw_proxy(
    store: CwStore,
    upstream_url: String,
) -> (String, String, tokio::task::JoinHandle<()>) {
    let mut state = DaemonState::new(store);
    state.openai_upstream = upstream_url;
    state.capture_wire_egress = true;
    let control_token = state.control_token.clone();

    let router = build_proxy_router(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    (format!("http://{addr}"), control_token, handle)
}

#[tokio::test]
async fn stateful_responses_pin_reaches_mock_upstream_and_sqlite_egress() {
    let (mock_url, calls, received, _mock_task) = spawn_mock_openai_upstream().await;
    let store = CwStore::open_in_memory().unwrap();
    let (proxy_url, control_token, _proxy_task) =
        spawn_real_cw_proxy(store.clone(), mock_url).await;
    let session_id = "cw119b_stateful_responses";
    let now = Utc::now();
    store
        .upsert_session(&Session {
            id: session_id.into(),
            parent_session_id: None,
            harness: HarnessKind::GenericProxy,
            workspace_root: None,
            model: "gpt-4o-mini".into(),
            provider: ProviderKind::OpenAI,
            title: Some("CW-119B mock Responses test".into()),
            created_at: now,
            updated_at: now,
        })
        .unwrap();
    let mut autopilot = store.get_autopilot_config().unwrap_or_default();
    autopilot.enabled = true;
    store.set_autopilot_config(&autopilot).unwrap();
    let raw = json!({
        "model":"gpt-4o-mini",
        "previous_response_id":"resp_previous_exact",
        "input":[{"type":"message","role":"user","content":"before rule commit"}]
    });
    let precommit_response = reqwest::Client::new()
        .post(format!("{proxy_url}/openai/v1/responses"))
        .header("x-cw-session-id", session_id)
        .header("x-cw-control-token", &control_token)
        .json(&raw)
        .send()
        .await
        .unwrap();
    assert_eq!(precommit_response.status(), 200);
    assert_eq!(received.lock().unwrap()[0].1, raw);
    let precommit_turn = store
        .get_latest_turn(session_id)
        .unwrap()
        .expect("pre-commit request must persist");
    assert!(precommit_turn.applied_mutation_ids.is_empty());
    let precommit_egress: serde_json::Value = serde_json::from_slice(
        &store
            .get_wire_egress(&precommit_turn.id)
            .unwrap()
            .expect("pre-commit wire egress must persist"),
    )
    .unwrap();
    assert_eq!(precommit_egress, raw);

    store
        .save_mutation(&MutationRule {
            id: "cw119b_pin".into(),
            session_id: session_id.into(),
            op: MutationOp::PinContext {
                label: "qualification".into(),
                content: "preserve this directive".into(),
                source_node_id: None,
            },
            status: MutationStatus::Committed,
            estimated_saved_tokens: 0,
            estimated_kv_break_tokens: 0,
            created_at: now,
        })
        .unwrap();
    store
        .set_pin_context_policy(session_id, true, true)
        .unwrap();

    let raw = json!({
        "model":"gpt-4o-mini",
        "previous_response_id":"resp_previous_exact",
        "input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"finish the task"}]}]
    });
    let response = reqwest::Client::new()
        .post(format!("{proxy_url}/openai/v1/responses"))
        .header("x-cw-session-id", session_id)
        .header("x-cw-control-token", &control_token)
        .json(&raw)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(calls.load(Ordering::SeqCst), 2);

    let upstream = {
        let captured = received.lock().unwrap();
        assert_eq!(captured.len(), 2);
        let (headers, upstream) = &captured[1];
        assert!(headers.get("x-cw-control-token").is_none());
        assert!(headers.get("x-cw-session-id").is_none());
        assert_eq!(upstream["previous_response_id"], "resp_previous_exact");
        assert_eq!(
            upstream["input"][0]["content"][0],
            raw["input"][0]["content"][0]
        );
        assert_eq!(upstream["input"][0]["content"].as_array().unwrap().len(), 2);
        assert!(upstream.to_string().contains("preserve this directive"));
        let mut expected = raw.clone();
        expected["input"][0]["content"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "type":"input_text",
                "text":"\n<cw_pinned_context>\n- [qualification]: preserve this directive\n</cw_pinned_context>"
            }));
        assert_eq!(*upstream, expected);
        upstream.clone()
    };

    let turn = store
        .get_latest_turn(session_id)
        .unwrap()
        .expect("proxy turn must persist");
    assert!(
        turn.applied_mutation_ids
            .iter()
            .any(|id| id == "cw119b_pin")
    );
    let egress: serde_json::Value = serde_json::from_slice(
        &store
            .get_wire_egress(&turn.id)
            .unwrap()
            .expect("wire egress must persist"),
    )
    .unwrap();
    assert_eq!(egress, upstream);

    let no_type = json!({
        "model":"gpt-4o-mini",
        "previous_response_id":"resp_previous_exact",
        "input":[{"role":"user","content":"continue without explicit type"}]
    });
    let response = reqwest::Client::new()
        .post(format!("{proxy_url}/openai/v1/responses"))
        .header("x-cw-session-id", session_id)
        .header("x-cw-control-token", &control_token)
        .json(&no_type)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let no_type_upstream = received.lock().unwrap()[2].1.clone();
    assert_eq!(
        no_type_upstream["previous_response_id"],
        "resp_previous_exact"
    );
    assert!(
        no_type_upstream["input"][0]["content"]
            .as_str()
            .unwrap()
            .starts_with("continue without explicit type")
    );
    assert!(
        no_type_upstream
            .to_string()
            .contains("preserve this directive")
    );
    let mut expected_no_type = no_type.clone();
    expected_no_type["input"][0]["content"] = json!(
        "continue without explicit type\n<cw_pinned_context>\n- [qualification]: preserve this directive\n</cw_pinned_context>"
    );
    assert_eq!(no_type_upstream, expected_no_type);
    let no_type_turn = store
        .get_latest_turn(session_id)
        .unwrap()
        .expect("no-type user tail turn must persist");
    assert!(
        no_type_turn
            .applied_mutation_ids
            .iter()
            .any(|id| id == "cw119b_pin")
    );
    let no_type_egress: serde_json::Value = serde_json::from_slice(
        &store
            .get_wire_egress(&no_type_turn.id)
            .unwrap()
            .expect("no-type wire egress must persist"),
    )
    .unwrap();
    assert_eq!(no_type_egress, no_type_upstream);

    let tool_continuation = json!({
        "model":"gpt-4o-mini",
        "previous_response_id":"resp_previous_exact",
        "input":[{"type":"function_call_output","call_id":"call_tool","output":"tool result"}]
    });
    let tool_response = reqwest::Client::new()
        .post(format!("{proxy_url}/openai/v1/responses"))
        .header("x-cw-session-id", session_id)
        .header("x-cw-control-token", &control_token)
        .json(&tool_continuation)
        .send()
        .await
        .unwrap();
    assert_eq!(tool_response.status(), 200);
    assert_eq!(calls.load(Ordering::SeqCst), 4);
    let tool_turn = store
        .get_latest_turn(session_id)
        .unwrap()
        .expect("tool continuation turn must persist");
    assert!(tool_turn.applied_mutation_ids.is_empty());
    assert_eq!(tool_turn.skipped_mutations.len(), 1);
    assert_eq!(tool_turn.skipped_mutations[0].rule_id, "cw119b_pin");
    assert_eq!(
        tool_turn.skipped_mutations[0].reason,
        "SkippedNoEligibleUserTail"
    );
    let tool_egress: serde_json::Value = serde_json::from_slice(
        &store
            .get_wire_egress(&tool_turn.id)
            .unwrap()
            .expect("tool continuation egress must persist"),
    )
    .unwrap();
    assert_eq!(tool_egress, tool_continuation);

    let unauthenticated = json!({
        "model":"gpt-4o-mini",
        "previous_response_id":"resp_previous_exact",
        "input":[{"type":"message","role":"user","content":"unauthorized request"}]
    });
    let response = reqwest::Client::new()
        .post(format!("{proxy_url}/openai/v1/responses"))
        .header("x-cw-session-id", session_id)
        .json(&unauthenticated)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
    assert_eq!(calls.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn test_cw118_real_mock_provider_proxy_execution_m0_m1_m2() {
    let (mock_url, mock_calls, received_reqs, _mock_task) = spawn_mock_openai_upstream().await;
    let store = CwStore::open_in_memory().unwrap();
    let (proxy_url, control_token, _proxy_task) =
        spawn_real_cw_proxy(store.clone(), mock_url.clone()).await;

    let client = reqwest::Client::new();

    // Standard conversation payload with user prompt, assistant tool call, tool result, and continuation
    let raw_request_body = json!({
        "model": "gpt-4o-mini",
        "messages": [
            {
                "role": "user",
                "content": "Perform task F1"
            },
            {
                "role": "assistant",
                "content": null,
                "tool_calls": [
                    {
                        "id": "call_mock_1",
                        "type": "function",
                        "function": {
                            "name": "read_large_file",
                            "arguments": "{\"path\": \"src/lib.rs\"}"
                        }
                    }
                ]
            },
            {
                "role": "tool",
                "tool_call_id": "call_mock_1",
                "content": "VERY LONG RAW TOOL LOG OUTPUT THAT EXPANDS TOKEN CONSUMPTION 1234567890 1234567890 1234567890"
            },
            {
                "role": "assistant",
                "content": "I reviewed the tool output."
            },
            {
                "role": "user",
                "content": "What is the final answer?"
            }
        ]
    });

    // ------------------------------------------------------------------------
    // Mode M0: Native Baseline (Direct HTTP to mock provider, proxy bypassed)
    // ------------------------------------------------------------------------
    let t0_start = std::time::Instant::now();
    let m0_resp = client
        .post(format!("{mock_url}/v1/chat/completions"))
        .json(&raw_request_body)
        .send()
        .await
        .expect("M0 direct request failed");
    let m0_elapsed = t0_start.elapsed();

    assert_eq!(m0_resp.status(), 200);
    let m0_data: serde_json::Value = m0_resp.json().await.unwrap();
    assert_eq!(m0_data["usage"]["prompt_tokens"], 150);
    assert_eq!(mock_calls.load(Ordering::SeqCst), 1);

    // ------------------------------------------------------------------------
    // Mode M1: ContextWatcher Passthrough (Observation-only via proxy socket)
    // ------------------------------------------------------------------------
    let m1_session_id = "cw118_m1_passthrough_session";
    let t1_start = std::time::Instant::now();
    let m1_resp = client
        .post(format!("{proxy_url}/openai/v1/chat/completions"))
        .header("x-cw-session-id", m1_session_id)
        .header("x-cw-control-token", &control_token)
        .json(&raw_request_body)
        .send()
        .await
        .expect("M1 proxy request failed");
    let m1_elapsed = t1_start.elapsed();

    assert_eq!(m1_resp.status(), 200);
    let m1_data: serde_json::Value = m1_resp.json().await.unwrap();
    // In M1 observation mode, no mutation is applied; upstream saw full 150 prompt tokens
    assert_eq!(m1_data["usage"]["prompt_tokens"], 150);
    assert_eq!(mock_calls.load(Ordering::SeqCst), 2);

    // Verify lossless payload preservation between M0 and M1 (P2 finding):
    {
        let captured = received_reqs.lock().unwrap();
        assert_eq!(captured.len(), 2);
        let (m0_headers, m0_payload) = &captured[0];
        let (m1_headers, m1_payload) = &captured[1];

        // M0 and M1 payloads forwarded to mock upstream are identical
        assert_eq!(
            m0_payload, m1_payload,
            "Forwarded payload in M1 must be identical to native M0 request"
        );

        // Header differences: internal credentials stripped in M1
        assert!(
            m0_headers.get("x-cw-control-token").is_none(),
            "M0 has no cw headers"
        );
        assert!(
            m1_headers.get("x-cw-control-token").is_none(),
            "M1 forwarded request must strip internal control token before egress"
        );
        assert!(
            m1_headers.get("x-cw-session-id").is_none(),
            "M1 forwarded request must strip internal session ID before egress"
        );
    }

    // Verify SQLite recorded the turn, sequence, model, and provider
    let m1_turn = store
        .get_latest_turn(m1_session_id)
        .expect("store query failed")
        .expect("M1 turn must be recorded in SQLite");
    assert_eq!(m1_turn.model.as_deref(), Some("gpt-4o-mini"));
    assert_eq!(m1_turn.sequence_number, 1);
    assert_eq!(m1_turn.normalization_provider, Some(ProviderKind::OpenAI));

    // Measured forwarding overhead (M1 - M0) is real and bounded (< 150ms on loopback)
    println!(
        "Observed Socket Latencies: M0={:?}, M1={:?}, Delta={:?}",
        m0_elapsed,
        m1_elapsed,
        m1_elapsed.saturating_sub(m0_elapsed)
    );
    assert!(
        m1_elapsed.saturating_sub(m0_elapsed) < std::time::Duration::from_millis(150),
        "Proxy loopback forwarding overhead must remain under 150ms"
    );

    // ------------------------------------------------------------------------
    // Mode M2: ContextWatcher Active Optimization (Mutations committed)
    // ------------------------------------------------------------------------
    let m2_session_id = "cw118_m2_optimized_session";

    // Configure autopilot: enabled for active optimization
    let mut ap_cfg = store.get_autopilot_config().unwrap_or_default();
    ap_cfg.enabled = true;
    store.set_autopilot_config(&ap_cfg).unwrap();

    // Ensure session exists in SQLite before attaching mutations
    store
        .upsert_session(&Session {
            id: m2_session_id.to_string(),
            parent_session_id: None,
            harness: HarnessKind::GenericProxy,
            workspace_root: None,
            model: "gpt-4o-mini".to_string(),
            provider: ProviderKind::OpenAI,
            title: Some("CW-118 M2 Session".to_string()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        })
        .unwrap();

    // Register active mutation rule targeting the historical tool result call_mock_1
    let rule = MutationRule {
        id: "mut_cw118_m2_prune_1".to_string(),
        session_id: m2_session_id.to_string(),
        op: MutationOp::PruneOrStub {
            target_fingerprint: None,
            target_call_id: Some("call_mock_1".to_string()),
            target_json_pointer: None,
            stub_text: "[pruned]".to_string(),
        },
        status: MutationStatus::Committed,
        estimated_saved_tokens: 105,
        estimated_kv_break_tokens: 0,
        created_at: Utc::now(),
    };
    store.save_mutation(&rule).unwrap();

    let t2_start = std::time::Instant::now();
    let m2_resp = client
        .post(format!("{proxy_url}/openai/v1/chat/completions"))
        .header("x-cw-session-id", m2_session_id)
        .header("x-cw-control-token", &control_token)
        .json(&raw_request_body)
        .send()
        .await
        .expect("M2 proxy request failed");
    let m2_elapsed = t2_start.elapsed();

    assert_eq!(m2_resp.status(), 200);
    let m2_data: serde_json::Value = m2_resp.json().await.unwrap();
    // Prefix edits are deferred on cold/unknown cache confidence, so this first M2 turn
    // remains passthrough and usage matches the unmutated request.
    assert_eq!(m2_data["usage"]["prompt_tokens"], 150);
    assert_eq!(mock_calls.load(Ordering::SeqCst), 3);

    // Verify SQLite recorded the mutated turn for M2
    let m2_turn = store
        .get_latest_turn(m2_session_id)
        .expect("store query failed")
        .expect("M2 turn must be recorded in SQLite");
    assert_eq!(m2_turn.model.as_deref(), Some("gpt-4o-mini"));
    assert_eq!(m2_turn.sequence_number, 1);

    // Verify wire egress remains passthrough when the scheduler defers prefix mutations.
    let egress_bytes = store
        .get_wire_egress(&m2_turn.id)
        .expect("store query failed")
        .expect("Wire egress must be recorded for active mutation");
    let egress_str = String::from_utf8_lossy(&egress_bytes);
    assert!(
        !egress_str.contains("[pruned]"),
        "Cold-cache first turn should defer prefix pruning"
    );
    assert!(m2_turn.applied_mutation_ids.is_empty());

    println!(
        "M2 Real Active Optimization: Latency={:?}, Observed Prompt Tokens={}",
        m2_elapsed, m2_data["usage"]["prompt_tokens"]
    );
}

// ============================================================================
// 15. Cache-Safe Policy Qualification (Defer on Warm Cache vs PinContext)
// ============================================================================

#[tokio::test]
async fn test_cw118_cache_safe_policy_qualification_defer_and_pin() {
    // ------------------------------------------------------------------------
    // Part A: Historical pruning on warm cache is rejected/deferred by production policy
    // ------------------------------------------------------------------------
    let raw_payload = json!({
        "model": "gpt-4o-mini",
        "messages": [
            {
                "role": "user",
                "content": "Perform task F1"
            },
            {
                "role": "assistant",
                "content": null,
                "tool_calls": [
                    {
                        "id": "call_hist_1",
                        "type": "function",
                        "function": {
                            "name": "exec_command",
                            "arguments": "{\"command\": \"cargo test\"}"
                        }
                    }
                ]
            },
            {
                "role": "tool",
                "tool_call_id": "call_hist_1",
                "content": "RAW HISTORICAL TOOL LOG CONTENT THAT CONSUMES TOKENS 12345"
            },
            {
                "role": "assistant",
                "content": "Read completed."
            },
            {
                "role": "user",
                "content": "Next step."
            }
        ]
    });

    let (mut turn, _) = analyze_turn_from_payload(
        "turn_cs_1".into(),
        "sess_cs_1".into(),
        None,
        1,
        &raw_payload,
        ProviderKind::OpenAI,
        None,
        Some(200),
        None,
    )
    .expect("failed to analyze turn");

    // Establish genuine observed provider cache hits on prefix
    turn.kv_cache_report.telemetry_available = true;
    turn.kv_cache_report.hit_tokens = 120;

    let prefix_fp = compute_prefix_fingerprint(&turn.nodes);

    // Warm verified cache state (reported cache hits on prefix, no intentional rebuild)
    let warm_cache_obs = classify_cache_state(
        &turn.session_id,
        ProviderKind::OpenAI,
        "gpt-4o-mini",
        turn.sequence_number,
        Some(&prefix_fp),
        None,
        Some(&turn.kv_cache_report),
        false,
    );
    assert_eq!(warm_cache_obs.state, CacheState::WarmVerified);

    let pricing = find_pricing_profile(ProviderKind::OpenAI, "gpt-4o-mini");

    // Candidate 1: Historical PruneOrStub targeting call_hist_1
    let op_prune = MutationOp::PruneOrStub {
        target_fingerprint: None,
        target_call_id: Some("call_hist_1".to_string()),
        target_json_pointer: None,
        stub_text: "[pruned]".to_string(),
    };

    let decision_prune = evaluate_decision(
        Some("cand_prune_warm"),
        None,
        &op_prune,
        &turn,
        &warm_cache_obs,
        pricing.as_ref(),
        3,
    );

    // Production policy MUST safely defer historical pruning on warm cache to avoid invalidating the cache prefix!
    assert_eq!(decision_prune.decision, OptimizationDecision::Defer);
    assert_eq!(
        decision_prune.reason_code,
        DecisionReasonCode::CachePrefixAtRisk
    );
    assert!(
        decision_prune.estimated_kv_break_tokens > 0,
        "Historical pruning on warm cache must report estimated KV break tokens > 0"
    );

    // ------------------------------------------------------------------------
    // Part B: Eligible tail-only PinContext is verified cache-safe (0 KV break) and applied
    // ------------------------------------------------------------------------
    let op_pin = MutationOp::PinContext {
        label: "RULE".to_string(),
        content: "CRITICAL REPO RULE PRESERVED".to_string(),
        source_node_id: None,
    };

    let decision_pin = evaluate_decision(
        Some("cand_pin_tail"),
        None,
        &op_pin,
        &turn,
        &warm_cache_obs,
        pricing.as_ref(),
        3,
    );

    // Production policy explicitly returns NoOp for tail append operations:
    // Tail-only context pinning preserves prefix (0 KV break) but yields no standalone token reduction.
    assert_eq!(decision_pin.decision, OptimizationDecision::NoOp);
    assert_eq!(decision_pin.estimated_kv_break_tokens, 0);
    assert!(decision_pin.evidence.is_append_only);
    assert_eq!(
        decision_pin.reason_code,
        DecisionReasonCode::TailAppendPreservesPrefixNoDirectSaving
    );

    // End-to-end socket verification of PinContext application through cw-proxy
    let (mock_url, _mock_calls, received_reqs, _mock_task) = spawn_mock_openai_upstream().await;
    let store = CwStore::open_in_memory().unwrap();
    let (proxy_url, control_token, _proxy_task) =
        spawn_real_cw_proxy(store.clone(), mock_url.clone()).await;

    let pin_session_id = "sess_cs_pin_test";
    store
        .upsert_session(&Session {
            id: pin_session_id.to_string(),
            parent_session_id: None,
            harness: HarnessKind::GenericProxy,
            workspace_root: None,
            model: "gpt-4o-mini".to_string(),
            provider: ProviderKind::OpenAI,
            title: Some("CW-118 Pin Test Session".to_string()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        })
        .unwrap();

    let mut ap_cfg = store.get_autopilot_config().unwrap_or_default();
    ap_cfg.enabled = true;
    store.set_autopilot_config(&ap_cfg).unwrap();

    let pin_rule = MutationRule {
        id: "mut_pin_rule_1".to_string(),
        session_id: pin_session_id.to_string(),
        op: op_pin,
        status: MutationStatus::Committed,
        estimated_saved_tokens: 0,
        estimated_kv_break_tokens: 0,
        created_at: Utc::now(),
    };
    store.save_mutation(&pin_rule).unwrap();
    store
        .set_pin_context_policy(&pin_rule.session_id, true, true)
        .unwrap();

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{proxy_url}/openai/v1/chat/completions"))
        .header("x-cw-session-id", pin_session_id)
        .header("x-cw-control-token", &control_token)
        .json(&raw_payload)
        .send()
        .await
        .expect("Proxy request with pinned context failed");

    assert_eq!(resp.status(), 200);

    // Inspect the outbound request captured at the mock upstream
    let reqs = received_reqs.lock().unwrap();
    assert_eq!(reqs.len(), 1);
    let (_headers, payload_received) = &reqs[0];
    let payload_str = serde_json::to_string(payload_received).unwrap();

    // Verify pinned context was injected at the tail
    assert!(
        payload_str.contains("<cw_pinned_context>"),
        "Outbound request must contain injected <cw_pinned_context> block"
    );
    assert!(
        payload_str.contains("CRITICAL REPO RULE PRESERVED"),
        "Outbound request must contain the pinned rule text"
    );

    // Structural prefix comparison: verify all original messages are structurally identical and in the exact same order before the appended tail!
    let orig_msgs = raw_payload["messages"].as_array().unwrap();
    let recv_msgs = payload_received["messages"]
        .as_array()
        .expect("messages array must exist");
    assert_eq!(
        orig_msgs.len(),
        recv_msgs.len(),
        "Number of messages must remain identical"
    );

    // All prefix messages before the tail must be structurally identical (100% prefix preservation):
    for idx in 0..orig_msgs.len() - 1 {
        assert_eq!(
            &orig_msgs[idx], &recv_msgs[idx],
            "Prefix message {idx} must be structurally identical to original"
        );
    }

    // Tail message must preserve original role and prefix content:
    let last_orig = orig_msgs.last().unwrap();
    let last_recv = recv_msgs.last().unwrap();
    assert_eq!(last_orig["role"], last_recv["role"]);
    let orig_tail_text = last_orig["content"].as_str().unwrap();
    let recv_tail_text = last_recv["content"].as_str().unwrap();
    assert!(
        recv_tail_text.starts_with(orig_tail_text),
        "Tail message must preserve original text prefix"
    );

    // Verify wire egress in SQLite contains the pinned context
    let turn_rec = store
        .get_latest_turn(pin_session_id)
        .unwrap()
        .expect("Turn must be recorded");
    let egress_bytes = store.get_wire_egress(&turn_rec.id).unwrap().unwrap();
    let egress_str = String::from_utf8_lossy(&egress_bytes);
    assert!(egress_str.contains("<cw_pinned_context>"));
}
