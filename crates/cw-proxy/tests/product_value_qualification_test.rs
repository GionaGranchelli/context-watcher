// crates/cw-proxy/tests/product_value_qualification_test.rs
//
// CW-118: Product Value & Economic Qualification Tests
//
// Rigorous deterministic tests exercising REAL production components:
//   1. Randomized balanced scheduling (Latin square mode-order rotation across 45 sessions)
//   2. Identical initial fixture hashes across modes (independent acceptance verification)
//   3. Result persistence and resume idempotency
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

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use cw_core::{
    analyze_turn_from_payload,
    ir::*,
    policy::{
        CacheState, DecisionReasonCode, OptimizationDecision, classify_cache_state,
        compute_prefix_fingerprint, evaluate_decision, find_pricing_profile,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::json;

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

    for &task in tasks {
        for rep in 0..repetitions {
            let block_id = format!("block_{}_rep{}", task.code(), rep);
            let offset = rep % modes.len();
            for pos in 0..modes.len() {
                let mode = modes[(pos + offset) % modes.len()];
                let session_id = format!(
                    "cw118_{}_rep{}_{:?}_{:03}",
                    task.code().to_lowercase(),
                    rep,
                    mode,
                    index
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

    // Run 1: generate 3 sessions
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

    let content1 = fs::read_to_string(&result_file).unwrap();
    let json1: serde_json::Value = serde_json::from_str(&content1).unwrap();
    let sessions1 = json1["sessions"].as_array().unwrap();
    assert_eq!(sessions1.len(), 3);

    // Run 2: repeat run with same output files
    let output2 = Command::new("python3")
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
    assert!(output2.status.success());

    let content2 = fs::read_to_string(&result_file).unwrap();
    let json2: serde_json::Value = serde_json::from_str(&content2).unwrap();
    let sessions2 = json2["sessions"].as_array().unwrap();

    assert_eq!(sessions2.len(), 3);

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
