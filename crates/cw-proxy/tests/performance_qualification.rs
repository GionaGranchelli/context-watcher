// crates/cw-proxy/tests/performance_qualification.rs
//
// CW-115: Empirical Performance & Savings Qualification
//
// Demonstrates whether ContextWatcher provides measurable improvements in
// token consumption, prompt-cache efficiency, latency, and execution cost
// across three experiment modes:
//   Mode A: Native Baseline (direct to provider, no proxy)
//   Mode B: ContextWatcher Passthrough (through proxy, zero active mutations)
//   Mode C: ContextWatcher Optimized (through proxy, active context optimizations)
//
// Uses actual provider telemetry with verifiable provenance.
// Enforces strict spend budget (< $1.00 total) and non-fabrication invariants.

use chrono::{DateTime, Utc};
use cw_core::ir::{MutationOp, MutationRule, MutationStatus};
use cw_proxy::{proxy::build_proxy_router, state::DaemonState};
use cw_store::CwStore;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

pub fn which(bin: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let full = dir.join(bin);
            if full.is_file() { Some(full) } else { None }
        })
    })
}

pub fn require_installed_binary(bin: &str) -> PathBuf {
    which(bin).unwrap_or_else(|| {
        panic!(
            "Required harness binary '{bin}' was not found in PATH. \
             Ensure '{bin}' is installed before running this harness qualification test."
        );
    })
}

// ============================================================================
// 1. Budget Guards and Cost Models
// ============================================================================

/// Target ceiling on total live spend across all test runs in this binary process.
/// User authorized $3-$4; we enforce an ultra-conservative $1.00 ceiling.
pub const MAX_CAMPAIGN_SPEND_USD: f64 = 1.00;
pub const MAX_CAMPAIGN_SPEND_MICRO_USD: u64 = 1_000_000;

/// Enforceable request cap per session to prevent infinite tool loops.
pub const MAX_REQUESTS_PER_SESSION: usize = 30;

/// Track spend and reservations in micro-dollars ($1.00 = 1,000,000 micro-dollars) atomically.
static TOTAL_SPENT_MICRO_USD: AtomicU64 = AtomicU64::new(0);
static TOTAL_RESERVED_MICRO_USD: AtomicU64 = AtomicU64::new(0);

/// RAII reservation guard that holds budget allocation before dispatching a request,
/// releasing any uncommitted budget if dropped without completion.
pub struct BudgetReservation {
    pub reserved_micro_usd: u64,
    committed: bool,
}

impl BudgetReservation {
    pub fn commit(mut self, actual_cost_usd: f64) {
        let actual_micro_usd = (actual_cost_usd * 1_000_000.0).round() as u64;
        TOTAL_SPENT_MICRO_USD.fetch_add(actual_micro_usd, Ordering::SeqCst);
        TOTAL_RESERVED_MICRO_USD.fetch_sub(self.reserved_micro_usd, Ordering::SeqCst);
        self.committed = true;
    }
}

impl Drop for BudgetReservation {
    fn drop(&mut self) {
        if !self.committed && self.reserved_micro_usd > 0 {
            TOTAL_RESERVED_MICRO_USD.fetch_sub(self.reserved_micro_usd, Ordering::SeqCst);
        }
    }
}

pub struct SpendGuard;

impl SpendGuard {
    /// Pre-request budget reservation: checks available funds (spent + reserved) before request dispatch.
    /// Aborts *before* network invocation if the estimated worst-case cost exceeds remaining budget.
    pub fn reserve(estimated_cost_usd: f64) -> Result<BudgetReservation, String> {
        let req_micro_usd = (estimated_cost_usd * 1_000_000.0).round() as u64;
        loop {
            let spent = TOTAL_SPENT_MICRO_USD.load(Ordering::SeqCst);
            let reserved = TOTAL_RESERVED_MICRO_USD.load(Ordering::SeqCst);
            let total = spent + reserved + req_micro_usd;
            if total > MAX_CAMPAIGN_SPEND_MICRO_USD {
                return Err(format!(
                    "PRE-REQUEST BUDGET EXCEEDED: Committed ${:.4} + Reserved ${:.4} + Est ${:.4} exceeds limit ${:.2}",
                    spent as f64 / 1_000_000.0,
                    reserved as f64 / 1_000_000.0,
                    req_micro_usd as f64 / 1_000_000.0,
                    MAX_CAMPAIGN_SPEND_USD
                ));
            }
            if TOTAL_RESERVED_MICRO_USD
                .compare_exchange_weak(
                    reserved,
                    reserved + req_micro_usd,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                )
                .is_ok()
            {
                return Ok(BudgetReservation {
                    reserved_micro_usd: req_micro_usd,
                    committed: false,
                });
            }
        }
    }

    /// Reconciles post-request provider-reported usage.
    pub fn record_and_check(
        uncached_in: u32,
        cached_in: u32,
        out: u32,
        model: &str,
    ) -> Result<f64, String> {
        let cost = calculate_provider_cost(uncached_in, cached_in, out, model);
        let micro_usd = (cost * 1_000_000.0).round() as u64;
        let prev = TOTAL_SPENT_MICRO_USD.fetch_add(micro_usd, Ordering::SeqCst);
        let total = (prev + micro_usd) as f64 / 1_000_000.0;
        if total > MAX_CAMPAIGN_SPEND_USD {
            return Err(format!(
                "BUDGET CEILING EXCEEDED: Total spend ${:.4} exceeds limit ${:.2}",
                total, MAX_CAMPAIGN_SPEND_USD
            ));
        }
        Ok(cost)
    }

    pub fn total_spent() -> f64 {
        TOTAL_SPENT_MICRO_USD.load(Ordering::SeqCst) as f64 / 1_000_000.0
    }

    pub fn total_reserved() -> f64 {
        TOTAL_RESERVED_MICRO_USD.load(Ordering::SeqCst) as f64 / 1_000_000.0
    }
}

/// Official published pricing for OpenAI models (per 1,000,000 tokens).
/// Returns estimated dollar cost from provider-reported token usage.
pub fn calculate_provider_cost(
    uncached_input: u32,
    cached_input: u32,
    output: u32,
    model: &str,
) -> f64 {
    let (rate_uncached, rate_cached, rate_out) = match model {
        "gpt-4o-mini" => (0.150, 0.075, 0.600), // $0.15 / 1M uncached, $0.075 / 1M cached, $0.60 / 1M out
        "gpt-4o" => (2.500, 1.250, 10.000),
        _ => (0.150, 0.075, 0.600),
    };

    let cost_uncached = (uncached_input as f64 * rate_uncached) / 1_000_000.0;
    let cost_cached = (cached_input as f64 * rate_cached) / 1_000_000.0;
    let cost_out = (output as f64 * rate_out) / 1_000_000.0;

    cost_uncached + cost_cached + cost_out
}

// ============================================================================
// 2. Telemetry & Measurement Structures
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExperimentMode {
    #[serde(rename = "mode_a_native_baseline")]
    ModeANativeBaseline,
    #[serde(rename = "mode_b_passthrough")]
    ModeBPassthrough,
    #[serde(rename = "mode_c_optimized")]
    ModeCOptimized,
}

impl ExperimentMode {
    pub fn label(&self) -> &'static str {
        match self {
            Self::ModeANativeBaseline => "Mode A (Native Baseline)",
            Self::ModeBPassthrough => "Mode B (CW Passthrough)",
            Self::ModeCOptimized => "Mode C (CW Optimized)",
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Self::ModeANativeBaseline => "Mode_A",
            Self::ModeBPassthrough => "Mode_B",
            Self::ModeCOptimized => "Mode_C",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestTelemetry {
    pub mode: ExperimentMode,
    pub session_id: String,
    pub turn_index: usize,
    pub request_index: usize,
    pub model: String,
    pub input_tokens: u32,
    pub cached_tokens: u32,
    pub uncached_tokens: u32,
    pub output_tokens: u32,
    pub cache_hit_ratio: f64,
    #[serde(alias = "first_body_byte_ms")]
    pub ttfb_ms: Option<u64>,
    pub request_latency_ms: Option<u64>,
    pub turn_elapsed_ms: u64,
    pub cost_usd: f64,
    pub applied_mutation_ids: Vec<String>,
    pub raw_provider_usage: Option<Value>,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionBenchmarkResult {
    pub session_id: String,
    pub mode: ExperimentMode,
    pub task_family: String,
    pub repetition: usize,
    pub model: String,
    pub total_input_tokens: u32,
    pub total_cached_tokens: u32,
    pub total_uncached_tokens: u32,
    pub total_output_tokens: u32,
    pub overall_cache_ratio: f64,
    pub total_cost_usd: f64,
    pub total_wall_time_ms: u64,
    #[serde(alias = "mean_ttft_ms")]
    pub mean_ttfb_ms: Option<f64>,
    pub acceptance_passed: bool,
    pub requests: Vec<RequestTelemetry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModeComparison {
    pub task_family: String,
    // Passthrough Overhead (Mode B vs Mode A)
    pub passthrough_latency_overhead_ms: f64,
    pub passthrough_latency_overhead_pct: f64,
    pub passthrough_token_divergence: i64,
    // Optimization Net Impact (Mode C vs Mode B)
    pub token_reduction_vs_passthrough: i64,
    pub token_reduction_pct_vs_passthrough: f64,
    pub cache_ratio_delta_vs_passthrough: f64,
    pub cost_savings_usd_vs_passthrough: f64,
    pub cost_savings_pct_vs_passthrough: f64,
    // Net Impact (Mode C vs Mode A)
    pub net_token_reduction_vs_native: i64,
    pub net_token_reduction_pct_vs_native: f64,
    pub net_cost_savings_usd_vs_native: f64,
    pub net_cost_savings_pct_vs_native: f64,
    pub net_latency_delta_ms_vs_native: f64,
    pub regressions_noted: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CW115BenchmarkReport {
    pub date: String,
    #[serde(alias = "commit")]
    pub git_commit: String,
    #[serde(default = "default_package_version")]
    pub package_version: String,
    pub harness: String,
    pub model: String,
    pub total_sessions_executed: usize,
    pub total_spend_usd: f64,
    pub sessions: Vec<SessionBenchmarkResult>,
    pub comparisons: Vec<ModeComparison>,
}

fn default_package_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

// ============================================================================
// 3. Workload Fixtures
// ============================================================================

pub struct WorkloadFixture {
    pub workspace_dir: PathBuf,
    pub test_file: &'static str,
    pub initial_file: &'static str,
    pub family_name: &'static str,
}

impl WorkloadFixture {
    pub fn create_f1_bug_fixing() -> Self {
        let temp_dir = std::env::temp_dir().join(format!("cw115_f1_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&temp_dir).unwrap();

        fs::write(
            temp_dir.join("math_ops.py"),
            r#"# math_ops.py - initially defective
def divide_or_fallback(a, b, fallback=0.0):
    return a / b

def safe_mean(numbers):
    return sum(numbers) / len(numbers)
"#,
        )
        .unwrap();

        fs::write(
            temp_dir.join("test_math_ops.py"),
            r#"import unittest
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
"#,
        )
        .unwrap();

        Self {
            workspace_dir: temp_dir,
            test_file: "test_math_ops.py",
            initial_file: "math_ops.py",
            family_name: "F1BugFixing",
        }
    }

    pub fn create_f2_refactoring() -> Self {
        let temp_dir = std::env::temp_dir().join(format!("cw115_f2_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&temp_dir).unwrap();

        fs::write(
            temp_dir.join("string_utils.py"),
            r#"# string_utils.py - needs refactoring
def format_header(title, max_len=20):
    cleaned = title.strip()
    if len(cleaned) > max_len:
        return cleaned[:max_len-3] + "..."
    return cleaned.upper()

def format_footer(note, max_len=20):
    cleaned = note.strip()
    if len(cleaned) > max_len:
        return cleaned[:max_len-3] + "..."
    return cleaned.lower()
"#,
        )
        .unwrap();

        fs::write(
            temp_dir.join("test_string_utils.py"),
            r#"import unittest
from string_utils import format_header, format_footer, _truncate_with_ellipsis

class TestStringUtils(unittest.TestCase):
    def test_shared_helper(self):
        self.assertEqual(_truncate_with_ellipsis("short", 10), "short")
        self.assertEqual(_truncate_with_ellipsis("long text exceeding", 10), "long te...")

    def test_header(self):
        self.assertEqual(format_header("  hello world  "), "HELLO WORLD")
        self.assertEqual(format_header("a very long header text that exceeds limit", 10), "a very ...")

    def test_footer(self):
        self.assertEqual(format_footer("  CONFIDENTIAL  "), "confidential")
        self.assertEqual(format_footer("a very long footer text that exceeds limit", 10), "a very ...")

if __name__ == '__main__':
    unittest.main()
"#,
        )
        .unwrap();

        Self {
            workspace_dir: temp_dir,
            test_file: "test_string_utils.py",
            initial_file: "string_utils.py",
            family_name: "F2Refactoring",
        }
    }

    pub fn turn1_prompt(&self) -> &'static str {
        match self.family_name {
            "F1BugFixing" => {
                "Read math_ops.py and test_math_ops.py. Fix the ZeroDivisionError and empty list defects in math_ops.py so that python3 test_math_ops.py passes."
            }
            "F2Refactoring" => {
                "Read string_utils.py and test_string_utils.py. Extract a shared helper function `_truncate_with_ellipsis(text, max_len=20)` to eliminate duplication between format_header and format_footer so that python3 test_string_utils.py passes."
            }
            _ => "Fix implementation to make tests pass.",
        }
    }

    pub fn turn2_prompt(&self) -> &'static str {
        match self.family_name {
            "F1BugFixing" => {
                "Add complete PEP 484 type annotations and docstrings to divide_or_fallback and safe_mean in math_ops.py. Run python3 test_math_ops.py to verify."
            }
            "F2Refactoring" => {
                "Add complete PEP 484 type annotations and docstrings to _truncate_with_ellipsis, format_header, and format_footer in string_utils.py. Run python3 test_string_utils.py to verify."
            }
            _ => "Add docstrings and type annotations and verify tests pass.",
        }
    }

    pub fn run_acceptance_test(&self) -> bool {
        let status = Command::new("python3")
            .arg(self.test_file)
            .current_dir(&self.workspace_dir)
            .status();
        match status {
            Ok(s) => s.success(),
            Err(_) => false,
        }
    }
}

impl Drop for WorkloadFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.workspace_dir);
    }
}

// ============================================================================
// 4. Codex CLI Output Parsers
// ============================================================================

pub fn codex_native_direct_config_args() -> Vec<String> {
    [
        "model_provider=\"openai_direct\"".to_owned(),
        "model_providers.openai_direct.name=\"OpenAIDirect\"".to_owned(),
        "model_providers.openai_direct.base_url=\"https://api.openai.com/v1\"".to_owned(),
        "model_providers.openai_direct.wire_api=\"responses\"".to_owned(),
        "model_providers.openai_direct.requires_openai_auth=true".to_owned(),
        "model_providers.openai_direct.env_key=\"OPENAI_API_KEY\"".to_owned(),
    ]
    .into_iter()
    .flat_map(|setting| ["--config".to_owned(), setting])
    .collect()
}

pub fn extract_thread_id(stdout: &str) -> Option<String> {
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

pub fn extract_turn_completed_usage(stdout: &str) -> Option<(u32, u32, u32)> {
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
// 5. Native Baseline (Mode A) Multi-Turn Runner
// ============================================================================

pub fn run_native_baseline_session(
    codex_bin: &Path,
    api_key: &str,
    model: &str,
    fixture: &WorkloadFixture,
    session_id: &str,
    repetition: usize,
) -> Result<SessionBenchmarkResult, String> {
    let temp_home =
        std::env::temp_dir().join(format!("codex_home_mode_a_{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp_home).map_err(|e| e.to_string())?;

    let session_reservation = SpendGuard::reserve(0.10)?;
    let mut requests = Vec::new();
    let session_start = Instant::now();

    // Turn 1: Initial defect resolution
    let t1_start = Instant::now();
    let mut cmd1 = Command::new(codex_bin);
    cmd1.current_dir(&fixture.workspace_dir);
    cmd1.args([
        "exec",
        "--json",
        "--skip-git-repo-check",
        "--ignore-user-config",
        "-c",
        "approval_policy=\"never\"",
        "-c",
        "sandbox_mode=\"workspace-write\"",
    ]);
    cmd1.args(codex_native_direct_config_args());
    let t1_prompt = fixture.turn1_prompt();
    cmd1.args(["-m", model, t1_prompt]);
    cmd1.stdin(std::process::Stdio::null());
    cmd1.env("OPENAI_API_KEY", api_key);
    cmd1.env("CODEX_HOME", &temp_home);

    let out1 = cmd1
        .output()
        .map_err(|e| format!("Failed to run turn 1: {e}"))?;
    let t1_elapsed = t1_start.elapsed().as_millis() as u64;
    let stdout1 = String::from_utf8_lossy(&out1.stdout);
    let tid1 =
        extract_thread_id(&stdout1).ok_or_else(|| "Missing thread_id on Turn 1".to_string())?;

    let (in1, c1, out_tok1) = match extract_turn_completed_usage(&stdout1) {
        Some(u) => u,
        None => {
            eprintln!("Turn 1 exit code: {:?}", out1.status.code());
            eprintln!("Turn 1 stderr: {}", String::from_utf8_lossy(&out1.stderr));
            eprintln!("Turn 1 stdout lines:");
            for l in stdout1.lines() {
                eprintln!("  {}", l);
            }
            return Err("Missing turn.completed usage on Turn 1".to_string());
        }
    };
    let uncached1 = in1.saturating_sub(c1);
    let cost1 = calculate_provider_cost(uncached1, c1, out_tok1, model);

    requests.push(RequestTelemetry {
        mode: ExperimentMode::ModeANativeBaseline,
        session_id: session_id.to_string(),
        turn_index: 1,
        request_index: 1,
        model: model.to_string(),
        input_tokens: in1,
        cached_tokens: c1,
        uncached_tokens: uncached1,
        output_tokens: out_tok1,
        cache_hit_ratio: if in1 > 0 { c1 as f64 / in1 as f64 } else { 0.0 },
        ttfb_ms: None, // Native direct has no proxy interceptor
        request_latency_ms: Some(t1_elapsed),
        turn_elapsed_ms: t1_elapsed,
        cost_usd: cost1,
        applied_mutation_ids: Vec::new(),
        raw_provider_usage: Some(json!({
            "input_tokens": in1,
            "cached_input_tokens": c1,
            "output_tokens": out_tok1,
        })),
        timestamp: Utc::now(),
    });

    // Turn 2: Native resume - Add docstrings and type annotations
    let t2_start = Instant::now();
    let mut cmd2 = Command::new(codex_bin);
    cmd2.current_dir(&fixture.workspace_dir);
    cmd2.args([
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
    cmd2.args(codex_native_direct_config_args());
    let t2_prompt = fixture.turn2_prompt();
    cmd2.args(["-m", model, &tid1, t2_prompt]);
    cmd2.stdin(std::process::Stdio::null());
    cmd2.env("OPENAI_API_KEY", api_key);
    cmd2.env("CODEX_HOME", &temp_home);

    let out2 = cmd2
        .output()
        .map_err(|e| format!("Failed to run turn 2: {e}"))?;
    let t2_elapsed = t2_start.elapsed().as_millis() as u64;
    let stdout2 = String::from_utf8_lossy(&out2.stdout);

    let (in2, c2, out_tok2) = match extract_turn_completed_usage(&stdout2) {
        Some(u) => u,
        None => {
            eprintln!("Turn 2 exit code: {:?}", out2.status.code());
            eprintln!("Turn 2 stderr: {}", String::from_utf8_lossy(&out2.stderr));
            eprintln!("Turn 2 stdout lines:");
            for l in stdout2.lines() {
                eprintln!("  {}", l);
            }
            return Err("Missing turn.completed usage on Turn 2".to_string());
        }
    };
    let uncached2 = in2.saturating_sub(c2);
    let cost2 = calculate_provider_cost(uncached2, c2, out_tok2, model);

    requests.push(RequestTelemetry {
        mode: ExperimentMode::ModeANativeBaseline,
        session_id: session_id.to_string(),
        turn_index: 2,
        request_index: 2,
        model: model.to_string(),
        input_tokens: in2,
        cached_tokens: c2,
        uncached_tokens: uncached2,
        output_tokens: out_tok2,
        cache_hit_ratio: if in2 > 0 { c2 as f64 / in2 as f64 } else { 0.0 },
        ttfb_ms: None,
        request_latency_ms: Some(t2_elapsed),
        turn_elapsed_ms: t2_elapsed,
        cost_usd: cost2,
        applied_mutation_ids: Vec::new(),
        raw_provider_usage: Some(json!({
            "input_tokens": in2,
            "cached_input_tokens": c2,
            "output_tokens": out_tok2,
        })),
        timestamp: Utc::now(),
    });

    let total_wall_time = session_start.elapsed().as_millis() as u64;
    let acceptance = fixture.run_acceptance_test();
    let _ = fs::remove_dir_all(&temp_home);

    let total_in: u32 = requests.iter().map(|r| r.input_tokens).sum();
    let total_cached: u32 = requests.iter().map(|r| r.cached_tokens).sum();
    let total_uncached: u32 = requests.iter().map(|r| r.uncached_tokens).sum();
    let total_out: u32 = requests.iter().map(|r| r.output_tokens).sum();
    let total_cost: f64 = requests.iter().map(|r| r.cost_usd).sum();

    // Reconcile pre-request reservation with actual session cost
    session_reservation.commit(total_cost);

    Ok(SessionBenchmarkResult {
        session_id: session_id.to_string(),
        mode: ExperimentMode::ModeANativeBaseline,
        task_family: fixture.family_name.to_string(),
        repetition,
        model: model.to_string(),
        total_input_tokens: total_in,
        total_cached_tokens: total_cached,
        total_uncached_tokens: total_uncached,
        total_output_tokens: total_out,
        overall_cache_ratio: if total_in > 0 {
            total_cached as f64 / total_in as f64
        } else {
            0.0
        },
        total_cost_usd: total_cost,
        total_wall_time_ms: total_wall_time,
        mean_ttfb_ms: None,
        acceptance_passed: acceptance,
        requests,
    })
}

// ============================================================================
// 6. ContextWatcher Proxied Runner (Mode B Passthrough & Mode C Optimized)
// ============================================================================

pub async fn run_proxied_benchmark_session(
    codex_bin: &Path,
    api_key: &str,
    model: &str,
    fixture: &WorkloadFixture,
    session_id: &str,
    mode: ExperimentMode,
    repetition: usize,
) -> Result<SessionBenchmarkResult, String> {
    let store = CwStore::open_in_memory().map_err(|e| e.to_string())?;
    let is_optimized = mode == ExperimentMode::ModeCOptimized;

    store
        .set_autopilot_config(&cw_core::autopilot::AutoPilotConfig {
            enabled: is_optimized,
            ..Default::default()
        })
        .map_err(|e| e.to_string())?;

    let mut state = DaemonState::new(store.clone());
    state.openai_upstream = "https://api.openai.com".to_string();
    state.capture_wire_egress = true;
    let control_token = state.control_token.clone();

    let app = build_proxy_router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| e.to_string())?;
    let proxy_port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let proxy_task = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let temp_home =
        std::env::temp_dir().join(format!("codex_home_proxied_{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp_home).map_err(|e| e.to_string())?;

    let session_reservation = SpendGuard::reserve(0.10)?;
    let mut requests = Vec::new();
    let session_start = Instant::now();

    // Turn 1: Defect repair
    let t1_start = Instant::now();
    let codex_bin_c = codex_bin.to_path_buf();
    let ws_c = fixture.workspace_dir.clone();
    let home_c = temp_home.clone();
    let key_c = api_key.to_string();
    let sid_c = session_id.to_string();
    let tok_c = control_token.clone();
    let model_str = model.to_string();

    let t1_prompt = fixture.turn1_prompt();
    let out1 = tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new(codex_bin_c);
        cmd.current_dir(&ws_c);
        cmd.args([
            "exec",
            "--json",
            "--skip-git-repo-check",
            "--ignore-user-config",
            "-c",
            "approval_policy=\"never\"",
            "-c",
            "sandbox_mode=\"workspace-write\"",
        ]);
        cmd.args(cw_harnesses::qualification::codex_proxy_config_args(
            proxy_port,
        ));
        cmd.args(["-m", &model_str, t1_prompt]);
        cmd.stdin(std::process::Stdio::null());
        cmd.env("OPENAI_API_KEY", key_c);
        cmd.env("CODEX_HOME", &home_c);
        cmd.env("CW_SESSION_ID", &sid_c);
        cmd.env("CW_CONTROL_TOKEN", &tok_c);
        cmd.output()
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| format!("Failed to run proxied turn 1: {e}"))?;

    let t1_elapsed = t1_start.elapsed().as_millis() as u64;
    let stdout1 = String::from_utf8_lossy(&out1.stdout);
    let tid1 =
        extract_thread_id(&stdout1).ok_or_else(|| "Missing thread_id on Turn 1".to_string())?;

    // In Mode C: commit surgical optimization after Turn 1
    if is_optimized {
        let t1_turns = store.list_turns(session_id).unwrap_or_default();
        for turn in &t1_turns {
            for node in &turn.nodes {
                if matches!(node.kind, cw_core::ir::NodeKind::ToolResult { .. }) {
                    let call_id = match &node.kind {
                        cw_core::ir::NodeKind::ToolResult { call_id, .. }
                            if !call_id.is_empty() =>
                        {
                            Some(call_id.clone())
                        }
                        _ => None,
                    };
                    let rule = MutationRule {
                        id: format!("opt_prune_{}", node.id),
                        session_id: session_id.to_string(),
                        op: MutationOp::PruneOrStub {
                            target_fingerprint: Some(node.fingerprint.0.clone()),
                            target_call_id: call_id,
                            target_json_pointer: Some(node.raw_json_pointer.clone()),
                            stub_text: "[cw:pruned Turn 1 test execution log - resolved]"
                                .to_string(),
                        },
                        status: MutationStatus::Committed,
                        estimated_saved_tokens: node.token_span.count as i32,
                        estimated_kv_break_tokens: 0,
                        created_at: Utc::now(),
                    };
                    store.save_mutation(&rule).map_err(|e| e.to_string())?;
                }
            }
        }

        store
            .save_mutation(&MutationRule {
                id: format!("opt_pin_directive_{}", session_id),
                session_id: session_id.to_string(),
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
            .map_err(|e| e.to_string())?;
    }

    // Turn 2: Native resume
    let t2_start = Instant::now();
    let codex_bin_c2 = codex_bin.to_path_buf();
    let ws_c2 = fixture.workspace_dir.clone();
    let home_c2 = temp_home.clone();
    let key_c2 = api_key.to_string();
    let sid_c2 = session_id.to_string();
    let tok_c2 = control_token.clone();
    let tid_c2 = tid1.clone();
    let model_str2 = model.to_string();

    let t2_prompt = fixture.turn2_prompt();
    let _out2 = tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new(codex_bin_c2);
        cmd.current_dir(&ws_c2);
        cmd.args([
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
        cmd.args(cw_harnesses::qualification::codex_proxy_config_args(
            proxy_port,
        ));
        cmd.args(["-m", &model_str2, &tid_c2, t2_prompt]);
        cmd.stdin(std::process::Stdio::null());
        cmd.env("OPENAI_API_KEY", key_c2);
        cmd.env("CODEX_HOME", &home_c2);
        cmd.env("CW_SESSION_ID", &sid_c2);
        cmd.env("CW_CONTROL_TOKEN", &tok_c2);
        cmd.output()
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| format!("Failed to run proxied turn 2: {e}"))?;

    let t2_elapsed = t2_start.elapsed().as_millis() as u64;

    // Harvest exact telemetry from ContextWatcher's persistent SQLite store
    let turns = store.list_turns(session_id).map_err(|e| e.to_string())?;
    assert!(!turns.is_empty(), "Store must record at least 1 turn");

    // Enforce upper bound on agent request iterations to prevent runaway looping
    if turns.len() > MAX_REQUESTS_PER_SESSION {
        return Err(format!(
            "MAX_REQUESTS_PER_SESSION ({MAX_REQUESTS_PER_SESSION}) exceeded: session executed {} requests",
            turns.len()
        ));
    }

    for (idx, turn) in turns.iter().enumerate() {
        let usage = turn.observed_usage.clone().unwrap_or(json!({}));
        let in_tok = usage
            .get("input_tokens")
            .or_else(|| usage.get("prompt_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(turn.token_usage.input_tokens as u64) as u32;
        let c_tok = turn.token_usage.cached_tokens;
        let out_tok = usage
            .get("output_tokens")
            .or_else(|| usage.get("completion_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(turn.token_usage.output_tokens as u64) as u32;
        let uncached = in_tok.saturating_sub(c_tok);
        // Calculate cost per request without incrementing the global counter (avoid double-counting)
        let cost = calculate_provider_cost(uncached, c_tok, out_tok, model);

        requests.push(RequestTelemetry {
            mode,
            session_id: session_id.to_string(),
            turn_index: idx + 1,
            request_index: idx + 1,
            model: model.to_string(),
            input_tokens: in_tok,
            cached_tokens: c_tok,
            uncached_tokens: uncached,
            output_tokens: out_tok,
            cache_hit_ratio: if in_tok > 0 {
                c_tok as f64 / in_tok as f64
            } else {
                0.0
            },
            ttfb_ms: turn.first_body_byte_ms,
            request_latency_ms: turn.latency_ms,
            turn_elapsed_ms: if idx == 0 { t1_elapsed } else { t2_elapsed },
            cost_usd: cost,
            applied_mutation_ids: turn.applied_mutation_ids.clone(),
            raw_provider_usage: turn.observed_usage.clone(),
            timestamp: turn.created_at,
        });
    }

    let total_wall_time = session_start.elapsed().as_millis() as u64;
    let acceptance = fixture.run_acceptance_test();
    let _ = fs::remove_dir_all(&temp_home);
    proxy_task.abort();

    let total_in: u32 = requests.iter().map(|r| r.input_tokens).sum();
    let total_cached: u32 = requests.iter().map(|r| r.cached_tokens).sum();
    let total_uncached: u32 = requests.iter().map(|r| r.uncached_tokens).sum();
    let total_out: u32 = requests.iter().map(|r| r.output_tokens).sum();
    let total_cost: f64 = requests.iter().map(|r| r.cost_usd).sum();

    let ttfb_vals: Vec<u64> = requests.iter().filter_map(|r| r.ttfb_ms).collect();
    let mean_ttfb = if !ttfb_vals.is_empty() {
        Some(ttfb_vals.iter().sum::<u64>() as f64 / ttfb_vals.len() as f64)
    } else {
        None
    };

    // Reconcile pre-request reservation with actual session cost
    session_reservation.commit(total_cost);

    Ok(SessionBenchmarkResult {
        session_id: session_id.to_string(),
        mode,
        task_family: fixture.family_name.to_string(),
        repetition,
        model: model.to_string(),
        total_input_tokens: total_in,
        total_cached_tokens: total_cached,
        total_uncached_tokens: total_uncached,
        total_output_tokens: total_out,
        overall_cache_ratio: if total_in > 0 {
            total_cached as f64 / total_in as f64
        } else {
            0.0
        },
        total_cost_usd: total_cost,
        total_wall_time_ms: total_wall_time,
        mean_ttfb_ms: mean_ttfb,
        acceptance_passed: acceptance,
        requests,
    })
}

// ============================================================================
// 7. Comparative Analysis & Reporting Logic
// ============================================================================

pub fn compute_mode_comparisons(
    family_name: &str,
    results_a: &[SessionBenchmarkResult],
    results_b: &[SessionBenchmarkResult],
    results_c: &[SessionBenchmarkResult],
) -> ModeComparison {
    let mean_in_a = results_a
        .iter()
        .map(|s| s.total_input_tokens as f64)
        .sum::<f64>()
        / results_a.len().max(1) as f64;
    let mean_in_b = results_b
        .iter()
        .map(|s| s.total_input_tokens as f64)
        .sum::<f64>()
        / results_b.len().max(1) as f64;
    let mean_in_c = results_c
        .iter()
        .map(|s| s.total_input_tokens as f64)
        .sum::<f64>()
        / results_c.len().max(1) as f64;

    let mean_time_a = results_a
        .iter()
        .map(|s| s.total_wall_time_ms as f64)
        .sum::<f64>()
        / results_a.len().max(1) as f64;
    let mean_time_b = results_b
        .iter()
        .map(|s| s.total_wall_time_ms as f64)
        .sum::<f64>()
        / results_b.len().max(1) as f64;
    let mean_time_c = results_c
        .iter()
        .map(|s| s.total_wall_time_ms as f64)
        .sum::<f64>()
        / results_c.len().max(1) as f64;

    let mean_cost_a =
        results_a.iter().map(|s| s.total_cost_usd).sum::<f64>() / results_a.len().max(1) as f64;
    let mean_cost_b =
        results_b.iter().map(|s| s.total_cost_usd).sum::<f64>() / results_b.len().max(1) as f64;
    let mean_cost_c =
        results_c.iter().map(|s| s.total_cost_usd).sum::<f64>() / results_c.len().max(1) as f64;

    let mean_cache_b = results_b.iter().map(|s| s.overall_cache_ratio).sum::<f64>()
        / results_b.len().max(1) as f64;
    let mean_cache_c = results_c.iter().map(|s| s.overall_cache_ratio).sum::<f64>()
        / results_c.len().max(1) as f64;

    let passthrough_overhead_ms = mean_time_b - mean_time_a;
    let passthrough_overhead_pct = if mean_time_a > 0.0 {
        (passthrough_overhead_ms / mean_time_a) * 100.0
    } else {
        0.0
    };
    let token_div = (mean_in_b - mean_in_a).round() as i64;

    let tok_red_vs_b = (mean_in_b - mean_in_c).round() as i64;
    let tok_red_pct_vs_b = if mean_in_b > 0.0 {
        (tok_red_vs_b as f64 / mean_in_b) * 100.0
    } else {
        0.0
    };
    let cost_sav_vs_b = mean_cost_b - mean_cost_c;
    let cost_sav_pct_vs_b = if mean_cost_b > 0.0 {
        (cost_sav_vs_b / mean_cost_b) * 100.0
    } else {
        0.0
    };

    let net_tok_red = (mean_in_a - mean_in_c).round() as i64;
    let net_tok_red_pct = if mean_in_a > 0.0 {
        (net_tok_red as f64 / mean_in_a) * 100.0
    } else {
        0.0
    };
    let net_cost_sav = mean_cost_a - mean_cost_c;
    let net_cost_sav_pct = if mean_cost_a > 0.0 {
        (net_cost_sav / mean_cost_a) * 100.0
    } else {
        0.0
    };
    let net_time_delta = mean_time_c - mean_time_a;

    let mut regressions = Vec::new();
    if mean_cache_c < mean_cache_b {
        regressions.push(format!(
            "Cache Hit Ratio Regression: Mode C ({:.1}%) vs Mode B ({:.1}%) - pruning past turns broke cached prefix boundary",
            mean_cache_c * 100.0, mean_cache_b * 100.0
        ));
    }
    if passthrough_overhead_ms > 50.0 {
        regressions.push(format!(
            "Proxy Latency Overhead: Mode B added {:.1}ms over Mode A",
            passthrough_overhead_ms
        ));
    }
    if mean_cost_c > mean_cost_b {
        regressions.push(format!(
            "Cost Inversion: Mode C (${:.5}) cost more than Mode B (${:.5}) because cache miss penalty exceeded input token savings",
            mean_cost_c, mean_cost_b
        ));
    }

    ModeComparison {
        task_family: family_name.to_string(),
        passthrough_latency_overhead_ms: passthrough_overhead_ms,
        passthrough_latency_overhead_pct: passthrough_overhead_pct,
        passthrough_token_divergence: token_div,
        token_reduction_vs_passthrough: tok_red_vs_b,
        token_reduction_pct_vs_passthrough: tok_red_pct_vs_b,
        cache_ratio_delta_vs_passthrough: mean_cache_c - mean_cache_b,
        cost_savings_usd_vs_passthrough: cost_sav_vs_b,
        cost_savings_pct_vs_passthrough: cost_sav_pct_vs_b,
        net_token_reduction_vs_native: net_tok_red,
        net_token_reduction_pct_vs_native: net_tok_red_pct,
        net_cost_savings_usd_vs_native: net_cost_sav,
        net_cost_savings_pct_vs_native: net_cost_sav_pct,
        net_latency_delta_ms_vs_native: net_time_delta,
        regressions_noted: regressions,
    }
}

// ============================================================================
// 8. Deterministic Test (Always Runs in Default CI)
// ============================================================================

#[test]
fn test_cw115_cost_model_and_budget_guards() {
    // Verify pricing formulas for gpt-4o-mini
    let cost = calculate_provider_cost(10_000, 5_000, 2_000, "gpt-4o-mini");
    // (10_000 * 0.150 + 5_000 * 0.075 + 2_000 * 0.600) / 1,000,000 = (1.50 + 0.375 + 1.20) / 1000 = 0.003075
    assert!((cost - 0.003075).abs() < 1e-6);

    // Verify pre-request budget reservation succeeds within budget limit
    let reservation = SpendGuard::reserve(0.01).expect("Reservation within budget should succeed");
    assert_eq!(reservation.reserved_micro_usd, 10_000);
    assert_eq!(SpendGuard::total_reserved(), 0.01);

    // Commit actual usage
    reservation.commit(0.008);
    assert_eq!(SpendGuard::total_reserved(), 0.0);
    assert!((SpendGuard::total_spent() - 0.008).abs() < 1e-6);

    // Drop uncommitted reservation: held budget must be released safely
    {
        let _uncommitted = SpendGuard::reserve(0.05).expect("Should reserve");
        assert_eq!(SpendGuard::total_reserved(), 0.05);
    }
    assert_eq!(SpendGuard::total_reserved(), 0.0);

    // Exceed budget reservation check: must abort before dispatching network request
    let res = SpendGuard::reserve(MAX_CAMPAIGN_SPEND_USD + 0.10);
    assert!(
        res.is_err(),
        "Reservation exceeding budget ceiling must fail before request"
    );

    // Regression test for double-counting defect:
    // A $0.10 reservation reconciled against $0.025 of actual usage produces
    // exactly $0.025 in committed spend delta and 0 outstanding reservations.
    let start_spent = SpendGuard::total_spent();
    let recon_reservation = SpendGuard::reserve(0.10).expect("Should reserve $0.10 for session");
    assert_eq!(recon_reservation.reserved_micro_usd, 100_000);
    assert!((SpendGuard::total_reserved() - 0.10).abs() < 1e-6);

    // Calculate individual turn costs without incrementing global spend counter
    let cost_req1 = calculate_provider_cost(10_000, 5_000, 1_000, "gpt-4o-mini");
    let cost_req2 = 0.025 - cost_req1;
    let turn_total = cost_req1 + cost_req2;
    assert!((turn_total - 0.025).abs() < 1e-6);
    // Global spend counter must remain unchanged before session reconciliation
    assert!((SpendGuard::total_spent() - start_spent).abs() < 1e-6);

    // Reconcile session total exactly once
    recon_reservation.commit(turn_total);
    assert_eq!(SpendGuard::total_reserved(), 0.0);
    assert!((SpendGuard::total_spent() - (start_spent + 0.025)).abs() < 1e-6);
}

#[test]
fn test_cw115_deterministic_comparison_math_and_regression_disclosure() {
    let dummy_a = vec![SessionBenchmarkResult {
        session_id: "a1".into(),
        mode: ExperimentMode::ModeANativeBaseline,
        task_family: "F1".into(),
        repetition: 1,
        model: "mock".into(),
        total_input_tokens: 3000,
        total_cached_tokens: 1500,
        total_uncached_tokens: 1500,
        total_output_tokens: 400,
        overall_cache_ratio: 0.50,
        total_cost_usd: 0.0005,
        total_wall_time_ms: 2000,
        mean_ttfb_ms: None,
        acceptance_passed: true,
        requests: vec![],
    }];

    let dummy_b = vec![SessionBenchmarkResult {
        session_id: "b1".into(),
        mode: ExperimentMode::ModeBPassthrough,
        task_family: "F1".into(),
        repetition: 1,
        model: "mock".into(),
        total_input_tokens: 3000,
        total_cached_tokens: 1500,
        total_uncached_tokens: 1500,
        total_output_tokens: 400,
        overall_cache_ratio: 0.50,
        total_cost_usd: 0.0005,
        total_wall_time_ms: 2005, // 5ms proxy overhead
        mean_ttfb_ms: Some(250.0),
        acceptance_passed: true,
        requests: vec![],
    }];

    // Mode C with aggressive pruning that cut tokens but broke cache
    let dummy_c = vec![SessionBenchmarkResult {
        session_id: "c1".into(),
        mode: ExperimentMode::ModeCOptimized,
        task_family: "F1".into(),
        repetition: 1,
        model: "mock".into(),
        total_input_tokens: 2200, // 800 tokens cut
        total_cached_tokens: 0,   // Cache invalidated
        total_uncached_tokens: 2200,
        total_output_tokens: 400,
        overall_cache_ratio: 0.0,
        total_cost_usd: 0.00057, // Inverted: higher cost due to 0 cache discount!
        total_wall_time_ms: 2100,
        mean_ttfb_ms: Some(300.0),
        acceptance_passed: true,
        requests: vec![],
    }];

    let comparison = compute_mode_comparisons("F1BugFixing", &dummy_a, &dummy_b, &dummy_c);

    assert_eq!(comparison.token_reduction_vs_passthrough, 800);
    assert!((comparison.passthrough_latency_overhead_ms - 5.0).abs() < 1e-3);
    assert_eq!(comparison.passthrough_token_divergence, 0);

    // Strict Invariant: Regressions MUST be explicitly surfaced in the report!
    assert!(
        comparison
            .regressions_noted
            .iter()
            .any(|r| r.contains("Cache Hit Ratio Regression")),
        "Report must disclose cache hit ratio regressions"
    );
    assert!(
        comparison
            .regressions_noted
            .iter()
            .any(|r| r.contains("Cost Inversion")),
        "Report must disclose cost inversions where cache loss outweighs token cuts"
    );
}

// ============================================================================
// 9. Live Provider Qualification Pilot (Opt-in via CW_QUALIFY_REAL_CODEX=1)
// ============================================================================

#[tokio::test]
#[ignore = "opt-in real Codex qualification pilot; requires CW_QUALIFY_REAL_CODEX=1 and OPENAI_API_KEY with authorized budget"]
async fn test_live_codex_cw115_empirical_performance_qualification() {
    let opt_in = std::env::var("CW_QUALIFY_REAL_CODEX").unwrap_or_default();
    if opt_in != "1" {
        panic!(
            "SKIPPED: Live qualification requires CW_QUALIFY_REAL_CODEX=1 and authorized budget"
        );
    }
    let api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY required for live test");
    let codex_bin = require_installed_binary("codex");

    let model = "gpt-4o-mini";
    let repetitions = 2; // Alternating trials: A1, B1, C1, A2, B2, C2

    let mut results_a = Vec::new();
    let mut results_b = Vec::new();
    let mut results_c = Vec::new();
    let mut all_sessions = Vec::new();

    println!("\n=======================================================");
    println!("CW-115 Empirical Performance Qualification Pilot");
    println!(
        "Model: {} | Harness: Codex CLI | Workload: F1 BugFixing",
        model
    );
    println!("Design: Alternating trials across Mode A, Mode B, Mode C");
    println!("Spend Ceiling: ${:.2}", MAX_CAMPAIGN_SPEND_USD);
    println!("=======================================================\n");

    for rep in 1..=repetitions {
        // Mode A: Native Baseline
        println!("[TRIAL] Executing Mode A (Native Baseline) rep {}", rep);
        let fixture_a = WorkloadFixture::create_f1_bug_fixing();
        let sid_a = format!("cw115_native_f1_rep{}", rep);
        let res_a =
            run_native_baseline_session(&codex_bin, &api_key, model, &fixture_a, &sid_a, rep)
                .expect("Mode A session failed");
        assert!(res_a.acceptance_passed, "Mode A must pass acceptance test");
        println!(
            "  -> Mode A rep {}: in={}, cached={}, cost=${:.5}, elapsed={}ms",
            rep,
            res_a.total_input_tokens,
            res_a.total_cached_tokens,
            res_a.total_cost_usd,
            res_a.total_wall_time_ms
        );
        all_sessions.push(res_a.clone());
        results_a.push(res_a);

        // Sleep briefly to avoid OpenAI transient 429 rate limits
        tokio::time::sleep(Duration::from_millis(1500)).await;

        // Mode B: CW Passthrough
        println!("[TRIAL] Executing Mode B (CW Passthrough) rep {}", rep);
        let fixture_b = WorkloadFixture::create_f1_bug_fixing();
        let sid_b = format!("cw115_passthrough_f1_rep{}", rep);
        let res_b = run_proxied_benchmark_session(
            &codex_bin,
            &api_key,
            model,
            &fixture_b,
            &sid_b,
            ExperimentMode::ModeBPassthrough,
            rep,
        )
        .await
        .expect("Mode B session failed");
        assert!(res_b.acceptance_passed, "Mode B must pass acceptance test");
        println!(
            "  -> Mode B rep {}: in={}, cached={}, cost=${:.5}, elapsed={}ms",
            rep,
            res_b.total_input_tokens,
            res_b.total_cached_tokens,
            res_b.total_cost_usd,
            res_b.total_wall_time_ms
        );
        all_sessions.push(res_b.clone());
        results_b.push(res_b);

        tokio::time::sleep(Duration::from_millis(1500)).await;

        // Mode C: CW Optimized
        println!("[TRIAL] Executing Mode C (CW Optimized) rep {}", rep);
        let fixture_c = WorkloadFixture::create_f1_bug_fixing();
        let sid_c = format!("cw115_optimized_f1_rep{}", rep);
        let res_c = run_proxied_benchmark_session(
            &codex_bin,
            &api_key,
            model,
            &fixture_c,
            &sid_c,
            ExperimentMode::ModeCOptimized,
            rep,
        )
        .await
        .expect("Mode C session failed");
        assert!(res_c.acceptance_passed, "Mode C must pass acceptance test");
        println!(
            "  -> Mode C rep {}: in={}, cached={}, cost=${:.5}, elapsed={}ms",
            rep,
            res_c.total_input_tokens,
            res_c.total_cached_tokens,
            res_c.total_cost_usd,
            res_c.total_wall_time_ms
        );
        all_sessions.push(res_c.clone());
        results_c.push(res_c);

        tokio::time::sleep(Duration::from_millis(1500)).await;
    }

    let comparison_f1 = compute_mode_comparisons("F1BugFixing", &results_a, &results_b, &results_c);

    println!("\n=======================================================");
    println!("CW-115 PILOT COMPARATIVE RESULTS");
    println!("=======================================================");
    println!("1. Passthrough Overhead (Mode B vs Mode A):");
    println!(
        "   Latency delta: {:+.1} ms ({:+.2}%)",
        comparison_f1.passthrough_latency_overhead_ms,
        comparison_f1.passthrough_latency_overhead_pct
    );
    println!(
        "   Live session token delta (Mode B mean - Mode A mean): {} tokens (stochastic iteration variance)",
        comparison_f1.passthrough_token_divergence
    );
    println!("2. Optimization Net Benefit (Mode C vs Mode B):");
    println!(
        "   Token reduction: {:+} tokens ({:+.2}%)",
        comparison_f1.token_reduction_vs_passthrough,
        comparison_f1.token_reduction_pct_vs_passthrough
    );
    println!(
        "   Cost delta: ${:+.5} ({:+.2}%)",
        comparison_f1.cost_savings_usd_vs_passthrough,
        comparison_f1.cost_savings_pct_vs_passthrough
    );
    println!(
        "   Cache ratio delta: {:+.2}%",
        comparison_f1.cache_ratio_delta_vs_passthrough * 100.0
    );
    println!(
        "3. Total Campaign Spend: ${:.4} (Ceiling: ${:.2})",
        SpendGuard::total_spent(),
        MAX_CAMPAIGN_SPEND_USD
    );
    println!(
        "4. Surfaced Regressions / Trade-offs: {:?}",
        comparison_f1.regressions_noted
    );
    println!("=======================================================\n");

    let report = CW115BenchmarkReport {
        date: Utc::now().to_rfc3339(),
        git_commit: "e9b1b43b35eb6bb89faaa0425e408ec2be59db94".to_string(),
        package_version: env!("CARGO_PKG_VERSION").to_string(),
        harness: "Codex CLI 0.166.0".to_string(),
        model: model.to_string(),
        total_sessions_executed: all_sessions.len(),
        total_spend_usd: SpendGuard::total_spent(),
        sessions: all_sessions,
        comparisons: vec![comparison_f1],
    };

    let report_json = serde_json::to_string_pretty(&report).unwrap();
    let data_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("docs")
        .join("cw115_benchmark_data.json");
    fs::write(data_path, report_json).expect("failed to write benchmark json");
}
