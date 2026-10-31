#![allow(clippy::too_many_arguments, clippy::collapsible_if)]

use axum::{
    Json, Router,
    body::Body,
    http::{HeaderMap, Request, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::Utc;
use cw_core::ir::*;
use cw_proxy::{DaemonState, build_proxy_router};
use cw_store::CwStore;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
    time::Duration,
};
use tower::ServiceExt;

// ============================================================================
// 1. Task Families and Harnesses
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TaskFamily {
    F1BugFixing,
    F2Refactoring,
    F3FeatureImplementation,
    F4Tdd,
    F5InvestigationRecovery,
}

impl TaskFamily {
    pub fn all() -> &'static [TaskFamily] {
        &[
            TaskFamily::F1BugFixing,
            TaskFamily::F2Refactoring,
            TaskFamily::F3FeatureImplementation,
            TaskFamily::F4Tdd,
            TaskFamily::F5InvestigationRecovery,
        ]
    }

    pub fn code(&self) -> &'static str {
        match self {
            Self::F1BugFixing => "F1",
            Self::F2Refactoring => "F2",
            Self::F3FeatureImplementation => "F3",
            Self::F4Tdd => "F4",
            Self::F5InvestigationRecovery => "F5",
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::F1BugFixing => "Bug fixing (math_ops)",
            Self::F2Refactoring => "Refactoring (string_utils)",
            Self::F3FeatureImplementation => "Feature implementation (cache_system)",
            Self::F4Tdd => "Test-driven development (validator)",
            Self::F5InvestigationRecovery => "Investigation and recovery (pipeline)",
        }
    }

    pub fn test_file(&self) -> &'static str {
        match self {
            Self::F1BugFixing => "test_math_ops.py",
            Self::F2Refactoring => "test_string_utils.py",
            Self::F3FeatureImplementation => "test_cache_system.py",
            Self::F4Tdd => "test_validator.py",
            Self::F5InvestigationRecovery => "test_pipeline.py",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HarnessTarget {
    Codex,
    Omp,
    Hermes,
}

impl HarnessTarget {
    pub fn all() -> &'static [HarnessTarget] {
        &[
            HarnessTarget::Codex,
            HarnessTarget::Omp,
            HarnessTarget::Hermes,
        ]
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Codex => "Codex CLI",
            Self::Omp => "OMP",
            Self::Hermes => "Hermes Agent",
        }
    }

    pub fn binary_name(&self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Omp => "omp",
            Self::Hermes => "hermes",
        }
    }
}

// ============================================================================
// 2. Workload Workspace Generation
// ============================================================================

pub struct WorkloadFixture {
    pub workspace_dir: PathBuf,
    pub test_file: &'static str,
}

impl WorkloadFixture {
    pub fn create(family: TaskFamily) -> Self {
        let temp_dir =
            std::env::temp_dir().join(format!("cw114_{}_{}", family.code(), uuid::Uuid::new_v4()));
        fs::create_dir_all(&temp_dir).unwrap();

        match family {
            TaskFamily::F1BugFixing => {
                // Buggy initial implementation
                fs::write(
                    temp_dir.join("math_ops.py"),
                    r#"# math_ops.py - initially buggy
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
            }
            TaskFamily::F2Refactoring => {
                // Unrefactored duplicated logic
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
                ).unwrap();
            }
            TaskFamily::F3FeatureImplementation => {
                fs::write(
                    temp_dir.join("store.py"),
                    r#"# store.py
class KeyValueStore:
    def __init__(self):
        self._data = {}
    def put(self, k, v):
        self._data[k] = v
    def get(self, k):
        return self._data.get(k)
"#,
                )
                .unwrap();
                // Incomplete cache.py
                fs::write(
                    temp_dir.join("cache.py"),
                    r#"# cache.py - skeleton
class CachedStore:
    def __init__(self, backing_store):
        self.backing_store = backing_store
        self.cache = {}
    def get(self, k):
        return None
    def put(self, k, v):
        pass
"#,
                )
                .unwrap();
                fs::write(
                    temp_dir.join("test_cache_system.py"),
                    r#"import unittest
from store import KeyValueStore
from cache import CachedStore

class TestCacheSystem(unittest.TestCase):
    def test_cached_lookup(self):
        backend = KeyValueStore()
        backend.put("user_1", "Alice")
        cached = CachedStore(backend)
        self.assertEqual(cached.get("user_1"), "Alice")
        backend.put("user_1", "MutatedDirectly")
        # Should still return cached value "Alice"
        self.assertEqual(cached.get("user_1"), "Alice")

    def test_put_updates_both(self):
        backend = KeyValueStore()
        cached = CachedStore(backend)
        cached.put("key", "val")
        self.assertEqual(cached.get("key"), "val")
        self.assertEqual(backend.get("key"), "val")

if __name__ == '__main__':
    unittest.main()
"#,
                )
                .unwrap();
            }
            TaskFamily::F4Tdd => {
                fs::write(
                    temp_dir.join("validator.py"),
                    r#"# validator.py - initially empty
def validate_registration(username, email, age):
    return False
"#,
                )
                .unwrap();
                fs::write(
                    temp_dir.join("test_validator.py"),
                    r#"import unittest
from validator import validate_registration

class TestValidator(unittest.TestCase):
    def test_valid(self):
        self.assertTrue(validate_registration("alice", "alice@example.com", 25))
    def test_invalid_username(self):
        self.assertFalse(validate_registration("", "alice@example.com", 25))
    def test_invalid_email(self):
        self.assertFalse(validate_registration("alice", "bademail", 25))
    def test_invalid_age(self):
        self.assertFalse(validate_registration("alice", "alice@example.com", 12))
        self.assertFalse(validate_registration("alice", "alice@example.com", 150))

if __name__ == '__main__':
    unittest.main()
"#,
                )
                .unwrap();
            }
            TaskFamily::F5InvestigationRecovery => {
                // Broken pipeline initially
                fs::write(
                    temp_dir.join("pipeline.py"),
                    r#"# pipeline.py - broken import and syntax
import non_existent_broken_library_xyz

def run_pipeline(records):
    raise RuntimeError("Broken pipeline")
"#,
                )
                .unwrap();
                fs::write(
                    temp_dir.join("test_pipeline.py"),
                    r#"import unittest
from pipeline import run_pipeline

class TestPipeline(unittest.TestCase):
    def test_pipeline_processing(self):
        data = [{"id": 2, "score": 80}, {"id": 1, "score": 95}, {"id": 3, "score": 50}]
        res = run_pipeline(data)
        self.assertEqual(res, [{"id": 1, "score": 95}, {"id": 2, "score": 80}])

if __name__ == '__main__':
    unittest.main()
"#,
                )
                .unwrap();
            }
        }

        Self {
            workspace_dir: temp_dir,
            test_file: family.test_file(),
        }
    }

    pub fn verify_acceptance_tests(&self) -> bool {
        let output = Command::new("python3")
            .args(["-m", "unittest", self.test_file])
            .current_dir(&self.workspace_dir)
            .output();

        match output {
            Ok(out) => out.status.success(),
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
// 3. Campaign Session Evidence & Results
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionQualificationResult {
    pub session_id: String,
    pub harness: String,
    pub task_family: String,
    pub repetition: usize,
    pub status: String, // "PASS", "FAIL", "BLOCKED"
    pub logical_turns: usize,
    pub model_requests: usize,
    pub resume_preserved: bool,
    pub overlay_applied: bool,
    pub overlay_reapplied_across_resume: bool,
    pub tool_pairing_intact: bool,
    pub transcripts_unmodified: bool,
    pub credentials_isolated: bool,
    pub acceptance_test_passed: bool,
    pub stream_completion_clean: bool,
    pub protocol_reliability: bool,
    pub agentic_reliability: bool,
    pub failure_class: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CapturedRequest {
    pub path: String,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
    pub json_body: Option<Value>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct CampaignReport {
    pub baseline_commit: String,
    pub codex_version: String,
    pub hermes_version: String,
    pub omp_version: String,
    pub total_sessions: usize,
    pub passed_sessions: usize,
    pub failed_sessions: usize,
    pub blocked_sessions: usize,
    pub results: Vec<SessionQualificationResult>,
}

// ============================================================================
// 4. Unified Deterministic Campaign Runner
// ============================================================================

pub struct CampaignRunner {
    pub baseline_commit: String,
    pub codex_bin: Option<PathBuf>,
    pub hermes_bin: Option<PathBuf>,
    pub omp_bin: Option<PathBuf>,
}

impl Default for CampaignRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl CampaignRunner {
    pub fn new() -> Self {
        let baseline = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_else(|_| "unknown".to_string());

        let which = |name| {
            Command::new("which")
                .arg(name)
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()))
        };

        Self {
            baseline_commit: baseline,
            codex_bin: which("codex"),
            hermes_bin: which("hermes"),
            omp_bin: which("omp"),
        }
    }

    pub async fn run_session(
        &self,
        harness: HarnessTarget,
        family: TaskFamily,
        repetition: usize,
    ) -> SessionQualificationResult {
        let sid = format!(
            "cw114_{}_{}_rep{}",
            harness.binary_name(),
            family.code(),
            repetition
        );
        let fixture = WorkloadFixture::create(family);

        // Check binary availability
        let bin_opt = match harness {
            HarnessTarget::Codex => &self.codex_bin,
            HarnessTarget::Hermes => &self.hermes_bin,
            HarnessTarget::Omp => &self.omp_bin,
        };

        if bin_opt.is_none() {
            return SessionQualificationResult {
                session_id: sid,
                harness: harness.name().to_string(),
                task_family: family.name().to_string(),
                repetition,
                status: "BLOCKED".to_string(),
                logical_turns: 0,
                model_requests: 0,
                resume_preserved: false,
                overlay_applied: false,
                overlay_reapplied_across_resume: false,
                tool_pairing_intact: false,
                transcripts_unmodified: true,
                credentials_isolated: true,
                acceptance_test_passed: false,
                stream_completion_clean: false,
                protocol_reliability: false,
                agentic_reliability: false,
                failure_class: Some(format!("Binary '{}' not installed", harness.binary_name())),
            };
        }

        // Setup store and proxy
        let store = CwStore::open_in_memory().unwrap();
        store
            .set_autopilot_config(&cw_core::autopilot::AutoPilotConfig {
                enabled: true,
                ..Default::default()
            })
            .unwrap();

        let mut state = DaemonState::new(store.clone());
        let control_token = state.control_token.clone();

        // Setup mock provider with request capturing for this task family
        let captured = Arc::new(Mutex::new(Vec::new()));
        let (mock_url, mock_server) =
            setup_task_mock_provider(family, fixture.workspace_dir.clone(), captured.clone()).await;
        state.openai_upstream = mock_url.clone();
        state.anthropic_upstream = mock_url.clone();
        state.deepseek_upstream = mock_url.clone();

        let proxy_app = build_proxy_router(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_port = listener.local_addr().unwrap().port();
        let proxy_task = tokio::spawn(async move {
            axum::serve(listener, proxy_app).await.unwrap();
        });

        // Execute harness workflow across 8 checkpoints with native resume
        let run_result = match harness {
            HarnessTarget::Codex => {
                execute_codex_session(
                    bin_opt.as_ref().unwrap(),
                    proxy_port,
                    &control_token,
                    &sid,
                    &fixture,
                    &store,
                    family,
                    repetition,
                    captured.clone(),
                )
                .await
            }
            HarnessTarget::Omp => {
                execute_omp_session(
                    bin_opt.as_ref().unwrap(),
                    proxy_port,
                    &control_token,
                    &sid,
                    &fixture,
                    &store,
                    family,
                    repetition,
                    captured.clone(),
                )
                .await
            }
            HarnessTarget::Hermes => {
                execute_hermes_session(
                    bin_opt.as_ref().unwrap(),
                    proxy_port,
                    &control_token,
                    &sid,
                    &fixture,
                    &store,
                    family,
                    repetition,
                    captured.clone(),
                )
                .await
            }
        };

        // Teardown proxy and mock
        proxy_task.abort();
        mock_server.abort();

        run_result
    }

    pub async fn run_full_campaign(&self) -> CampaignReport {
        let mut results = Vec::new();

        for harness in HarnessTarget::all() {
            for family in TaskFamily::all() {
                for rep in 1..=3 {
                    let res = self.run_session(*harness, *family, rep).await;
                    results.push(res);
                }
            }
        }

        let passed = results.iter().filter(|r| r.status == "PASS").count();
        let failed = results.iter().filter(|r| r.status == "FAIL").count();
        let blocked = results.iter().filter(|r| r.status == "BLOCKED").count();

        let get_version = |cmd: &str| {
            Command::new(cmd)
                .arg("--version")
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_else(|_| "absent".to_string())
        };

        CampaignReport {
            baseline_commit: self.baseline_commit.clone(),
            codex_version: get_version("codex"),
            hermes_version: get_version("hermes"),
            omp_version: get_version("omp"),
            total_sessions: results.len(),
            passed_sessions: passed,
            failed_sessions: failed,
            blocked_sessions: blocked,
            results,
        }
    }
}

// ============================================================================
// 5. Mock Provider Setup with Realistic Tool Executions for F1..F5
// ============================================================================

fn family_tool_action(family: TaskFamily) -> (&'static str, &'static str) {
    match family {
        TaskFamily::F1BugFixing => (
            "math_ops.py",
            r#"# math_ops.py - repaired by tool
def divide_or_fallback(a, b, fallback=0.0):
    if b == 0:
        return fallback
    return a / b

def safe_mean(numbers):
    if not numbers:
        return 0.0
    return sum(numbers) / len(numbers)
"#,
        ),
        TaskFamily::F2Refactoring => (
            "string_utils.py",
            r#"# string_utils.py - refactored by tool
def _truncate_with_ellipsis(text, max_len):
    if len(text) > max_len:
        return text[:max_len-3] + "..."
    return text

def format_header(title, max_len=20):
    cleaned = title.strip()
    truncated = _truncate_with_ellipsis(cleaned, max_len)
    if truncated.endswith("..."):
        return truncated
    return truncated.upper()

def format_footer(note, max_len=20):
    cleaned = note.strip()
    truncated = _truncate_with_ellipsis(cleaned, max_len)
    if truncated.endswith("..."):
        return truncated
    return truncated.lower()
"#,
        ),
        TaskFamily::F3FeatureImplementation => (
            "cache.py",
            r#"# cache.py - implemented by tool
class CachedStore:
    def __init__(self, backing_store):
        self.backing_store = backing_store
        self.cache = {}
    def get(self, k):
        if k in self.cache:
            return self.cache[k]
        val = self.backing_store.get(k)
        if val is not None:
            self.cache[k] = val
        return val
    def put(self, k, v):
        self.cache[k] = v
        self.backing_store.put(k, v)
"#,
        ),
        TaskFamily::F4Tdd => (
            "validator.py",
            r#"# validator.py - implemented by tool
def validate_registration(username, email, age):
    if not username or not username.strip():
        return False
    if "@" not in email or "." not in email:
        return False
    if age < 18 or age > 120:
        return False
    return True
"#,
        ),
        TaskFamily::F5InvestigationRecovery => (
            "pipeline.py",
            r#"# pipeline.py - recovered by tool
def run_pipeline(records):
    filtered = [r for r in records if r.get("score", 0) >= 70]
    return sorted(filtered, key=lambda r: r.get("score", 0), reverse=True)
"#,
        ),
    }
}

async fn setup_task_mock_provider(
    family: TaskFamily,
    workspace: PathBuf,
    captured: Arc<Mutex<Vec<CapturedRequest>>>,
) -> (String, tokio::task::JoinHandle<()>) {
    let workspace = Arc::new(workspace);
    let ws_clone1 = workspace.clone();
    let ws_clone2 = workspace.clone();
    let ws_clone3 = workspace.clone();
    let ws_clone4 = workspace.clone();
    let ws_clone5 = workspace.clone();
    let cap_clone1 = captured.clone();
    let cap_clone2 = captured.clone();
    let cap_clone3 = captured.clone();
    let cap_clone4 = captured.clone();
    let cap_clone5 = captured.clone();

    let app = Router::new()
        .route(
            "/v1/models",
            get(|| async { Json(json!({"data": [{"id": "mock-model", "object": "model"}]})) }),
        )
        .route(
            "/v1/models/:model",
            get(|| async { Json(json!({"id": "mock-model", "object": "model"})) }),
        )
        .route(
            "/models",
            get(|| async { Json(json!({"data": [{"id": "mock-model", "object": "model"}]})) }),
        )
        .route(
            "/models/:model",
            get(|| async { Json(json!({"id": "mock-model", "object": "model"})) }),
        )
        .route(
            "/deepseek/v1/models",
            get(|| async { Json(json!({"data": [{"id": "mock-model", "object": "model"}]})) }),
        )
        .route(
            "/deepseek/v1/models/:model",
            get(|| async { Json(json!({"id": "mock-model", "object": "model"})) }),
        )
        .route(
            "/deepseek/models",
            get(|| async { Json(json!({"data": [{"id": "mock-model", "object": "model"}]})) }),
        )
        .route(
            "/deepseek/models/:model",
            get(|| async { Json(json!({"id": "mock-model", "object": "model"})) }),
        )
        .route(
            "/deepseek/api/v1/models",
            get(|| async { Json(json!({"data": [{"id": "mock-model", "object": "model"}]})) }),
        )
        .route(
            "/v1/responses",
            post(move |headers: HeaderMap, body: axum::body::Bytes| {
                let ws = ws_clone1.clone();
                let cap = cap_clone1.clone();
                async move { handle_family_responses(family, headers, body, ws, cap).await }
            }),
        )
        .route(
            "/v1/chat/completions",
            post(move |headers: HeaderMap, body: axum::body::Bytes| {
                let ws = ws_clone2.clone();
                let cap = cap_clone2.clone();
                async move {
                    handle_family_chat(
                        family,
                        headers,
                        body,
                        "/v1/chat/completions".to_string(),
                        ws,
                        cap,
                    )
                    .await
                }
            }),
        )
        .route(
            "/chat/completions",
            post(move |headers: HeaderMap, body: axum::body::Bytes| {
                let ws = ws_clone3.clone();
                let cap = cap_clone3.clone();
                async move {
                    handle_family_chat(
                        family,
                        headers,
                        body,
                        "/chat/completions".to_string(),
                        ws,
                        cap,
                    )
                    .await
                }
            }),
        )
        .route(
            "/deepseek/chat/completions",
            post(move |headers: HeaderMap, body: axum::body::Bytes| {
                let ws = ws_clone4.clone();
                let cap = cap_clone4.clone();
                async move {
                    handle_family_chat(
                        family,
                        headers,
                        body,
                        "/deepseek/chat/completions".to_string(),
                        ws,
                        cap,
                    )
                    .await
                }
            }),
        )
        .route(
            "/deepseek/v1/chat/completions",
            post(move |headers: HeaderMap, body: axum::body::Bytes| {
                let ws = ws_clone5.clone();
                let cap = cap_clone5.clone();
                async move {
                    handle_family_chat(
                        family,
                        headers,
                        body,
                        "/deepseek/v1/chat/completions".to_string(),
                        ws,
                        cap,
                    )
                    .await
                }
            }),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    (format!("http://127.0.0.1:{port}"), task)
}

async fn handle_family_responses(
    family: TaskFamily,
    headers: HeaderMap,
    body: axum::body::Bytes,
    workspace: Arc<PathBuf>,
    captured: Arc<Mutex<Vec<CapturedRequest>>>,
) -> Response {
    let json_val = serde_json::from_slice::<Value>(&body).ok();

    captured.lock().unwrap().push(CapturedRequest {
        path: "/v1/responses".to_string(),
        headers: headers.clone(),
        body: body.to_vec(),
        json_body: json_val.clone(),
    });

    let (filename, content) = family_tool_action(family);
    let target_file = workspace.join(filename);

    let has_tool_output = json_val.as_ref().is_some_and(|payload| {
        payload
            .get("input")
            .and_then(Value::as_array)
            .is_some_and(|items| {
                items.iter().any(|item| {
                    item.get("type").and_then(Value::as_str) == Some("function_call_output")
                })
            })
    });

    let file_already_repaired = target_file.exists()
        && fs::read_to_string(&target_file)
            .map(|s| {
                s.contains("repaired by tool")
                    || s.contains("refactored by tool")
                    || s.contains("implemented by tool")
                    || s.contains("recovered by tool")
            })
            .unwrap_or(false);

    if !file_already_repaired && !has_tool_output {
        let call_id = format!(
            "call_{}",
            &uuid::Uuid::new_v4().to_string().replace('-', "")[..8]
        );
        let hex_content: String = content
            .as_bytes()
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect();
        let cmd = format!(
            "python3 -c \"import os; open('env_check.txt', 'w').write(os.environ.get('CW_CONTROL_TOKEN', 'ABSENT')); open('{}', 'wb').write(bytes.fromhex('{}'))\"",
            filename, hex_content
        );
        let args = serde_json::to_string(&json!({"cmd": cmd})).unwrap();

        let events = [
            json!({"type": "response.created", "response": {"id": format!("resp_{call_id}"), "status": "in_progress"}}),
            json!({"type": "response.output_item.added", "output_index": 0, "item": {"id": format!("fc_{call_id}"), "type": "function_call", "call_id": call_id, "name": "exec_command", "arguments": ""}}),
            json!({"type": "response.function_call_arguments.delta", "output_index": 0, "call_id": call_id, "delta": args}),
            json!({"type": "response.function_call_arguments.done", "output_index": 0, "call_id": call_id, "arguments": args}),
            json!({"type": "response.output_item.done", "output_index": 0, "item": {"id": format!("fc_{call_id}"), "type": "function_call", "call_id": call_id, "name": "exec_command", "arguments": args}}),
            json!({"type": "response.completed", "response": {"id": format!("resp_{call_id}"), "status": "completed", "output": [{"id": format!("fc_{call_id}"), "type": "function_call", "call_id": call_id, "name": "exec_command", "arguments": args}], "usage": {"input_tokens": 120, "output_tokens": 30, "total_tokens": 150}}}),
        ];

        let mut sse_body = String::new();
        for ev in events {
            sse_body.push_str(&format!(
                "data: {}\n\n",
                serde_json::to_string(&ev).unwrap()
            ));
        }
        sse_body.push_str("data: [DONE]\n\n");

        Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "text/event-stream")
            .header("cache-control", "no-cache")
            .body(Body::from(sse_body))
            .unwrap()
    } else {
        let text = format!(
            "Task {} step executed and verified successfully.",
            family.code()
        );
        let events = [
            json!({"type": "response.created", "response": {"id": "resp_done", "status": "in_progress"}}),
            json!({"type": "response.output_item.added", "output_index": 0, "item": {"id": "msg_done", "type": "message", "role": "assistant", "content": []}}),
            json!({"type": "response.content_part.added", "output_index": 0, "content_index": 0, "part": {"type": "output_text", "text": ""}}),
            json!({"type": "response.output_text.delta", "output_index": 0, "content_index": 0, "delta": text}),
            json!({"type": "response.output_text.done", "output_index": 0, "content_index": 0, "text": text}),
            json!({"type": "response.content_part.done", "output_index": 0, "content_index": 0, "part": {"type": "output_text", "text": text}}),
            json!({"type": "response.output_item.done", "output_index": 0, "item": {"id": "msg_done", "type": "message", "role": "assistant", "content": [{"type": "output_text", "text": text}]}}),
            json!({"type": "response.completed", "response": {"id": "resp_done", "status": "completed", "output": [{"id": "msg_done", "type": "message", "role": "assistant", "content": [{"type": "output_text", "text": text}]}], "usage": {"input_tokens": 120, "output_tokens": 30, "total_tokens": 150}}}),
        ];

        let mut sse_body = String::new();
        for ev in events {
            sse_body.push_str(&format!(
                "data: {}\n\n",
                serde_json::to_string(&ev).unwrap()
            ));
        }
        sse_body.push_str("data: [DONE]\n\n");

        Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "text/event-stream")
            .header("cache-control", "no-cache")
            .body(Body::from(sse_body))
            .unwrap()
    }
}

async fn handle_family_chat(
    family: TaskFamily,
    headers: HeaderMap,
    body: axum::body::Bytes,
    path: String,
    workspace: Arc<PathBuf>,
    captured: Arc<Mutex<Vec<CapturedRequest>>>,
) -> Response {
    let json_val = serde_json::from_slice::<Value>(&body).ok();
    let is_stream = json_val
        .as_ref()
        .and_then(|v| v.get("stream"))
        .and_then(Value::as_bool)
        .unwrap_or(true);

    captured.lock().unwrap().push(CapturedRequest {
        path,
        headers: headers.clone(),
        body: body.to_vec(),
        json_body: json_val.clone(),
    });

    let (filename, content) = family_tool_action(family);
    let target_file = workspace.join(filename);

    let has_tool_res = json_val.as_ref().is_some_and(|payload| {
        payload
            .get("messages")
            .and_then(Value::as_array)
            .is_some_and(|msgs| {
                msgs.iter()
                    .any(|m| m.get("role").and_then(Value::as_str) == Some("tool"))
            })
    });

    let file_already_repaired = target_file.exists()
        && fs::read_to_string(&target_file)
            .map(|s| {
                s.contains("repaired by tool")
                    || s.contains("refactored by tool")
                    || s.contains("implemented by tool")
                    || s.contains("recovered by tool")
            })
            .unwrap_or(false);

    let is_omp = json_val.as_ref().is_some_and(|p| {
        p.get("tools")
            .and_then(Value::as_array)
            .is_some_and(|tools| {
                tools.iter().any(|t| {
                    t.get("function")
                        .and_then(|f| f.get("name"))
                        .and_then(Value::as_str)
                        == Some("write")
                })
            })
    });

    if !file_already_repaired && !has_tool_res {
        let call_id = format!(
            "call_{}",
            &uuid::Uuid::new_v4().to_string().replace('-', "")[..8]
        );
        let hex_content: String = content
            .as_bytes()
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect();
        let cmd = format!(
            "python3 -c \"import os; open('env_check.txt', 'w').write(os.environ.get('CW_CONTROL_TOKEN', 'ABSENT')); open('{}', 'wb').write(bytes.fromhex('{}'))\"",
            filename, hex_content
        );

        let (tool_name, tc_args) = if is_omp {
            (
                "write",
                json!({
                    "path": filename,
                    "content": content,
                    "i": format!("repair {filename} for task"),
                })
                .to_string(),
            )
        } else {
            (
                "terminal",
                json!({
                    "command": cmd,
                })
                .to_string(),
            )
        };

        if is_stream {
            let chunks = [
                json!({
                    "id": format!("chatcmpl_{call_id}"),
                    "object": "chat.completion.chunk",
                    "created": Utc::now().timestamp(),
                    "model": "mock-model",
                    "choices": [{
                        "index": 0,
                        "delta": {
                            "role": "assistant",
                            "content": null,
                            "tool_calls": [{
                                "index": 0,
                                "id": call_id,
                                "type": "function",
                                "function": {
                                    "name": tool_name,
                                    "arguments": tc_args,
                                }
                            }]
                        },
                        "finish_reason": null
                    }]
                }),
                json!({
                    "id": format!("chatcmpl_{call_id}"),
                    "object": "chat.completion.chunk",
                    "created": Utc::now().timestamp(),
                    "model": "mock-model",
                    "choices": [{
                        "index": 0,
                        "delta": {},
                        "finish_reason": "tool_calls"
                    }]
                }),
            ];

            let mut sse_body = String::new();
            for c in chunks {
                sse_body.push_str(&format!("data: {}\n\n", serde_json::to_string(&c).unwrap()));
            }
            sse_body.push_str("data: [DONE]\n\n");

            Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "text/event-stream")
                .header("cache-control", "no-cache")
                .body(Body::from(sse_body))
                .unwrap()
        } else {
            Json(json!({
                "id": format!("chatcmpl_{call_id}"),
                "object": "chat.completion",
                "created": Utc::now().timestamp(),
                "model": "mock-model",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [{
                            "id": call_id,
                            "type": "function",
                            "function": {
                                "name": tool_name,
                                "arguments": tc_args,
                            }
                        }]
                    },
                    "finish_reason": "tool_calls"
                }],
                "usage": {"prompt_tokens": 120, "completion_tokens": 30, "total_tokens": 150}
            }))
            .into_response()
        }
    } else {
        let msg = format!(
            "Task {} step executed and verified successfully.",
            family.code()
        );
        if is_stream {
            let chunks = [
                json!({
                    "id": "chatcmpl_done",
                    "object": "chat.completion.chunk",
                    "created": Utc::now().timestamp(),
                    "model": "mock-model",
                    "choices": [{
                        "index": 0,
                        "delta": {
                            "role": "assistant",
                            "content": msg
                        },
                        "finish_reason": null
                    }]
                }),
                json!({
                    "id": "chatcmpl_done",
                    "object": "chat.completion.chunk",
                    "created": Utc::now().timestamp(),
                    "model": "mock-model",
                    "choices": [{
                        "index": 0,
                        "delta": {},
                        "finish_reason": "stop"
                    }],
                    "usage": {"prompt_tokens": 120, "completion_tokens": 30, "total_tokens": 150}
                }),
            ];

            let mut sse_body = String::new();
            for c in chunks {
                sse_body.push_str(&format!("data: {}\n\n", serde_json::to_string(&c).unwrap()));
            }
            sse_body.push_str("data: [DONE]\n\n");

            Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "text/event-stream")
                .header("cache-control", "no-cache")
                .body(Body::from(sse_body))
                .unwrap()
        } else {
            Json(json!({
                "id": "chatcmpl_done",
                "object": "chat.completion",
                "created": Utc::now().timestamp(),
                "model": "mock-model",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": msg
                    },
                    "finish_reason": "stop"
                }],
                "usage": {"prompt_tokens": 120, "completion_tokens": 30, "total_tokens": 150}
            }))
            .into_response()
        }
    }
}

// ============================================================================
// 6. Harness Session Execution Engines (Codex, OMP, Hermes)
// ============================================================================

fn verify_captured_credentials_isolated(captured: &[CapturedRequest], control_token: &str) -> bool {
    for req in captured {
        if req.headers.contains_key("x-cw-control-token") {
            return false;
        }
        for (name, val) in &req.headers {
            if val
                .as_bytes()
                .windows(control_token.len())
                .any(|w| w == control_token.as_bytes())
            {
                return false;
            }
            if name.as_str().to_lowercase().contains("control-token") {
                return false;
            }
        }
        if req
            .body
            .windows(control_token.len())
            .any(|w| w == control_token.as_bytes())
        {
            return false;
        }
    }
    true
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolPairingError {
    NoToolCallsObserved,
    DuplicateEmittedCallId {
        call_id: String,
        request_index: usize,
    },
    DuplicateResult {
        call_id: String,
        request_index: usize,
    },
    OrphanedResult {
        call_id: String,
        request_index: usize,
    },
    MissingResult {
        call_id: String,
        request_index: usize,
    },
    MultiplicityMismatch {
        call_id: String,
        emitted_count: usize,
        result_count: usize,
        request_index: usize,
    },
}

impl std::fmt::Display for ToolPairingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoToolCallsObserved => {
                write!(f, "No tool calls were observed in captured requests")
            }
            Self::DuplicateEmittedCallId {
                call_id,
                request_index,
            } => {
                write!(
                    f,
                    "Duplicate emitted tool call ID '{}' in request {}",
                    call_id, request_index
                )
            }
            Self::DuplicateResult {
                call_id,
                request_index,
            } => {
                write!(
                    f,
                    "Duplicate tool result for call ID '{}' in request {}",
                    call_id, request_index
                )
            }
            Self::OrphanedResult {
                call_id,
                request_index,
            } => {
                write!(
                    f,
                    "Orphaned tool result for uncalled ID '{}' in request {}",
                    call_id, request_index
                )
            }
            Self::MissingResult {
                call_id,
                request_index,
            } => {
                write!(
                    f,
                    "Missing tool result for emitted call ID '{}' in request {}",
                    call_id, request_index
                )
            }
            Self::MultiplicityMismatch {
                call_id,
                emitted_count,
                result_count,
                request_index,
            } => {
                write!(
                    f,
                    "Multiplicity mismatch for call ID '{}' in request {}: {} emitted vs {} results",
                    call_id, request_index, emitted_count, result_count
                )
            }
        }
    }
}

fn verify_captured_tool_pairing_with_error(
    captured: &[CapturedRequest],
) -> Result<(), ToolPairingError> {
    let mut session_resolved_calls = HashSet::new();

    for (req_idx, req) in captured.iter().enumerate() {
        if let Some(ref json) = req.json_body {
            let mut req_emitted: HashMap<String, usize> = HashMap::new();
            let mut req_results: HashMap<String, usize> = HashMap::new();

            // Responses API format (Codex)
            if let Some(input) = json.get("input").and_then(Value::as_array) {
                for item in input {
                    if item.get("type").and_then(Value::as_str) == Some("function_call") {
                        if let Some(cid) = item.get("call_id").and_then(Value::as_str) {
                            let count = req_emitted.entry(cid.to_string()).or_insert(0);
                            *count += 1;
                            if *count > 1 {
                                return Err(ToolPairingError::DuplicateEmittedCallId {
                                    call_id: cid.to_string(),
                                    request_index: req_idx,
                                });
                            }
                        }
                    } else if item.get("type").and_then(Value::as_str)
                        == Some("function_call_output")
                    {
                        if let Some(cid) = item.get("call_id").and_then(Value::as_str) {
                            let count = req_results.entry(cid.to_string()).or_insert(0);
                            *count += 1;
                            if *count > 1 {
                                return Err(ToolPairingError::DuplicateResult {
                                    call_id: cid.to_string(),
                                    request_index: req_idx,
                                });
                            }
                        }
                    }
                }
            }

            // Chat Completions format (OMP and Hermes)
            if let Some(messages) = json.get("messages").and_then(Value::as_array) {
                for msg in messages {
                    if let Some(tcs) = msg.get("tool_calls").and_then(Value::as_array) {
                        for tc in tcs {
                            if let Some(id) = tc.get("id").and_then(Value::as_str) {
                                let count = req_emitted.entry(id.to_string()).or_insert(0);
                                *count += 1;
                                if *count > 1 {
                                    return Err(ToolPairingError::DuplicateEmittedCallId {
                                        call_id: id.to_string(),
                                        request_index: req_idx,
                                    });
                                }
                            }
                        }
                    }
                    if msg.get("role").and_then(Value::as_str) == Some("tool") {
                        if let Some(id) = msg.get("tool_call_id").and_then(Value::as_str) {
                            let count = req_results.entry(id.to_string()).or_insert(0);
                            *count += 1;
                            if *count > 1 {
                                return Err(ToolPairingError::DuplicateResult {
                                    call_id: id.to_string(),
                                    request_index: req_idx,
                                });
                            }
                        }
                    }
                }
            }

            // If this request has tool interactions, validate bijective ledger properties
            if !req_emitted.is_empty() || !req_results.is_empty() {
                // 1. Every result must match an emitted call (no orphans)
                for cid in req_results.keys() {
                    if !req_emitted.contains_key(cid) {
                        return Err(ToolPairingError::OrphanedResult {
                            call_id: cid.clone(),
                            request_index: req_idx,
                        });
                    }
                }

                // 2. Every emitted call must have a matching result with identical multiplicity (no missing results or multiplicity mismatch)
                for (cid, emitted_count) in &req_emitted {
                    let res_count = req_results.get(cid).copied().unwrap_or(0);
                    if res_count == 0 {
                        return Err(ToolPairingError::MissingResult {
                            call_id: cid.clone(),
                            request_index: req_idx,
                        });
                    }
                    if res_count != *emitted_count {
                        return Err(ToolPairingError::MultiplicityMismatch {
                            call_id: cid.clone(),
                            emitted_count: *emitted_count,
                            result_count: res_count,
                            request_index: req_idx,
                        });
                    }
                    session_resolved_calls.insert(cid.clone());
                }
            }
        }
    }

    if session_resolved_calls.is_empty() {
        return Err(ToolPairingError::NoToolCallsObserved);
    }

    Ok(())
}

fn verify_captured_tool_pairing(captured: &[CapturedRequest]) -> bool {
    match verify_captured_tool_pairing_with_error(captured) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("Tool pairing ledger validation failed: {e}");
            false
        }
    }
}

async fn execute_codex_session(
    codex_bin: &Path,
    proxy_port: u16,
    control_token: &str,
    cw_session_id: &str,
    fixture: &WorkloadFixture,
    store: &CwStore,
    family: TaskFamily,
    repetition: usize,
    captured: Arc<Mutex<Vec<CapturedRequest>>>,
) -> SessionQualificationResult {
    // Assert initial state fails acceptance tests (proving test harness sensitivity)
    assert!(
        !fixture.verify_acceptance_tests(),
        "Fixture for {:?} must initially fail acceptance tests",
        family
    );

    let temp_home = std::env::temp_dir().join(format!("codex_home_{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp_home).unwrap();

    let proxy_args = cw_harnesses::qualification::codex_proxy_config_args(proxy_port);

    let mut thread_id = String::new();
    let mut logical_turns = 0;

    // Checkpoint 1..4: Initial session
    for turn_idx in 1..=4 {
        let codex_bin = codex_bin.to_path_buf();
        let ws = fixture.workspace_dir.clone();
        let th = temp_home.clone();
        let sid = cw_session_id.to_string();
        let tok = control_token.to_string();
        let pargs = proxy_args.clone();
        let tid_clone = thread_id.clone();

        let output = match tokio::time::timeout(
            Duration::from_secs(120),
            tokio::task::spawn_blocking(move || {
                let mut cmd = Command::new(codex_bin);
                if turn_idx == 1 {
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
                    cmd.args(&pargs);
                    cmd.args([
                        "-m",
                        "mock-model",
                        &format!("Turn {} initial prompt for {}", turn_idx, family.code()),
                    ]);
                } else {
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
                    cmd.args(&pargs);
                    cmd.args([
                        "-m",
                        "mock-model",
                        &tid_clone,
                        &format!("Turn {} follow-up prompt", turn_idx),
                    ]);
                }

                cmd.current_dir(&ws);
                cmd.env("CODEX_HOME", &th);
                cmd.env("OPENAI_API_KEY", "mock-openai-key");
                cmd.env("CW_SESSION_ID", &sid);
                cmd.env("CW_CONTROL_TOKEN", &tok);
                cmd.output()
            }),
        )
        .await
        {
            Ok(Ok(Ok(out))) => out,
            Ok(Ok(Err(e))) => {
                let _ = fs::remove_dir_all(&temp_home);
                return SessionQualificationResult {
                    session_id: cw_session_id.to_string(),
                    harness: "Codex CLI".to_string(),
                    task_family: family.name().to_string(),
                    repetition,
                    status: "FAIL".to_string(),
                    logical_turns,
                    model_requests: store.list_turns(cw_session_id).unwrap_or_default().len(),
                    resume_preserved: false,
                    overlay_applied: false,
                    overlay_reapplied_across_resume: false,
                    tool_pairing_intact: false,
                    transcripts_unmodified: true,
                    credentials_isolated: false,
                    acceptance_test_passed: false,
                    stream_completion_clean: false,
                    protocol_reliability: false,
                    agentic_reliability: false,
                    failure_class: Some(format!("Failed to spawn codex on turn {turn_idx}: {e}")),
                };
            }
            _ => {
                let _ = fs::remove_dir_all(&temp_home);
                return SessionQualificationResult {
                    session_id: cw_session_id.to_string(),
                    harness: "Codex CLI".to_string(),
                    task_family: family.name().to_string(),
                    repetition,
                    status: "FAIL".to_string(),
                    logical_turns,
                    model_requests: store.list_turns(cw_session_id).unwrap_or_default().len(),
                    resume_preserved: false,
                    overlay_applied: false,
                    overlay_reapplied_across_resume: false,
                    tool_pairing_intact: false,
                    transcripts_unmodified: true,
                    credentials_isolated: false,
                    acceptance_test_passed: false,
                    stream_completion_clean: false,
                    protocol_reliability: false,
                    agentic_reliability: false,
                    failure_class: Some(format!("Codex turn {turn_idx} timed out after 120s")),
                };
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("Turn {} stdout:\n{}\nstderr:\n{}", turn_idx, stdout, stderr);
        if !output.status.success() {
            let _ = fs::remove_dir_all(&temp_home);
            return SessionQualificationResult {
                session_id: cw_session_id.to_string(),
                harness: "Codex CLI".to_string(),
                task_family: family.name().to_string(),
                repetition,
                status: "FAIL".to_string(),
                logical_turns,
                model_requests: store.list_turns(cw_session_id).unwrap_or_default().len(),
                resume_preserved: false,
                overlay_applied: false,
                overlay_reapplied_across_resume: false,
                tool_pairing_intact: false,
                transcripts_unmodified: true,
                credentials_isolated: false,
                acceptance_test_passed: false,
                stream_completion_clean: false,
                protocol_reliability: false,
                agentic_reliability: false,
                failure_class: Some(format!(
                    "Codex turn {turn_idx} failed: stderr={stderr}, stdout={stdout}"
                )),
            };
        }

        if turn_idx == 1 {
            for line in stdout.lines() {
                if let Ok(val) = serde_json::from_str::<Value>(line)
                    && val.get("type").and_then(Value::as_str) == Some("thread.started")
                    && let Some(tid) = val.get("thread_id").and_then(Value::as_str)
                {
                    thread_id = tid.to_string();
                    break;
                }
            }
            if thread_id.is_empty() {
                let _ = fs::remove_dir_all(&temp_home);
                return SessionQualificationResult {
                    session_id: cw_session_id.to_string(),
                    harness: "Codex CLI".to_string(),
                    task_family: family.name().to_string(),
                    repetition,
                    status: "FAIL".to_string(),
                    logical_turns,
                    model_requests: store.list_turns(cw_session_id).unwrap_or_default().len(),
                    resume_preserved: false,
                    overlay_applied: false,
                    overlay_reapplied_across_resume: false,
                    tool_pairing_intact: false,
                    transcripts_unmodified: true,
                    credentials_isolated: false,
                    acceptance_test_passed: false,
                    stream_completion_clean: false,
                    protocol_reliability: false,
                    agentic_reliability: false,
                    failure_class: Some(
                        "Failed to extract thread_id from Codex turn 1".to_string(),
                    ),
                };
            }

            // Assert that harness tool execution passed the acceptance tests
            if !fixture.verify_acceptance_tests() {
                let _ = fs::remove_dir_all(&temp_home);
                return SessionQualificationResult {
                    session_id: cw_session_id.to_string(),
                    harness: "Codex CLI".to_string(),
                    task_family: family.name().to_string(),
                    repetition,
                    status: "FAIL".to_string(),
                    logical_turns,
                    model_requests: store.list_turns(cw_session_id).unwrap_or_default().len(),
                    resume_preserved: false,
                    overlay_applied: false,
                    overlay_reapplied_across_resume: false,
                    tool_pairing_intact: false,
                    transcripts_unmodified: true,
                    credentials_isolated: false,
                    acceptance_test_passed: false,
                    stream_completion_clean: false,
                    protocol_reliability: false,
                    agentic_reliability: false,
                    failure_class: Some(
                        "Acceptance tests failed after Codex turn 1 tool execution".to_string(),
                    ),
                };
            }
        } else {
            let mut turn_thread_id = String::new();
            for line in stdout.lines() {
                if let Ok(val) = serde_json::from_str::<Value>(line)
                    && val.get("type").and_then(Value::as_str) == Some("thread.started")
                    && let Some(tid) = val.get("thread_id").and_then(Value::as_str)
                {
                    turn_thread_id = tid.to_string();
                    break;
                }
            }
            if turn_thread_id != thread_id {
                let _ = fs::remove_dir_all(&temp_home);
                return SessionQualificationResult {
                    session_id: cw_session_id.to_string(),
                    harness: "Codex CLI".to_string(),
                    task_family: family.name().to_string(),
                    repetition,
                    status: "FAIL".to_string(),
                    logical_turns,
                    model_requests: store.list_turns(cw_session_id).unwrap_or_default().len(),
                    resume_preserved: false,
                    overlay_applied: false,
                    overlay_reapplied_across_resume: false,
                    tool_pairing_intact: false,
                    transcripts_unmodified: true,
                    credentials_isolated: false,
                    acceptance_test_passed: false,
                    stream_completion_clean: false,
                    protocol_reliability: false,
                    agentic_reliability: false,
                    failure_class: Some(format!(
                        "Codex initial resume turn {turn_idx} thread_id mismatch: expected {thread_id}, got {turn_thread_id}"
                    )),
                };
            }
        }

        logical_turns += 1;
    }

    // Commit context overlay rule to ContextWatcher store before checkpoint 5
    let pre_mutation_count = store.list_turns(cw_session_id).unwrap_or_default().len();
    store
        .save_mutation(&MutationRule {
            id: format!("mut_{cw_session_id}"),
            session_id: cw_session_id.to_string(),
            op: MutationOp::PinContext {
                label: "Policy".to_string(),
                content: "CW114_POLICY_ACTIVE".to_string(),
                source_node_id: None,
            },
            status: MutationStatus::Committed,
            estimated_saved_tokens: 0,
            estimated_kv_break_tokens: 0,
            created_at: Utc::now(),
        })
        .unwrap();

    // Checkpoint 5..8: Resumed session with active overlay reapplication
    for turn_idx in 5..=8 {
        let codex_bin = codex_bin.to_path_buf();
        let ws = fixture.workspace_dir.clone();
        let th = temp_home.clone();
        let sid = cw_session_id.to_string();
        let tok = control_token.to_string();
        let pargs = proxy_args.clone();
        let tid_clone = thread_id.clone();

        let output = match tokio::time::timeout(
            Duration::from_secs(120),
            tokio::task::spawn_blocking(move || {
                let mut cmd = Command::new(codex_bin);
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
                cmd.args(&pargs);
                cmd.args([
                    "-m",
                    "mock-model",
                    &tid_clone,
                    &format!("Turn {} prompt after overlay commitment", turn_idx),
                ]);

                cmd.current_dir(&ws);
                cmd.env("CODEX_HOME", &th);
                cmd.env("OPENAI_API_KEY", "mock-openai-key");
                cmd.env("CW_SESSION_ID", &sid);
                cmd.env("CW_CONTROL_TOKEN", &tok);
                cmd.output()
            }),
        )
        .await
        {
            Ok(Ok(Ok(out))) => out,
            Ok(Ok(Err(e))) => {
                let _ = fs::remove_dir_all(&temp_home);
                return SessionQualificationResult {
                    session_id: cw_session_id.to_string(),
                    harness: "Codex CLI".to_string(),
                    task_family: family.name().to_string(),
                    repetition,
                    status: "FAIL".to_string(),
                    logical_turns,
                    model_requests: store.list_turns(cw_session_id).unwrap_or_default().len(),
                    resume_preserved: false,
                    overlay_applied: false,
                    overlay_reapplied_across_resume: false,
                    tool_pairing_intact: false,
                    transcripts_unmodified: true,
                    credentials_isolated: false,
                    acceptance_test_passed: false,
                    stream_completion_clean: false,
                    protocol_reliability: false,
                    agentic_reliability: false,
                    failure_class: Some(format!(
                        "Failed to spawn resumed codex on turn {turn_idx}: {e}"
                    )),
                };
            }
            _ => {
                let _ = fs::remove_dir_all(&temp_home);
                return SessionQualificationResult {
                    session_id: cw_session_id.to_string(),
                    harness: "Codex CLI".to_string(),
                    task_family: family.name().to_string(),
                    repetition,
                    status: "FAIL".to_string(),
                    logical_turns,
                    model_requests: store.list_turns(cw_session_id).unwrap_or_default().len(),
                    resume_preserved: false,
                    overlay_applied: false,
                    overlay_reapplied_across_resume: false,
                    tool_pairing_intact: false,
                    transcripts_unmodified: true,
                    credentials_isolated: false,
                    acceptance_test_passed: false,
                    stream_completion_clean: false,
                    protocol_reliability: false,
                    agentic_reliability: false,
                    failure_class: Some(format!(
                        "Codex resume turn {turn_idx} timed out after 120s"
                    )),
                };
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("Turn {} stdout:\n{}\nstderr:\n{}", turn_idx, stdout, stderr);
        if !output.status.success() {
            let _ = fs::remove_dir_all(&temp_home);
            return SessionQualificationResult {
                session_id: cw_session_id.to_string(),
                harness: "Codex CLI".to_string(),
                task_family: family.name().to_string(),
                repetition,
                status: "FAIL".to_string(),
                logical_turns,
                model_requests: store.list_turns(cw_session_id).unwrap_or_default().len(),
                resume_preserved: false,
                overlay_applied: false,
                overlay_reapplied_across_resume: false,
                tool_pairing_intact: false,
                transcripts_unmodified: true,
                credentials_isolated: false,
                acceptance_test_passed: false,
                stream_completion_clean: false,
                protocol_reliability: false,
                agentic_reliability: false,
                failure_class: Some(format!(
                    "Codex resume turn {turn_idx} failed: stderr={stderr}, stdout={stdout}"
                )),
            };
        }

        let mut turn_thread_id = String::new();
        for line in stdout.lines() {
            if let Ok(val) = serde_json::from_str::<Value>(line)
                && val.get("type").and_then(Value::as_str) == Some("thread.started")
                && let Some(tid) = val.get("thread_id").and_then(Value::as_str)
            {
                turn_thread_id = tid.to_string();
                break;
            }
        }
        if turn_thread_id != thread_id {
            let _ = fs::remove_dir_all(&temp_home);
            return SessionQualificationResult {
                session_id: cw_session_id.to_string(),
                harness: "Codex CLI".to_string(),
                task_family: family.name().to_string(),
                repetition,
                status: "FAIL".to_string(),
                logical_turns,
                model_requests: store.list_turns(cw_session_id).unwrap_or_default().len(),
                resume_preserved: false,
                overlay_applied: false,
                overlay_reapplied_across_resume: false,
                tool_pairing_intact: false,
                transcripts_unmodified: true,
                credentials_isolated: false,
                acceptance_test_passed: false,
                stream_completion_clean: false,
                protocol_reliability: false,
                agentic_reliability: false,
                failure_class: Some(format!(
                    "Codex post-mutation resume turn {turn_idx} thread_id mismatch: expected {thread_id}, got {turn_thread_id}"
                )),
            };
        }

        logical_turns += 1;
    }

    let all_sessions = store.list_sessions().unwrap_or_default();
    eprintln!(
        "All sessions in store: {:?}",
        all_sessions.iter().map(|s| &s.id).collect::<Vec<_>>()
    );
    for s in &all_sessions {
        let sturns = store.list_turns(&s.id).unwrap_or_default();
        eprintln!("Session {}: {} turns", s.id, sturns.len());
    }
    let turns = store.list_turns(cw_session_id).unwrap_or_default();
    eprintln!(
        "Turns for cw_session_id {}: count={}",
        cw_session_id,
        turns.len()
    );
    {
        let cap = captured.lock().unwrap();
        for (i, req) in cap.iter().enumerate() {
            eprintln!("Mock captured req {}: headers={:?}", i, req.headers);
        }
    }
    let model_requests = turns.len();
    let resume_preserved = !thread_id.is_empty() && logical_turns == 8;

    // Verify overlay presence:
    // 1. Pre-mutation turns must NOT contain CW114_POLICY_ACTIVE
    let pre_mutation_clean = turns.iter().take(pre_mutation_count).all(|t| {
        store
            .get_turn_mutated_payload(&t.id)
            .unwrap()
            .map(|p| {
                !serde_json::to_string(&p)
                    .unwrap_or_default()
                    .contains("CW114_POLICY_ACTIVE")
            })
            .unwrap_or(true)
    });

    // 2. Post-mutation turns (5..8) must each individually contain CW114_POLICY_ACTIVE across process restarts
    let post_mutation_turns: Vec<_> = turns.iter().skip(pre_mutation_count).collect();
    let overlay_reapplied_across_resume = post_mutation_turns.len() == 4
        && post_mutation_turns.iter().all(|t| {
            store
                .get_turn_mutated_payload(&t.id)
                .unwrap()
                .map(|p| {
                    serde_json::to_string(&p)
                        .unwrap_or_default()
                        .contains("CW114_POLICY_ACTIVE")
                })
                .unwrap_or(false)
        });

    let overlay_applied = pre_mutation_clean && overlay_reapplied_across_resume;

    let stream_completion_clean = turns
        .iter()
        .all(|t| t.outcome.as_deref() == Some("completed"));

    // Check rollout immutability
    let mut transcripts_unmodified = true;
    if let Ok(entries) = fs::read_dir(temp_home.join("sessions")) {
        for entry in entries.flatten() {
            if entry.path().is_file() {
                let content = fs::read_to_string(entry.path()).unwrap_or_default();
                if content.contains("CW114_POLICY_ACTIVE") {
                    transcripts_unmodified = false;
                }
            }
        }
    }

    // Check credential isolation:
    // 1. Tool-level sandboxing: env_check.txt was written by tool and has "ABSENT"
    let env_check_path = fixture.workspace_dir.join("env_check.txt");
    let env_check_content = fs::read_to_string(&env_check_path).unwrap_or_default();
    let tool_sandbox_clean = env_check_content.trim() == "ABSENT";

    // 2. Proxy wire-level stripping: upstream captured requests have no control token
    let cap_guard = captured.lock().unwrap();
    let proxy_wire_clean = verify_captured_credentials_isolated(&cap_guard, control_token);
    let credentials_isolated = tool_sandbox_clean && proxy_wire_clean;

    // Verify tool pairing in captured requests
    let tool_pairing_intact = verify_captured_tool_pairing(&cap_guard);
    drop(cap_guard);

    let acceptance_test_passed = fixture.verify_acceptance_tests();

    let _ = fs::remove_dir_all(&temp_home);

    let protocol_reliability = resume_preserved
        && overlay_applied
        && credentials_isolated
        && transcripts_unmodified
        && stream_completion_clean;
    let agentic_reliability = tool_pairing_intact && acceptance_test_passed && logical_turns == 8;

    eprintln!(
        "DEBUG CODEX: resume_preserved={resume_preserved}, overlay_applied={overlay_applied} (pre_clean={pre_mutation_clean}, post_reapplied={overlay_reapplied_across_resume}), credentials_isolated={credentials_isolated} (tool={tool_sandbox_clean}, wire={proxy_wire_clean}), transcripts_unmodified={transcripts_unmodified}, stream_clean={stream_completion_clean}, tool_pairing={tool_pairing_intact}, acceptance={acceptance_test_passed}"
    );

    let failure_class = if !(protocol_reliability && agentic_reliability) {
        Some(format!(
            "protocol_reliability={protocol_reliability} (resume={resume_preserved}, overlay={overlay_applied}, creds={credentials_isolated}, transcripts={transcripts_unmodified}, stream={stream_completion_clean}), agentic_reliability={agentic_reliability} (pairing={tool_pairing_intact}, acceptance={acceptance_test_passed})"
        ))
    } else {
        None
    };

    SessionQualificationResult {
        session_id: cw_session_id.to_string(),
        harness: "Codex CLI".to_string(),
        task_family: family.name().to_string(),
        repetition,
        status: if protocol_reliability && agentic_reliability {
            "PASS".to_string()
        } else {
            "FAIL".to_string()
        },
        logical_turns,
        model_requests,
        resume_preserved,
        overlay_applied,
        overlay_reapplied_across_resume: overlay_applied,
        tool_pairing_intact,
        transcripts_unmodified,
        credentials_isolated,
        acceptance_test_passed,
        stream_completion_clean,
        protocol_reliability,
        agentic_reliability,
        failure_class,
    }
}

fn find_jsonl_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(find_jsonl_files(&path));
            } else if path.extension().and_then(|s| s.to_str()) == Some("jsonl") {
                files.push(path);
            }
        }
    }
    files
}

async fn execute_omp_session(
    omp_bin: &Path,
    proxy_port: u16,
    control_token: &str,
    cw_session_id: &str,
    fixture: &WorkloadFixture,
    store: &CwStore,
    family: TaskFamily,
    repetition: usize,
    captured: Arc<Mutex<Vec<CapturedRequest>>>,
) -> SessionQualificationResult {
    // Assert initial state fails acceptance tests (proving test harness sensitivity)
    assert!(
        !fixture.verify_acceptance_tests(),
        "Fixture for {:?} must initially fail acceptance tests",
        family
    );

    let temp_agent_dir = std::env::temp_dir().join(format!("omp_agent_{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp_agent_dir).unwrap();

    let token_dir = std::env::temp_dir().join(format!("omp_token_{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&token_dir).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&token_dir, fs::Permissions::from_mode(0o700));
    }
    let token_file = token_dir.join("control-token");
    fs::write(&token_file, format!("{}\n", control_token)).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&token_file, fs::Permissions::from_mode(0o600));
    }

    let models_cfg = cw_harnesses::qualification::omp_proxy_models_config(proxy_port, "mock-model");
    fs::write(
        temp_agent_dir.join("models.json"),
        serde_json::to_string_pretty(&models_cfg).unwrap(),
    )
    .unwrap();

    let ext_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("plugins/pi-opencode/index.js");

    let mut logical_turns = 0;
    let mut native_sid = String::new();

    // Checkpoints 1..4
    for turn_idx in 1..=4 {
        let omp_bin = omp_bin.to_path_buf();
        let ws = fixture.workspace_dir.clone();
        let th = temp_agent_dir.clone();
        let tf = token_file.clone();
        let ep = ext_path.clone();
        let sid = cw_session_id.to_string();
        let nsid_clone = native_sid.clone();

        let output = match tokio::time::timeout(
            Duration::from_secs(120),
            tokio::task::spawn_blocking(move || {
                let mut cmd = Command::new(omp_bin);
                if turn_idx == 1 {
                    cmd.args([
                        "-p",
                        &format!("Turn {} initial prompt for {}", turn_idx, family.code()),
                        "--model",
                        "cw-proxy/mock-model",
                        "-e",
                        &ep.to_string_lossy(),
                    ]);
                } else {
                    cmd.args([
                        "-r",
                        &nsid_clone,
                        "-p",
                        &format!("Turn {} follow-up prompt", turn_idx),
                        "--model",
                        "cw-proxy/mock-model",
                        "-e",
                        &ep.to_string_lossy(),
                    ]);
                }

                cmd.current_dir(&ws);
                cmd.env("PI_CODING_AGENT_DIR", &th);
                cmd.env("CW_CONTROL_TOKEN_FILE", &tf);
                cmd.env("CW_DAEMON_URL", format!("http://127.0.0.1:{proxy_port}"));
                cmd.env("CW_SESSION_ID", &sid);
                cmd.output()
            }),
        )
        .await
        {
            Ok(Ok(Ok(out))) => out,
            Ok(Ok(Err(e))) => {
                let _ = fs::remove_dir_all(&temp_agent_dir);
                let _ = fs::remove_dir_all(&token_dir);
                return SessionQualificationResult {
                    session_id: cw_session_id.to_string(),
                    harness: "OMP".to_string(),
                    task_family: family.name().to_string(),
                    repetition,
                    status: "FAIL".to_string(),
                    logical_turns,
                    model_requests: 0,
                    resume_preserved: false,
                    overlay_applied: false,
                    overlay_reapplied_across_resume: false,
                    tool_pairing_intact: false,
                    transcripts_unmodified: true,
                    credentials_isolated: false,
                    acceptance_test_passed: false,
                    stream_completion_clean: false,
                    protocol_reliability: false,
                    agentic_reliability: false,
                    failure_class: Some(format!("Failed to spawn OMP on turn {turn_idx}: {e}")),
                };
            }
            _ => {
                let _ = fs::remove_dir_all(&temp_agent_dir);
                let _ = fs::remove_dir_all(&token_dir);
                return SessionQualificationResult {
                    session_id: cw_session_id.to_string(),
                    harness: "OMP".to_string(),
                    task_family: family.name().to_string(),
                    repetition,
                    status: "FAIL".to_string(),
                    logical_turns,
                    model_requests: 0,
                    resume_preserved: false,
                    overlay_applied: false,
                    overlay_reapplied_across_resume: false,
                    tool_pairing_intact: false,
                    transcripts_unmodified: true,
                    credentials_isolated: false,
                    acceptance_test_passed: false,
                    stream_completion_clean: false,
                    protocol_reliability: false,
                    agentic_reliability: false,
                    failure_class: Some(format!("OMP turn {turn_idx} timed out after 120s")),
                };
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("Turn {} stdout:\n{}\nstderr:\n{}", turn_idx, stdout, stderr);
        if !output.status.success() {
            let _ = fs::remove_dir_all(&temp_agent_dir);
            let _ = fs::remove_dir_all(&token_dir);
            return SessionQualificationResult {
                session_id: cw_session_id.to_string(),
                harness: "OMP".to_string(),
                task_family: family.name().to_string(),
                repetition,
                status: "FAIL".to_string(),
                logical_turns,
                model_requests: 0,
                resume_preserved: false,
                overlay_applied: false,
                overlay_reapplied_across_resume: false,
                tool_pairing_intact: false,
                transcripts_unmodified: true,
                credentials_isolated: false,
                acceptance_test_passed: false,
                stream_completion_clean: false,
                protocol_reliability: false,
                agentic_reliability: false,
                failure_class: Some(format!(
                    "OMP turn {turn_idx} failed: stderr={stderr}, stdout={stdout}"
                )),
            };
        }

        if turn_idx == 1 {
            // Extract native session ID from session files or stdout
            let session_dir = temp_agent_dir.join("sessions");
            if session_dir.exists() {
                let session_files = find_jsonl_files(&session_dir);
                if let Some(session_file) = session_files.first() {
                    if let Ok(content) = fs::read_to_string(session_file) {
                        if let Some(first_line) = content.lines().next() {
                            if let Ok(val) = serde_json::from_str::<Value>(first_line) {
                                if let Some(sid) = val.get("id").and_then(Value::as_str) {
                                    native_sid = sid.to_string();
                                }
                            }
                        }
                    }
                    if native_sid.is_empty() {
                        let stem = session_file.file_stem().unwrap().to_str().unwrap();
                        if let Some(uuid_part) = stem.split('_').nth(1) {
                            native_sid = uuid_part.to_string();
                        }
                    }
                }
            }
            if native_sid.is_empty()
                && let Some(pos) = stdout.find("session: ")
            {
                native_sid = stdout[pos + 9..]
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_string();
            }
            // CRITICAL: Do NOT invent an ID fallback. Fail qualification if native session ID cannot be recovered.
            if native_sid.is_empty() {
                let _ = fs::remove_dir_all(&temp_agent_dir);
                let _ = fs::remove_dir_all(&token_dir);
                return SessionQualificationResult {
                    session_id: cw_session_id.to_string(),
                    harness: "OMP".to_string(),
                    task_family: family.name().to_string(),
                    repetition,
                    status: "FAIL".to_string(),
                    logical_turns,
                    model_requests: 0,
                    resume_preserved: false,
                    overlay_applied: false,
                    overlay_reapplied_across_resume: false,
                    tool_pairing_intact: false,
                    transcripts_unmodified: true,
                    credentials_isolated: false,
                    acceptance_test_passed: false,
                    stream_completion_clean: false,
                    protocol_reliability: false,
                    agentic_reliability: false,
                    failure_class: Some(format!(
                        "Failed to recover native OMP session ID from session files or stdout:\n{stdout}"
                    )),
                };
            }

            // Assert that harness tool execution passed the acceptance tests
            if !fixture.verify_acceptance_tests() {
                let _ = fs::remove_dir_all(&temp_agent_dir);
                let _ = fs::remove_dir_all(&token_dir);
                return SessionQualificationResult {
                    session_id: cw_session_id.to_string(),
                    harness: "OMP".to_string(),
                    task_family: family.name().to_string(),
                    repetition,
                    status: "FAIL".to_string(),
                    logical_turns,
                    model_requests: 0,
                    resume_preserved: false,
                    overlay_applied: false,
                    overlay_reapplied_across_resume: false,
                    tool_pairing_intact: false,
                    transcripts_unmodified: true,
                    credentials_isolated: false,
                    acceptance_test_passed: false,
                    stream_completion_clean: false,
                    protocol_reliability: false,
                    agentic_reliability: false,
                    failure_class: Some(
                        "Acceptance tests failed after OMP turn 1 tool execution".to_string(),
                    ),
                };
            }
        } else {
            let session_dir = temp_agent_dir.join("sessions");
            let mut found_session_file = false;
            if session_dir.exists() {
                for f in find_jsonl_files(&session_dir) {
                    if let Some(stem) = f.file_stem().and_then(|s| s.to_str()) {
                        if stem.contains(&native_sid) {
                            found_session_file = true;
                            break;
                        }
                    }
                }
            }
            if !found_session_file {
                let _ = fs::remove_dir_all(&temp_agent_dir);
                let _ = fs::remove_dir_all(&token_dir);
                return SessionQualificationResult {
                    session_id: cw_session_id.to_string(),
                    harness: "OMP".to_string(),
                    task_family: family.name().to_string(),
                    repetition,
                    status: "FAIL".to_string(),
                    logical_turns,
                    model_requests: 0,
                    resume_preserved: false,
                    overlay_applied: false,
                    overlay_reapplied_across_resume: false,
                    tool_pairing_intact: false,
                    transcripts_unmodified: true,
                    credentials_isolated: false,
                    acceptance_test_passed: false,
                    stream_completion_clean: false,
                    protocol_reliability: false,
                    agentic_reliability: false,
                    failure_class: Some(format!(
                        "OMP turn {turn_idx} failed to preserve native session file for {native_sid}"
                    )),
                };
            }
        }

        logical_turns += 1;
    }

    let canonical_sid = format!("omp_{native_sid}");
    let _ = store.upsert_session(&Session {
        id: canonical_sid.clone(),
        parent_session_id: None,
        harness: HarnessKind::Omp,
        workspace_root: None,
        model: "mock-model".to_string(),
        provider: ProviderKind::OpenAI,
        title: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    });

    let pre_mutation_count = store
        .list_sessions()
        .unwrap_or_default()
        .iter()
        .map(|s| store.list_turns(&s.id).unwrap_or_default().len())
        .sum::<usize>();

    // Commit overlay before checkpoint 5 on both cw_session_id and canonical_sid
    for target_sid in &[cw_session_id, &canonical_sid] {
        let _ = store.save_mutation(&MutationRule {
            id: format!("mut_{target_sid}"),
            session_id: target_sid.to_string(),
            op: MutationOp::PinContext {
                label: "Policy".to_string(),
                content: "CW114_POLICY_ACTIVE".to_string(),
                source_node_id: None,
            },
            status: MutationStatus::Committed,
            estimated_saved_tokens: 0,
            estimated_kv_break_tokens: 0,
            created_at: Utc::now(),
        });
    }

    // Checkpoints 5..8
    for turn_idx in 5..=8 {
        let omp_bin = omp_bin.to_path_buf();
        let ws = fixture.workspace_dir.clone();
        let th = temp_agent_dir.clone();
        let tf = token_file.clone();
        let ep = ext_path.clone();
        let sid = cw_session_id.to_string();
        let nsid_clone = native_sid.clone();

        let output = match tokio::time::timeout(
            Duration::from_secs(120),
            tokio::task::spawn_blocking(move || {
                let mut cmd = Command::new(omp_bin);
                cmd.args([
                    "-r",
                    &nsid_clone,
                    "-p",
                    &format!("Turn {} resumed prompt", turn_idx),
                    "--model",
                    "cw-proxy/mock-model",
                    "-e",
                    &ep.to_string_lossy(),
                ]);
                cmd.current_dir(&ws);
                cmd.env("PI_CODING_AGENT_DIR", &th);
                cmd.env("CW_CONTROL_TOKEN_FILE", &tf);
                cmd.env("CW_DAEMON_URL", format!("http://127.0.0.1:{proxy_port}"));
                cmd.env("CW_SESSION_ID", &sid);
                cmd.output()
            }),
        )
        .await
        {
            Ok(Ok(Ok(out))) => out,
            Ok(Ok(Err(e))) => {
                let _ = fs::remove_dir_all(&temp_agent_dir);
                let _ = fs::remove_dir_all(&token_dir);
                return SessionQualificationResult {
                    session_id: cw_session_id.to_string(),
                    harness: "OMP".to_string(),
                    task_family: family.name().to_string(),
                    repetition,
                    status: "FAIL".to_string(),
                    logical_turns,
                    model_requests: 0,
                    resume_preserved: false,
                    overlay_applied: false,
                    overlay_reapplied_across_resume: false,
                    tool_pairing_intact: false,
                    transcripts_unmodified: true,
                    credentials_isolated: false,
                    acceptance_test_passed: false,
                    stream_completion_clean: false,
                    protocol_reliability: false,
                    agentic_reliability: false,
                    failure_class: Some(format!(
                        "Failed to spawn resumed OMP on turn {turn_idx}: {e}"
                    )),
                };
            }
            _ => {
                let _ = fs::remove_dir_all(&temp_agent_dir);
                let _ = fs::remove_dir_all(&token_dir);
                return SessionQualificationResult {
                    session_id: cw_session_id.to_string(),
                    harness: "OMP".to_string(),
                    task_family: family.name().to_string(),
                    repetition,
                    status: "FAIL".to_string(),
                    logical_turns,
                    model_requests: 0,
                    resume_preserved: false,
                    overlay_applied: false,
                    overlay_reapplied_across_resume: false,
                    tool_pairing_intact: false,
                    transcripts_unmodified: true,
                    credentials_isolated: false,
                    acceptance_test_passed: false,
                    stream_completion_clean: false,
                    protocol_reliability: false,
                    agentic_reliability: false,
                    failure_class: Some(format!("OMP resume turn {turn_idx} timed out after 120s")),
                };
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("Turn {} stdout:\n{}\nstderr:\n{}", turn_idx, stdout, stderr);
        if !output.status.success() {
            let _ = fs::remove_dir_all(&temp_agent_dir);
            let _ = fs::remove_dir_all(&token_dir);
            return SessionQualificationResult {
                session_id: cw_session_id.to_string(),
                harness: "OMP".to_string(),
                task_family: family.name().to_string(),
                repetition,
                status: "FAIL".to_string(),
                logical_turns,
                model_requests: 0,
                resume_preserved: false,
                overlay_applied: false,
                overlay_reapplied_across_resume: false,
                tool_pairing_intact: false,
                transcripts_unmodified: true,
                credentials_isolated: false,
                acceptance_test_passed: false,
                stream_completion_clean: false,
                protocol_reliability: false,
                agentic_reliability: false,
                failure_class: Some(format!(
                    "OMP resume turn {turn_idx} failed: stderr={stderr}, stdout={stdout}"
                )),
            };
        }

        let session_dir = temp_agent_dir.join("sessions");
        let mut found_session_file = false;
        if session_dir.exists() {
            for f in find_jsonl_files(&session_dir) {
                if let Some(stem) = f.file_stem().and_then(|s| s.to_str()) {
                    if stem.contains(&native_sid) {
                        found_session_file = true;
                        break;
                    }
                }
            }
        }
        if !found_session_file {
            let _ = fs::remove_dir_all(&temp_agent_dir);
            let _ = fs::remove_dir_all(&token_dir);
            return SessionQualificationResult {
                session_id: cw_session_id.to_string(),
                harness: "OMP".to_string(),
                task_family: family.name().to_string(),
                repetition,
                status: "FAIL".to_string(),
                logical_turns,
                model_requests: 0,
                resume_preserved: false,
                overlay_applied: false,
                overlay_reapplied_across_resume: false,
                tool_pairing_intact: false,
                transcripts_unmodified: true,
                credentials_isolated: false,
                acceptance_test_passed: false,
                stream_completion_clean: false,
                protocol_reliability: false,
                agentic_reliability: false,
                failure_class: Some(format!(
                    "OMP resume turn {turn_idx} failed to preserve native session file for {native_sid}"
                )),
            };
        }

        logical_turns += 1;
    }

    let all_sessions = store.list_sessions().unwrap_or_default();
    let mut all_turns = Vec::new();
    for s in &all_sessions {
        let sturns = store.list_turns(&s.id).unwrap_or_default();
        eprintln!("OMP Session {}: {} turns", s.id, sturns.len());
        all_turns.extend(sturns);
    }
    all_turns.sort_by_key(|t| t.created_at);

    for (idx, t) in all_turns.iter().enumerate() {
        let payload = store.get_turn_mutated_payload(&t.id).unwrap();
        eprintln!(
            "OMP Turn {} (sid={}, id={}) mutated payload present: {}, contains overlay: {}",
            idx + 1,
            t.session_id,
            t.id,
            payload.is_some(),
            payload
                .as_ref()
                .map(|p| serde_json::to_string(p)
                    .unwrap_or_default()
                    .contains("CW114_POLICY_ACTIVE"))
                .unwrap_or(false)
        );
    }

    let model_requests = all_turns.len();
    let resume_preserved = !native_sid.is_empty() && logical_turns == 8;

    // Verify overlay presence:
    // 1. Pre-mutation turns must NOT contain CW114_POLICY_ACTIVE
    let pre_mutation_clean = all_turns.iter().take(pre_mutation_count).all(|t| {
        let payload = store.get_turn_mutated_payload(&t.id).unwrap();
        payload
            .map(|p| {
                !serde_json::to_string(&p)
                    .unwrap_or_default()
                    .contains("CW114_POLICY_ACTIVE")
            })
            .unwrap_or(true)
    });

    // 2. Post-mutation turns (5..8) must each individually contain CW114_POLICY_ACTIVE across process restarts
    let post_mutation_turns: Vec<_> = all_turns.iter().skip(pre_mutation_count).collect();
    let overlay_reapplied_across_resume = post_mutation_turns.len() == 4
        && post_mutation_turns.iter().all(|t| {
            let payload = store.get_turn_mutated_payload(&t.id).unwrap();
            payload
                .map(|p| {
                    serde_json::to_string(&p)
                        .unwrap_or_default()
                        .contains("CW114_POLICY_ACTIVE")
                })
                .unwrap_or(false)
        });

    let overlay_applied = pre_mutation_clean && overlay_reapplied_across_resume;

    // Check transcript immutability: scan temp_agent_dir for .jsonl session files
    let mut transcripts_unmodified = true;
    for file in find_jsonl_files(&temp_agent_dir) {
        if let Ok(content) = fs::read_to_string(&file) {
            if content.contains("CW114_POLICY_ACTIVE") {
                transcripts_unmodified = false;
            }
        }
    }

    let cap_guard = captured.lock().unwrap();
    let credentials_isolated = verify_captured_credentials_isolated(&cap_guard, control_token);
    let tool_pairing_intact = verify_captured_tool_pairing(&cap_guard);
    drop(cap_guard);

    let stream_completion_clean = all_turns
        .iter()
        .all(|t| t.outcome.as_deref() == Some("completed"));

    let acceptance_test_passed = fixture.verify_acceptance_tests();

    let _ = fs::remove_dir_all(&temp_agent_dir);
    let _ = fs::remove_dir_all(&token_dir);

    let protocol_reliability = resume_preserved
        && overlay_applied
        && credentials_isolated
        && transcripts_unmodified
        && stream_completion_clean;
    let agentic_reliability = tool_pairing_intact && acceptance_test_passed && logical_turns == 8;

    eprintln!(
        "DEBUG OMP: resume_preserved={resume_preserved}, overlay_applied={overlay_applied} (pre_clean={pre_mutation_clean}, post_reapplied={overlay_reapplied_across_resume}), credentials_isolated={credentials_isolated}, transcripts_unmodified={transcripts_unmodified}, stream_clean={stream_completion_clean}, tool_pairing={tool_pairing_intact}, acceptance={acceptance_test_passed}"
    );

    let failure_class = if !(protocol_reliability && agentic_reliability) {
        Some(format!(
            "protocol_reliability={protocol_reliability} (resume={resume_preserved}, overlay={overlay_applied}, creds={credentials_isolated}, transcripts={transcripts_unmodified}, stream={stream_completion_clean}), agentic_reliability={agentic_reliability} (pairing={tool_pairing_intact}, acceptance={acceptance_test_passed})"
        ))
    } else {
        None
    };

    SessionQualificationResult {
        session_id: cw_session_id.to_string(),
        harness: "OMP".to_string(),
        task_family: family.name().to_string(),
        repetition,
        status: if protocol_reliability && agentic_reliability {
            "PASS".to_string()
        } else {
            "FAIL".to_string()
        },
        logical_turns,
        model_requests,
        resume_preserved,
        overlay_applied,
        overlay_reapplied_across_resume: overlay_applied,
        tool_pairing_intact,
        transcripts_unmodified,
        credentials_isolated,
        acceptance_test_passed,
        stream_completion_clean,
        protocol_reliability,
        agentic_reliability,
        failure_class,
    }
}

fn resolve_hermes_venv_python(hermes_bin: &Path) -> Option<PathBuf> {
    let mut current = hermes_bin.to_path_buf();
    let mut visited = HashSet::new();

    while visited.insert(current.clone()) {
        if !current.exists() {
            break;
        }
        if current.is_symlink() {
            if let Ok(target) = fs::read_link(&current) {
                current = if target.is_absolute() {
                    target
                } else {
                    current.parent().unwrap_or(Path::new("")).join(target)
                };
                continue;
            }
        }
        if let Ok(content) = fs::read_to_string(&current) {
            let mut found_target = None;
            for line in content.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("exec ") {
                    let parts: Vec<&str> = trimmed.split_whitespace().collect();
                    if parts.len() >= 2 {
                        let candidate = parts[1].trim_matches(|c| c == '"' || c == '\'');
                        let cand_path = PathBuf::from(candidate);
                        if cand_path.is_file() && cand_path != current {
                            found_target = Some(cand_path);
                            break;
                        }
                    }
                }
            }
            if let Some(target) = found_target {
                current = target;
                continue;
            }
        }
        break;
    }

    let mut p = current.clone();
    while let Some(parent) = p.parent() {
        if parent == p {
            break;
        }
        let hermes_root = if parent.file_name().and_then(|n| n.to_str()) == Some(".hermes") {
            parent.to_path_buf()
        } else if parent.join(".hermes").is_dir() {
            parent.join(".hermes")
        } else {
            p = parent.to_path_buf();
            continue;
        };

        let installs_dir = hermes_root.join("installs");
        if let Ok(entries) = fs::read_dir(installs_dir) {
            for entry in entries.flatten() {
                let envs_dir = entry.path().join("environments");
                if let Ok(env_entries) = fs::read_dir(envs_dir) {
                    for env_entry in env_entries.flatten() {
                        let py = env_entry.path().join("venv/bin/python");
                        if py.is_file() {
                            return Some(py);
                        }
                        let py3 = env_entry.path().join("venv/bin/python3");
                        if py3.is_file() {
                            return Some(py3);
                        }
                    }
                }
            }
        }
        p = parent.to_path_buf();
    }
    None
}

async fn execute_hermes_session(
    hermes_bin: &Path,
    proxy_port: u16,
    control_token: &str,
    cw_session_id: &str,
    fixture: &WorkloadFixture,
    store: &CwStore,
    family: TaskFamily,
    repetition: usize,
    captured: Arc<Mutex<Vec<CapturedRequest>>>,
) -> SessionQualificationResult {
    // Assert initial state fails acceptance tests (proving test harness sensitivity)
    assert!(
        !fixture.verify_acceptance_tests(),
        "Fixture for {:?} must initially fail acceptance tests",
        family
    );

    // Dedicated temporary isolated HERMES_HOME (strictly isolated, zero symlinks to user home)
    let temp_hermes_home =
        std::env::temp_dir().join(format!("hermes_home_{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp_hermes_home).unwrap();

    struct HermesCleanupGuard {
        home: PathBuf,
        trusted_dir: PathBuf,
    }

    impl Drop for HermesCleanupGuard {
        fn drop(&mut self) {
            let pid_file = self.home.join("gateway.pid");
            if pid_file.exists() {
                if let Ok(content) = fs::read_to_string(&pid_file) {
                    if let Ok(pid) = content.trim().parse::<i32>() {
                        let _ = std::process::Command::new("kill")
                            .args(["-TERM", &pid.to_string()])
                            .output();
                    }
                }
            }
            let _ = fs::remove_dir_all(&self.home);
            let _ = fs::remove_dir_all(&self.trusted_dir);
        }
    }

    // Dedicated temporary trusted directory for isolated qualification
    let temp_trusted_dir =
        std::env::temp_dir().join(format!("hermes_trusted_{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp_trusted_dir).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&temp_trusted_dir, fs::Permissions::from_mode(0o700));
    }

    let _guard = HermesCleanupGuard {
        home: temp_hermes_home.clone(),
        trusted_dir: temp_trusted_dir.clone(),
    };

    let (exec_cmd, base_args): (PathBuf, Vec<String>) = match resolve_hermes_venv_python(hermes_bin)
    {
        Some(py) => (py, vec!["-m".to_string(), "hermes_cli.main".to_string()]),
        None => {
            return SessionQualificationResult {
                session_id: cw_session_id.to_string(),
                harness: "Hermes Agent".to_string(),
                task_family: family.name().to_string(),
                repetition,
                status: "BLOCKED".to_string(),
                logical_turns: 0,
                model_requests: 0,
                resume_preserved: false,
                overlay_applied: false,
                overlay_reapplied_across_resume: false,
                tool_pairing_intact: false,
                transcripts_unmodified: true,
                credentials_isolated: false,
                acceptance_test_passed: false,
                stream_completion_clean: false,
                protocol_reliability: false,
                agentic_reliability: false,
                failure_class: Some(format!(
                    "BLOCKED: Failed to resolve isolated virtual environment python interpreter from '{}'. Never invoke installed launcher as fallback.",
                    hermes_bin.display()
                )),
            };
        }
    };

    // Copy repo's context-watcher plugin into temp_hermes_home/plugins/context-watcher
    let plugins_dir = temp_hermes_home.join("plugins");
    let target_plugin_dir = plugins_dir.join("context-watcher");
    fs::create_dir_all(&target_plugin_dir).unwrap();
    let repo_plugin_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("plugins/hermes/context-watcher");
    let _ = fs::copy(
        repo_plugin_dir.join("__init__.py"),
        target_plugin_dir.join("__init__.py"),
    );
    let _ = fs::copy(
        repo_plugin_dir.join("plugin.yaml"),
        target_plugin_dir.join("plugin.yaml"),
    );

    // Configure Hermes to enable context-watcher plugin and pass CW session and security variables into the client environment
    let config_yaml = r#"
plugins:
  enabled:
    - context-watcher
  disabled: []
terminal:
  client_env_passthrough:
    - CW_CONTROL_TOKEN_FILE
    - CW_HERMES_OVERRIDE_FILE
    - CW_HERMES_TRUSTED_DIR
    - CW_SESSION_ID
    - CW_WORKSPACE
"#;
    let _ = fs::write(temp_hermes_home.join("config.yaml"), config_yaml);
    let hermes_home_str = temp_hermes_home.to_string_lossy().to_string();

    let temp_token_file = temp_trusted_dir.join("control-token");
    fs::write(&temp_token_file, control_token).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&temp_token_file, fs::Permissions::from_mode(0o600));
    }
    let token_path_str = temp_token_file.to_string_lossy().to_string();
    let trusted_dir_str = temp_trusted_dir.to_string_lossy().to_string();

    let mut created_overrides = Vec::new();
    let mut native_sid = String::new();
    let mut logical_turns = 0;

    // Checkpoints 1..4
    for turn_idx in 1..=4 {
        let override_uuid = uuid::Uuid::new_v4().to_string();
        let override_path = temp_trusted_dir.join(format!("hermes-override-{override_uuid}.json"));
        let payload = json!({
            "session_id": cw_session_id,
            "workspace": fixture.workspace_dir.to_string_lossy(),
            "ts": Utc::now().timestamp(),
        });
        fs::write(&override_path, serde_json::to_vec(&payload).unwrap()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&override_path, fs::Permissions::from_mode(0o600));
        }
        created_overrides.push(override_path.clone());

        let exec_cmd_clone = exec_cmd.clone();
        let base_args_clone = base_args.clone();
        let ws = fixture.workspace_dir.clone();
        let tok = token_path_str.clone();
        let op = override_path.to_string_lossy().to_string();
        let td = trusted_dir_str.clone();
        let hh = hermes_home_str.clone();
        let nsid_clone = native_sid.clone();

        let output = match tokio::time::timeout(
            Duration::from_secs(120),
            tokio::task::spawn_blocking(move || {
                let mut cmd = Command::new(exec_cmd_clone);
                cmd.args(&base_args_clone);
                if turn_idx == 1 {
                    cmd.args([
                        "chat",
                        "-Q",
                        "-q",
                        &format!("Turn {} initial prompt for {}", turn_idx, family.code()),
                        "--provider",
                        "custom",
                        "--base-url",
                        &format!("http://127.0.0.1:{proxy_port}/deepseek"),
                        "--api-key",
                        "mock-key",
                        "-m",
                        "mock-model",
                        "--source",
                        "tool",
                        "--yolo",
                    ]);
                } else {
                    cmd.args([
                        "chat",
                        "-r",
                        &nsid_clone,
                        "-Q",
                        "-q",
                        &format!("Turn {} follow-up prompt", turn_idx),
                        "--provider",
                        "custom",
                        "--base-url",
                        &format!("http://127.0.0.1:{proxy_port}/deepseek"),
                        "--api-key",
                        "mock-key",
                        "-m",
                        "mock-model",
                        "--source",
                        "tool",
                        "--yolo",
                    ]);
                }

                cmd.current_dir(&ws);
                cmd.env("HERMES_HOME", &hh);
                cmd.env("CW_CONTROL_TOKEN_FILE", &tok);
                cmd.env("CW_HERMES_OVERRIDE_FILE", &op);
                cmd.env("CW_HERMES_TRUSTED_DIR", &td);
                cmd.output()
            }),
        )
        .await
        {
            Ok(Ok(Ok(out))) => out,
            Ok(Ok(Err(e))) => {
                return SessionQualificationResult {
                    session_id: cw_session_id.to_string(),
                    harness: "Hermes Agent".to_string(),
                    task_family: family.name().to_string(),
                    repetition,
                    status: "FAIL".to_string(),
                    logical_turns,
                    model_requests: 0,
                    resume_preserved: false,
                    overlay_applied: false,
                    overlay_reapplied_across_resume: false,
                    tool_pairing_intact: false,
                    transcripts_unmodified: true,
                    credentials_isolated: false,
                    acceptance_test_passed: false,
                    stream_completion_clean: false,
                    protocol_reliability: false,
                    agentic_reliability: false,
                    failure_class: Some(format!("Failed to spawn Hermes on turn {turn_idx}: {e}")),
                };
            }
            _ => {
                return SessionQualificationResult {
                    session_id: cw_session_id.to_string(),
                    harness: "Hermes Agent".to_string(),
                    task_family: family.name().to_string(),
                    repetition,
                    status: "FAIL".to_string(),
                    logical_turns,
                    model_requests: 0,
                    resume_preserved: false,
                    overlay_applied: false,
                    overlay_reapplied_across_resume: false,
                    tool_pairing_intact: false,
                    transcripts_unmodified: true,
                    credentials_isolated: false,
                    acceptance_test_passed: false,
                    stream_completion_clean: false,
                    protocol_reliability: false,
                    agentic_reliability: false,
                    failure_class: Some(format!("Hermes turn {turn_idx} timed out after 120s")),
                };
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !output.status.success() {
            return SessionQualificationResult {
                session_id: cw_session_id.to_string(),
                harness: "Hermes Agent".to_string(),
                task_family: family.name().to_string(),
                repetition,
                status: "FAIL".to_string(),
                logical_turns,
                model_requests: 0,
                resume_preserved: false,
                overlay_applied: false,
                overlay_reapplied_across_resume: false,
                tool_pairing_intact: false,
                transcripts_unmodified: true,
                credentials_isolated: false,
                acceptance_test_passed: false,
                stream_completion_clean: false,
                protocol_reliability: false,
                agentic_reliability: false,
                failure_class: Some(format!(
                    "Hermes turn {turn_idx} failed: stderr={stderr}, stdout={stdout}"
                )),
            };
        }

        if turn_idx == 1 {
            let combined = format!("{stdout}\n{stderr}");
            for line in combined.lines() {
                if let Some(pos) = line.find("session_id: ") {
                    native_sid = line[pos + 12..].trim().to_string();
                    if !native_sid.is_empty() {
                        break;
                    }
                } else if let Some(pos) = line.find("Session: ") {
                    native_sid = line[pos + 9..].trim().to_string();
                    if !native_sid.is_empty() {
                        break;
                    }
                } else if let Some(pos) = line.find("session: ") {
                    native_sid = line[pos + 9..].trim().to_string();
                    if !native_sid.is_empty() {
                        break;
                    }
                }
            }
            if native_sid.is_empty() {
                let sessions_dir = temp_hermes_home.join("sessions");
                if let Ok(entries) = fs::read_dir(&sessions_dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                                native_sid = stem.to_string();
                                break;
                            }
                        }
                    }
                }
            }
            // CRITICAL: Do NOT invent an ID fallback. Fail qualification if native session ID cannot be recovered.
            if native_sid.is_empty() {
                return SessionQualificationResult {
                    session_id: cw_session_id.to_string(),
                    harness: "Hermes Agent".to_string(),
                    task_family: family.name().to_string(),
                    repetition,
                    status: "FAIL".to_string(),
                    logical_turns,
                    model_requests: 0,
                    resume_preserved: false,
                    overlay_applied: false,
                    overlay_reapplied_across_resume: false,
                    tool_pairing_intact: false,
                    transcripts_unmodified: true,
                    credentials_isolated: false,
                    acceptance_test_passed: false,
                    stream_completion_clean: false,
                    protocol_reliability: false,
                    agentic_reliability: false,
                    failure_class: Some(format!(
                        "Failed to recover native Hermes session ID from output:\n{combined}"
                    )),
                };
            }

            // Assert that harness tool execution passed the acceptance tests
            if !fixture.verify_acceptance_tests() {
                return SessionQualificationResult {
                    session_id: cw_session_id.to_string(),
                    harness: "Hermes Agent".to_string(),
                    task_family: family.name().to_string(),
                    repetition,
                    status: "FAIL".to_string(),
                    logical_turns,
                    model_requests: 0,
                    resume_preserved: false,
                    overlay_applied: false,
                    overlay_reapplied_across_resume: false,
                    tool_pairing_intact: false,
                    transcripts_unmodified: true,
                    credentials_isolated: false,
                    acceptance_test_passed: false,
                    stream_completion_clean: false,
                    protocol_reliability: false,
                    agentic_reliability: false,
                    failure_class: Some(
                        "Acceptance tests failed after Hermes turn 1 tool execution".to_string(),
                    ),
                };
            }
        } else {
            let combined = format!("{stdout}\n{stderr}");
            if !combined.contains(&native_sid) {
                return SessionQualificationResult {
                    session_id: cw_session_id.to_string(),
                    harness: "Hermes Agent".to_string(),
                    task_family: family.name().to_string(),
                    repetition,
                    status: "FAIL".to_string(),
                    logical_turns,
                    model_requests: 0,
                    resume_preserved: false,
                    overlay_applied: false,
                    overlay_reapplied_across_resume: false,
                    tool_pairing_intact: false,
                    transcripts_unmodified: true,
                    credentials_isolated: false,
                    acceptance_test_passed: false,
                    stream_completion_clean: false,
                    protocol_reliability: false,
                    agentic_reliability: false,
                    failure_class: Some(format!(
                        "Hermes resume turn {turn_idx} output does not contain native session ID {native_sid}"
                    )),
                };
            }
        }

        logical_turns += 1;
    }

    let _ = store.upsert_session(&Session {
        id: cw_session_id.to_string(),
        parent_session_id: None,
        harness: HarnessKind::GenericProxy,
        workspace_root: None,
        model: "mock-model".to_string(),
        provider: ProviderKind::OpenAICompatible,
        title: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    });

    // Commit overlay before checkpoint 5
    let pre_mutation_count = store.list_turns(cw_session_id).unwrap_or_default().len();
    store
        .save_mutation(&MutationRule {
            id: format!("mut_{cw_session_id}"),
            session_id: cw_session_id.to_string(),
            op: MutationOp::PinContext {
                label: "Policy".to_string(),
                content: "CW114_POLICY_ACTIVE".to_string(),
                source_node_id: None,
            },
            status: MutationStatus::Committed,
            estimated_saved_tokens: 0,
            estimated_kv_break_tokens: 0,
            created_at: Utc::now(),
        })
        .unwrap();

    // Checkpoints 5..8
    for turn_idx in 5..=8 {
        let override_uuid = uuid::Uuid::new_v4().to_string();
        let override_path = temp_trusted_dir.join(format!("hermes-override-{override_uuid}.json"));
        let payload = json!({
            "session_id": cw_session_id,
            "workspace": fixture.workspace_dir.to_string_lossy(),
            "ts": Utc::now().timestamp(),
        });
        fs::write(&override_path, serde_json::to_vec(&payload).unwrap()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&override_path, fs::Permissions::from_mode(0o600));
        }
        created_overrides.push(override_path.clone());

        let exec_cmd_clone = exec_cmd.clone();
        let base_args_clone = base_args.clone();
        let ws = fixture.workspace_dir.clone();
        let tok = token_path_str.clone();
        let op = override_path.to_string_lossy().to_string();
        let td = trusted_dir_str.clone();
        let hh = hermes_home_str.clone();
        let nsid_clone = native_sid.clone();

        let output = match tokio::time::timeout(
            Duration::from_secs(120),
            tokio::task::spawn_blocking(move || {
                let mut cmd = Command::new(exec_cmd_clone);
                cmd.args(&base_args_clone);
                cmd.args([
                    "chat",
                    "-r",
                    &nsid_clone,
                    "-Q",
                    "-q",
                    &format!("Turn {} resumed prompt", turn_idx),
                    "--provider",
                    "custom",
                    "--base-url",
                    &format!("http://127.0.0.1:{proxy_port}/deepseek"),
                    "--api-key",
                    "mock-key",
                    "-m",
                    "mock-model",
                    "--source",
                    "tool",
                    "--yolo",
                ]);

                cmd.current_dir(&ws);
                cmd.env("HERMES_HOME", &hh);
                cmd.env("CW_CONTROL_TOKEN_FILE", &tok);
                cmd.env("CW_HERMES_OVERRIDE_FILE", &op);
                cmd.env("CW_HERMES_TRUSTED_DIR", &td);
                cmd.output()
            }),
        )
        .await
        {
            Ok(Ok(Ok(out))) => out,
            Ok(Ok(Err(e))) => {
                return SessionQualificationResult {
                    session_id: cw_session_id.to_string(),
                    harness: "Hermes Agent".to_string(),
                    task_family: family.name().to_string(),
                    repetition,
                    status: "FAIL".to_string(),
                    logical_turns,
                    model_requests: 0,
                    resume_preserved: false,
                    overlay_applied: false,
                    overlay_reapplied_across_resume: false,
                    tool_pairing_intact: false,
                    transcripts_unmodified: true,
                    credentials_isolated: false,
                    acceptance_test_passed: false,
                    stream_completion_clean: false,
                    protocol_reliability: false,
                    agentic_reliability: false,
                    failure_class: Some(format!(
                        "Failed to spawn resumed Hermes on turn {turn_idx}: {e}"
                    )),
                };
            }
            _ => {
                return SessionQualificationResult {
                    session_id: cw_session_id.to_string(),
                    harness: "Hermes Agent".to_string(),
                    task_family: family.name().to_string(),
                    repetition,
                    status: "FAIL".to_string(),
                    logical_turns,
                    model_requests: 0,
                    resume_preserved: false,
                    overlay_applied: false,
                    overlay_reapplied_across_resume: false,
                    tool_pairing_intact: false,
                    transcripts_unmodified: true,
                    credentials_isolated: false,
                    acceptance_test_passed: false,
                    stream_completion_clean: false,
                    protocol_reliability: false,
                    agentic_reliability: false,
                    failure_class: Some(format!(
                        "Hermes resume turn {turn_idx} timed out after 120s"
                    )),
                };
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !output.status.success() {
            return SessionQualificationResult {
                session_id: cw_session_id.to_string(),
                harness: "Hermes Agent".to_string(),
                task_family: family.name().to_string(),
                repetition,
                status: "FAIL".to_string(),
                logical_turns,
                model_requests: 0,
                resume_preserved: false,
                overlay_applied: false,
                overlay_reapplied_across_resume: false,
                tool_pairing_intact: false,
                transcripts_unmodified: true,
                credentials_isolated: false,
                acceptance_test_passed: false,
                stream_completion_clean: false,
                protocol_reliability: false,
                agentic_reliability: false,
                failure_class: Some(format!(
                    "Hermes resume turn {turn_idx} failed: stderr={stderr}, stdout={stdout}"
                )),
            };
        }

        let combined = format!("{stdout}\n{stderr}");
        if !combined.contains(&native_sid) {
            return SessionQualificationResult {
                session_id: cw_session_id.to_string(),
                harness: "Hermes Agent".to_string(),
                task_family: family.name().to_string(),
                repetition,
                status: "FAIL".to_string(),
                logical_turns,
                model_requests: 0,
                resume_preserved: false,
                overlay_applied: false,
                overlay_reapplied_across_resume: false,
                tool_pairing_intact: false,
                transcripts_unmodified: true,
                credentials_isolated: false,
                acceptance_test_passed: false,
                stream_completion_clean: false,
                protocol_reliability: false,
                agentic_reliability: false,
                failure_class: Some(format!(
                    "Hermes resume turn {turn_idx} output does not contain native session ID {native_sid}"
                )),
            };
        }

        logical_turns += 1;
    }

    let all_sessions = store.list_sessions().unwrap_or_default();
    eprintln!(
        "All sessions in store: {:?}",
        all_sessions.iter().map(|s| &s.id).collect::<Vec<_>>()
    );
    for s in &all_sessions {
        let sturns = store.list_turns(&s.id).unwrap_or_default();
        eprintln!("Session {}: {} turns", s.id, sturns.len());
    }
    let turns = store.list_turns(cw_session_id).unwrap_or_default();
    eprintln!(
        "Turns for cw_session_id {}: count={}",
        cw_session_id,
        turns.len()
    );
    {
        let cap = captured.lock().unwrap();
        for (i, req) in cap.iter().enumerate() {
            eprintln!("Mock captured req {}: headers={:?}", i, req.headers);
        }
    }
    let model_requests = turns.len();
    let resume_preserved = !native_sid.is_empty() && logical_turns == 8;

    // Verify overlay presence:
    // 1. Pre-mutation turns must NOT contain CW114_POLICY_ACTIVE
    let pre_mutation_clean = turns.iter().take(pre_mutation_count).all(|t| {
        store
            .get_turn_mutated_payload(&t.id)
            .unwrap()
            .map(|p| {
                !serde_json::to_string(&p)
                    .unwrap_or_default()
                    .contains("CW114_POLICY_ACTIVE")
            })
            .unwrap_or(true)
    });

    // 2. Post-mutation turns (5..8) must each individually contain CW114_POLICY_ACTIVE across process restarts
    let post_mutation_turns: Vec<_> = turns.iter().skip(pre_mutation_count).collect();
    let overlay_reapplied_across_resume = post_mutation_turns.len() == 4
        && post_mutation_turns.iter().all(|t| {
            store
                .get_turn_mutated_payload(&t.id)
                .unwrap()
                .map(|p| {
                    serde_json::to_string(&p)
                        .unwrap_or_default()
                        .contains("CW114_POLICY_ACTIVE")
                })
                .unwrap_or(false)
        });

    let overlay_applied = pre_mutation_clean && overlay_reapplied_across_resume;

    let cap_guard = captured.lock().unwrap();
    let credentials_isolated = verify_captured_credentials_isolated(&cap_guard, control_token);
    let tool_pairing_intact = verify_captured_tool_pairing(&cap_guard);
    drop(cap_guard);

    let stream_completion_clean = turns
        .iter()
        .all(|t| t.outcome.as_deref() == Some("completed"));

    let acceptance_test_passed = fixture.verify_acceptance_tests();

    // Check transcript immutability: scan temp_hermes_home for session files
    let mut transcripts_unmodified = true;
    if let Ok(entries) = fs::read_dir(temp_hermes_home.join("sessions")) {
        for entry in entries.flatten() {
            if entry.path().is_file() {
                let content = fs::read_to_string(entry.path()).unwrap_or_default();
                if content.contains("CW114_POLICY_ACTIVE") {
                    transcripts_unmodified = false;
                }
            }
        }
    }

    let protocol_reliability = resume_preserved
        && overlay_applied
        && credentials_isolated
        && transcripts_unmodified
        && stream_completion_clean;
    let agentic_reliability = tool_pairing_intact && acceptance_test_passed && logical_turns == 8;

    SessionQualificationResult {
        session_id: cw_session_id.to_string(),
        harness: "Hermes Agent".to_string(),
        task_family: family.name().to_string(),
        repetition,
        status: if protocol_reliability && agentic_reliability {
            "PASS".to_string()
        } else {
            "FAIL".to_string()
        },
        logical_turns,
        model_requests,
        resume_preserved,
        overlay_applied,
        overlay_reapplied_across_resume: overlay_applied,
        tool_pairing_intact,
        transcripts_unmodified,
        credentials_isolated,
        acceptance_test_passed,
        stream_completion_clean,
        protocol_reliability,
        agentic_reliability,
        failure_class: if protocol_reliability && agentic_reliability {
            None
        } else {
            Some(format!(
                "Hermes checks failed: resume_preserved={}, overlay_applied={}, credentials_isolated={}, transcripts_unmodified={}, stream_completion_clean={}, tool_pairing_intact={}, acceptance_test_passed={}, logical_turns={}",
                resume_preserved,
                overlay_applied,
                credentials_isolated,
                transcripts_unmodified,
                stream_completion_clean,
                tool_pairing_intact,
                acceptance_test_passed,
                logical_turns
            ))
        },
    }
}

// ============================================================================
// 7. Deterministic Integration Tests & CI Harness
// ============================================================================

#[tokio::test]
async fn test_workload_fixtures_acceptance_criteria() {
    for family in TaskFamily::all() {
        let fixture = WorkloadFixture::create(*family);
        // Initially, the fixture has failing tests (or unfinished implementation)
        assert!(
            !fixture.verify_acceptance_tests(),
            "Task family {:?} initial state must fail acceptance tests",
            family
        );
    }
}

#[tokio::test]
async fn test_cw114_single_session_codex_deterministic() {
    let runner = CampaignRunner::new();
    if runner.codex_bin.is_none() {
        println!("SKIPPED: codex binary not available");
        return;
    }
    let res = runner
        .run_session(HarnessTarget::Codex, TaskFamily::F1BugFixing, 1)
        .await;
    assert_eq!(
        res.status, "PASS",
        "Codex F1 Bug Fixing session must pass: {:?}",
        res.failure_class
    );
    assert!(res.protocol_reliability, "Protocol reliability must pass");
    assert!(res.agentic_reliability, "Agentic reliability must pass");
    assert!(res.acceptance_test_passed, "Acceptance tests must pass");
    assert!(res.tool_pairing_intact, "Tool pairing must be intact");
    assert!(res.overlay_applied, "Overlay must be applied");
}

#[tokio::test]
async fn test_cw114_single_session_omp_deterministic() {
    let runner = CampaignRunner::new();
    if runner.omp_bin.is_none() {
        println!("SKIPPED: omp binary not available");
        return;
    }
    let res = runner
        .run_session(HarnessTarget::Omp, TaskFamily::F1BugFixing, 1)
        .await;
    assert_eq!(
        res.status, "PASS",
        "OMP F1 Bug Fixing session must pass: {:?}",
        res.failure_class
    );
    assert!(res.protocol_reliability, "Protocol reliability must pass");
    assert!(res.agentic_reliability, "Agentic reliability must pass");
    assert!(res.acceptance_test_passed, "Acceptance tests must pass");
    assert!(res.tool_pairing_intact, "Tool pairing must be intact");
    assert!(res.overlay_applied, "Overlay must be applied");
}

#[tokio::test]
async fn test_cw114_single_session_hermes_deterministic() {
    let runner = CampaignRunner::new();
    if runner.hermes_bin.is_none() {
        println!("SKIPPED: hermes binary not available");
        return;
    }
    let res = runner
        .run_session(HarnessTarget::Hermes, TaskFamily::F1BugFixing, 1)
        .await;
    assert_eq!(
        res.status, "PASS",
        "Hermes F1 Bug Fixing session must pass: {:?}",
        res.failure_class
    );
    assert!(res.protocol_reliability, "Protocol reliability must pass");
    assert!(res.agentic_reliability, "Agentic reliability must pass");
    assert!(res.acceptance_test_passed, "Acceptance tests must pass");
    assert!(res.tool_pairing_intact, "Tool pairing must be intact");
    assert!(res.overlay_applied, "Overlay must be applied");
}

#[tokio::test]
#[ignore = "CW-114 deterministic full 45-session campaign; run with cargo test --test long_session_reliability test_cw114_deterministic_45_session_campaign -- --ignored --nocapture"]
async fn test_cw114_deterministic_45_session_campaign() {
    let runner = CampaignRunner::new();
    let report = runner.run_full_campaign().await;

    println!("\n=======================================================");
    println!("CW-114 QUALIFICATION CAMPAIGN RESULTS (45 SESSIONS)");
    println!("=======================================================");
    println!("Baseline commit : {}", report.baseline_commit);
    println!("Codex version   : {}", report.codex_version);
    println!("Hermes version  : {}", report.hermes_version);
    println!("OMP version     : {}", report.omp_version);
    println!("Total sessions  : {}", report.total_sessions);
    println!("Passed sessions : {}", report.passed_sessions);
    println!("Failed sessions : {}", report.failed_sessions);
    println!("Blocked sessions: {}", report.blocked_sessions);
    println!("-------------------------------------------------------");

    for r in &report.results {
        println!(
            "[{}] {:<10} {:<30} rep {}: requests={}, turns={}, acceptance={}",
            r.status,
            r.harness,
            r.task_family,
            r.repetition,
            r.model_requests,
            r.logical_turns,
            r.acceptance_test_passed
        );
    }
    println!("=======================================================\n");

    assert_eq!(
        report.passed_sessions, 45,
        "All 45 sessions must pass without any blocked or failed sessions"
    );
    assert_eq!(report.failed_sessions, 0, "Zero sessions may fail");
    assert_eq!(report.blocked_sessions, 0, "Zero sessions may be blocked");
    assert_eq!(
        report.total_sessions, 45,
        "Campaign must run all 45 sessions across the matrix"
    );
}

// ============================================================================
// 8. Phase 4 Fault Injection and Resilience Test Suite
// ============================================================================

#[tokio::test]
async fn test_cw114_fault_injection_mid_stream_drop_recorded_as_stream_error() {
    let app = Router::new().route(
        "/v1/chat/completions",
        post(|| async {
            let chunks: Vec<Result<axum::body::Bytes, std::io::Error>> = vec![
                Ok(axum::body::Bytes::from("data: {\"id\":\"cmpl-drop\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"Starting chunk...\"}}]}\n\n")),
            ];
            // Stream EOF without terminal [DONE]
            Response::builder()
                .header("content-type", "text/event-stream")
                .body(Body::from_stream(futures::stream::iter(chunks)))
                .unwrap()
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let mock_task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let store = CwStore::open_in_memory().unwrap();
    let mut state = DaemonState::new(store.clone());
    let token = state.control_token.clone();
    state.openai_upstream = format!("http://127.0.0.1:{port}");

    let sid = "cw114_fault_stream_drop";
    let _ = store.upsert_session(&Session {
        id: sid.to_string(),
        parent_session_id: None,
        harness: HarnessKind::GenericProxy,
        workspace_root: None,
        model: "mock-model".to_string(),
        provider: ProviderKind::OpenAI,
        title: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    });

    let req = Request::builder()
        .method("POST")
        .uri("/openai/v1/chat/completions")
        .header("host", "127.0.0.1")
        .header("content-type", "application/json")
        .header("x-cw-session-id", sid)
        .header("x-cw-control-token", &token)
        .body(Body::from(
            json!({
                "model": "mock-model",
                "stream": true,
                "messages": [{"role": "user", "content": "hello"}]
            })
            .to_string(),
        ))
        .unwrap();

    let resp = build_proxy_router(state).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let mut stream = resp.into_body().into_data_stream();
    while let Some(chunk) = stream.next().await {
        let _ = chunk.unwrap();
    }

    let mut final_turn = None;
    for _ in 0..100 {
        if let Ok(Some(turn)) = store.get_latest_turn(sid)
            && turn.outcome.as_deref() != Some("pending")
            && turn.outcome.is_some()
        {
            final_turn = Some(turn);
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let turn = final_turn.expect("Turn must record final outcome");
    assert_eq!(
        turn.outcome.as_deref(),
        Some("stream_error"),
        "Mid-stream drop without terminal SSE event must record stream_error"
    );
    mock_task.abort();
}

#[tokio::test]
async fn test_cw114_fault_injection_http_502_and_429_recovery() {
    let attempt = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let attempt_clone = attempt.clone();

    let app = Router::new().route(
        "/v1/chat/completions",
        post(move || {
            let attempt = attempt_clone.clone();
            async move {
                let count = attempt.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if count == 0 {
                    Response::builder()
                        .status(StatusCode::BAD_GATEWAY)
                        .body(Body::from("Bad Gateway"))
                        .unwrap()
                } else if count == 1 {
                    Response::builder()
                        .status(StatusCode::TOO_MANY_REQUESTS)
                        .header("retry-after", "1")
                        .body(Body::from("Rate limited"))
                        .unwrap()
                } else {
                    Response::builder()
                        .status(StatusCode::OK)
                        .header("content-type", "application/json")
                        .body(Body::from(json!({
                            "id": "cmpl-recovered",
                            "object": "chat.completion",
                            "choices": [{"index": 0, "message": {"role": "assistant", "content": "Recovered!"}, "finish_reason": "stop"}],
                            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
                        }).to_string()))
                        .unwrap()
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let mock_task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let store = CwStore::open_in_memory().unwrap();
    let mut state = DaemonState::new(store.clone());
    let token = state.control_token.clone();
    state.openai_upstream = format!("http://127.0.0.1:{port}");

    let sid = "cw114_fault_502_429_rec";
    let _ = store.upsert_session(&Session {
        id: sid.to_string(),
        parent_session_id: None,
        harness: HarnessKind::GenericProxy,
        workspace_root: None,
        model: "mock-model".to_string(),
        provider: ProviderKind::OpenAI,
        title: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    });

    // Request 1 -> 502
    let req1 = Request::builder()
        .method("POST")
        .uri("/openai/v1/chat/completions")
        .header("host", "127.0.0.1")
        .header("content-type", "application/json")
        .header("x-cw-session-id", sid)
        .header("x-cw-control-token", &token)
        .body(Body::from(
            json!({
                "model": "mock-model",
                "messages": [{"role": "user", "content": "attempt 1"}]
            })
            .to_string(),
        ))
        .unwrap();

    let resp1 = build_proxy_router(state.clone())
        .oneshot(req1)
        .await
        .unwrap();
    assert_eq!(resp1.status(), StatusCode::BAD_GATEWAY);
    let _ = axum::body::to_bytes(resp1.into_body(), 1024).await;

    // Request 2 -> 429
    let req2 = Request::builder()
        .method("POST")
        .uri("/openai/v1/chat/completions")
        .header("host", "127.0.0.1")
        .header("content-type", "application/json")
        .header("x-cw-session-id", sid)
        .header("x-cw-control-token", &token)
        .body(Body::from(
            json!({
                "model": "mock-model",
                "messages": [{"role": "user", "content": "attempt 2"}]
            })
            .to_string(),
        ))
        .unwrap();

    let resp2 = build_proxy_router(state.clone())
        .oneshot(req2)
        .await
        .unwrap();
    assert_eq!(resp2.status(), StatusCode::TOO_MANY_REQUESTS);
    let _ = axum::body::to_bytes(resp2.into_body(), 1024).await;

    // Request 3 -> 200 OK (recovered)
    let req3 = Request::builder()
        .method("POST")
        .uri("/openai/v1/chat/completions")
        .header("host", "127.0.0.1")
        .header("content-type", "application/json")
        .header("x-cw-session-id", sid)
        .header("x-cw-control-token", &token)
        .body(Body::from(
            json!({
                "model": "mock-model",
                "messages": [{"role": "user", "content": "attempt 3"}]
            })
            .to_string(),
        ))
        .unwrap();

    let resp3 = build_proxy_router(state.clone())
        .oneshot(req3)
        .await
        .unwrap();
    assert_eq!(resp3.status(), StatusCode::OK);
    let _ = axum::body::to_bytes(resp3.into_body(), 1024).await;

    let mut turns = Vec::new();
    for _ in 0..100 {
        turns = store.list_turns(sid).unwrap();
        if turns.len() == 3
            && turns
                .iter()
                .all(|t| t.outcome.as_deref() != Some("pending"))
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert_eq!(turns.len(), 3);
    assert_eq!(turns[0].outcome.as_deref(), Some("upstream_error"));
    assert_eq!(turns[1].outcome.as_deref(), Some("upstream_error"));
    assert_eq!(turns[2].outcome.as_deref(), Some("completed"));

    mock_task.abort();
}

#[tokio::test]
async fn test_cw114_resilience_daemon_restart_persisted_state() {
    let temp_db_dir = std::env::temp_dir().join(format!("cw_db_{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp_db_dir).unwrap();
    let db_path = temp_db_dir.join("cw_test.db");

    let app = Router::new().route(
        "/v1/chat/completions",
        post(|| async {
            Json(json!({
                "id": "cmpl-restart-test",
                "object": "chat.completion",
                "choices": [{"index": 0, "message": {"role": "assistant", "content": "Daemon restart response"}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 50, "completion_tokens": 10, "total_tokens": 60}
            }))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mock_port = listener.local_addr().unwrap().port();
    let mock_task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let sid = "cw114_daemon_restart_sid";

    // 1. Initial Daemon instance
    let control_token = "cw114_persisted_control_token_fixed".to_string();
    {
        let store1 = CwStore::open(&db_path).unwrap();
        store1
            .set_autopilot_config(&cw_core::autopilot::AutoPilotConfig {
                enabled: true,
                ..Default::default()
            })
            .unwrap();
        store1
            .upsert_session(&Session {
                id: sid.to_string(),
                parent_session_id: None,
                harness: HarnessKind::GenericProxy,
                workspace_root: None,
                model: "mock-model".to_string(),
                provider: ProviderKind::OpenAI,
                title: None,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            })
            .unwrap();

        let mut state1 = DaemonState::new(store1.clone());
        state1.control_token = control_token.clone();
        state1.openai_upstream = format!("http://127.0.0.1:{mock_port}");

        let req1 = Request::builder()
            .method("POST")
            .uri("/openai/v1/chat/completions")
            .header("host", "127.0.0.1")
            .header("content-type", "application/json")
            .header("x-cw-session-id", sid)
            .header("x-cw-control-token", &control_token)
            .body(Body::from(
                json!({
                    "model": "mock-model",
                    "messages": [{"role": "user", "content": "Turn 1 before restart"}]
                })
                .to_string(),
            ))
            .unwrap();

        let resp1 = build_proxy_router(state1).oneshot(req1).await.unwrap();
        assert_eq!(resp1.status(), StatusCode::OK);

        // Commit overlay rule in persistent store
        store1
            .save_mutation(&MutationRule {
                id: format!("mut_{sid}"),
                session_id: sid.to_string(),
                op: MutationOp::PinContext {
                    label: "RestartPolicy".to_string(),
                    content: "CW114_RESTART_ACTIVE_PIN".to_string(),
                    source_node_id: None,
                },
                status: MutationStatus::Committed,
                estimated_saved_tokens: 0,
                estimated_kv_break_tokens: 0,
                created_at: Utc::now(),
            })
            .unwrap();
    } // Daemon 1 dropped

    // 2. Restarted Daemon instance on same SQLite DB file
    {
        let store2 = CwStore::open(&db_path).unwrap();
        let sessions = store2.list_sessions().unwrap();
        assert!(
            sessions.iter().any(|s| s.id == sid),
            "Session must survive daemon restart in SQLite"
        );

        let mut state2 = DaemonState::new(store2.clone());
        state2.control_token = control_token.clone();
        state2.openai_upstream = format!("http://127.0.0.1:{mock_port}");

        let req2 = Request::builder()
            .method("POST")
            .uri("/openai/v1/chat/completions")
            .header("host", "127.0.0.1")
            .header("content-type", "application/json")
            .header("x-cw-session-id", sid)
            .header("x-cw-control-token", &control_token)
            .body(Body::from(
                json!({
                    "model": "mock-model",
                    "messages": [
                        {"role": "user", "content": "Turn 1 before restart"},
                        {"role": "assistant", "content": "Daemon restart response"},
                        {"role": "user", "content": "Turn 2 after restart with active pin"}
                    ]
                })
                .to_string(),
            ))
            .unwrap();

        let resp2 = build_proxy_router(state2).oneshot(req2).await.unwrap();
        assert_eq!(resp2.status(), StatusCode::OK);

        let turns = store2.list_turns(sid).unwrap();
        assert_eq!(
            turns.len(),
            2,
            "Both turns must be recorded in SQLite across restart"
        );

        let turn2_payload = store2
            .get_turn_mutated_payload(&turns[1].id)
            .unwrap()
            .unwrap();
        assert!(
            serde_json::to_string(&turn2_payload)
                .unwrap()
                .contains("CW114_RESTART_ACTIVE_PIN"),
            "Committed overlay must survive restart and be applied to post-restart request"
        );
    }

    mock_task.abort();
    let _ = fs::remove_dir_all(&temp_db_dir);
}

#[tokio::test]
async fn test_cw114_resilience_concurrent_sessions_distinct_overlays() {
    let captured = Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_clone = captured.clone();

    let app = Router::new().route(
        "/v1/chat/completions",
        post(move |body: axum::body::Bytes| {
            let cap = captured_clone.clone();
            async move {
                let val = serde_json::from_slice::<Value>(&body).unwrap();
                cap.lock().unwrap().push(val);
                tokio::time::sleep(Duration::from_millis(50)).await;
                Json(json!({
                    "id": "cmpl-concurrent",
                    "object": "chat.completion",
                    "choices": [{"index": 0, "message": {"role": "assistant", "content": "Concurrent OK"}, "finish_reason": "stop"}],
                    "usage": {"prompt_tokens": 10, "completion_tokens": 2, "total_tokens": 12}
                }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mock_port = listener.local_addr().unwrap().port();
    let mock_task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let store = CwStore::open_in_memory().unwrap();
    store
        .set_autopilot_config(&cw_core::autopilot::AutoPilotConfig {
            enabled: true,
            ..Default::default()
        })
        .unwrap();

    let sid_a = "cw114_concurrent_sess_A";
    let sid_b = "cw114_concurrent_sess_B";

    for sid in &[sid_a, sid_b] {
        store
            .upsert_session(&Session {
                id: sid.to_string(),
                parent_session_id: None,
                harness: HarnessKind::GenericProxy,
                workspace_root: None,
                model: "mock-model".to_string(),
                provider: ProviderKind::OpenAI,
                title: None,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            })
            .unwrap();
    }

    store
        .save_mutation(&MutationRule {
            id: "mut_A".to_string(),
            session_id: sid_a.to_string(),
            op: MutationOp::PinContext {
                label: "PolicyA".to_string(),
                content: "CRITICAL_POLICY_A_ONLY: STRICT_ISOLATION_A".to_string(),
                source_node_id: None,
            },
            status: MutationStatus::Committed,
            estimated_saved_tokens: 0,
            estimated_kv_break_tokens: 0,
            created_at: Utc::now(),
        })
        .unwrap();

    store
        .save_mutation(&MutationRule {
            id: "mut_B".to_string(),
            session_id: sid_b.to_string(),
            op: MutationOp::PinContext {
                label: "PolicyB".to_string(),
                content: "CRITICAL_POLICY_B_ONLY: STRICT_ISOLATION_B".to_string(),
                source_node_id: None,
            },
            status: MutationStatus::Committed,
            estimated_saved_tokens: 0,
            estimated_kv_break_tokens: 0,
            created_at: Utc::now(),
        })
        .unwrap();

    let mut state = DaemonState::new(store.clone());
    let token = state.control_token.clone();
    state.openai_upstream = format!("http://127.0.0.1:{mock_port}");

    let router = build_proxy_router(state);

    let req_a = Request::builder()
        .method("POST")
        .uri("/openai/v1/chat/completions")
        .header("host", "127.0.0.1")
        .header("content-type", "application/json")
        .header("x-cw-session-id", sid_a)
        .header("x-cw-control-token", &token)
        .body(Body::from(
            json!({
                "model": "mock-model",
                "messages": [{"role": "user", "content": "Concurrent prompt from session A"}]
            })
            .to_string(),
        ))
        .unwrap();

    let req_b = Request::builder()
        .method("POST")
        .uri("/openai/v1/chat/completions")
        .header("host", "127.0.0.1")
        .header("content-type", "application/json")
        .header("x-cw-session-id", sid_b)
        .header("x-cw-control-token", &token)
        .body(Body::from(
            json!({
                "model": "mock-model",
                "messages": [{"role": "user", "content": "Concurrent prompt from session B"}]
            })
            .to_string(),
        ))
        .unwrap();

    let (resp_a, resp_b) = tokio::join!(router.clone().oneshot(req_a), router.oneshot(req_b),);

    assert_eq!(resp_a.unwrap().status(), StatusCode::OK);
    assert_eq!(resp_b.unwrap().status(), StatusCode::OK);

    let turn_a = store.get_latest_turn(sid_a).unwrap().unwrap();
    let turn_b = store.get_latest_turn(sid_b).unwrap().unwrap();

    let payload_a_str =
        serde_json::to_string(&store.get_turn_mutated_payload(&turn_a.id).unwrap().unwrap())
            .unwrap();
    let payload_b_str =
        serde_json::to_string(&store.get_turn_mutated_payload(&turn_b.id).unwrap().unwrap())
            .unwrap();

    assert!(
        payload_a_str.contains("CRITICAL_POLICY_A_ONLY"),
        "Session A must contain Policy A"
    );
    assert!(
        !payload_a_str.contains("CRITICAL_POLICY_B_ONLY"),
        "Session A must NOT leak Policy B"
    );

    assert!(
        payload_b_str.contains("CRITICAL_POLICY_B_ONLY"),
        "Session B must contain Policy B"
    );
    assert!(
        !payload_b_str.contains("CRITICAL_POLICY_A_ONLY"),
        "Session B must NOT leak Policy A"
    );

    mock_task.abort();
}

// ============================================================================
// 9. Negative Regression Tests for Tool Pairing Ledger & Multiplicity
// ============================================================================

fn make_test_captured(json: Value) -> CapturedRequest {
    let body = serde_json::to_vec(&json).unwrap();
    CapturedRequest {
        path: "/test".to_string(),
        headers: HeaderMap::new(),
        body,
        json_body: Some(json),
    }
}

#[test]
fn test_tool_pairing_rejects_duplicate_emitted_call_id_in_responses_api() {
    let req = make_test_captured(json!({
        "input": [
            {"type": "function_call", "call_id": "call_123", "name": "test_fn", "arguments": "{}"},
            {"type": "function_call", "call_id": "call_123", "name": "test_fn", "arguments": "{}"},
            {"type": "function_call_output", "call_id": "call_123", "output": "ok"}
        ]
    }));
    let res = verify_captured_tool_pairing_with_error(&[req]);
    assert_eq!(
        res,
        Err(ToolPairingError::DuplicateEmittedCallId {
            call_id: "call_123".to_string(),
            request_index: 0
        })
    );
}

#[test]
fn test_tool_pairing_rejects_duplicate_emitted_call_id_in_chat_completions() {
    let req = make_test_captured(json!({
        "messages": [
            {
                "role": "assistant",
                "tool_calls": [
                    {"id": "call_123", "type": "function", "function": {"name": "test_fn", "arguments": "{}"}},
                    {"id": "call_123", "type": "function", "function": {"name": "test_fn", "arguments": "{}"}}
                ]
            },
            {
                "role": "tool",
                "tool_call_id": "call_123",
                "content": "ok"
            }
        ]
    }));
    let res = verify_captured_tool_pairing_with_error(&[req]);
    assert_eq!(
        res,
        Err(ToolPairingError::DuplicateEmittedCallId {
            call_id: "call_123".to_string(),
            request_index: 0
        })
    );
}

#[test]
fn test_tool_pairing_rejects_duplicate_result_in_responses_api() {
    let req = make_test_captured(json!({
        "input": [
            {"type": "function_call", "call_id": "call_123", "name": "test_fn", "arguments": "{}"},
            {"type": "function_call_output", "call_id": "call_123", "output": "ok1"},
            {"type": "function_call_output", "call_id": "call_123", "output": "ok2"}
        ]
    }));
    let res = verify_captured_tool_pairing_with_error(&[req]);
    assert_eq!(
        res,
        Err(ToolPairingError::DuplicateResult {
            call_id: "call_123".to_string(),
            request_index: 0
        })
    );
}

#[test]
fn test_tool_pairing_rejects_duplicate_result_in_chat_completions() {
    let req = make_test_captured(json!({
        "messages": [
            {
                "role": "assistant",
                "tool_calls": [
                    {"id": "call_123", "type": "function", "function": {"name": "test_fn", "arguments": "{}"}}
                ]
            },
            {"role": "tool", "tool_call_id": "call_123", "content": "ok1"},
            {"role": "tool", "tool_call_id": "call_123", "content": "ok2"}
        ]
    }));
    let res = verify_captured_tool_pairing_with_error(&[req]);
    assert_eq!(
        res,
        Err(ToolPairingError::DuplicateResult {
            call_id: "call_123".to_string(),
            request_index: 0
        })
    );
}

#[test]
fn test_tool_pairing_rejects_orphaned_result_in_responses_api() {
    let req = make_test_captured(json!({
        "input": [
            {"type": "function_call_output", "call_id": "call_orphan", "output": "ok"}
        ]
    }));
    let res = verify_captured_tool_pairing_with_error(&[req]);
    assert_eq!(
        res,
        Err(ToolPairingError::OrphanedResult {
            call_id: "call_orphan".to_string(),
            request_index: 0
        })
    );
}

#[test]
fn test_tool_pairing_rejects_orphaned_result_in_chat_completions() {
    let req = make_test_captured(json!({
        "messages": [
            {"role": "tool", "tool_call_id": "call_orphan", "content": "ok"}
        ]
    }));
    let res = verify_captured_tool_pairing_with_error(&[req]);
    assert_eq!(
        res,
        Err(ToolPairingError::OrphanedResult {
            call_id: "call_orphan".to_string(),
            request_index: 0
        })
    );
}

#[test]
fn test_tool_pairing_rejects_missing_result_in_responses_api() {
    let req = make_test_captured(json!({
        "input": [
            {"type": "function_call", "call_id": "call_missing", "name": "test_fn", "arguments": "{}"}
        ]
    }));
    let res = verify_captured_tool_pairing_with_error(&[req]);
    assert_eq!(
        res,
        Err(ToolPairingError::MissingResult {
            call_id: "call_missing".to_string(),
            request_index: 0
        })
    );
}

#[test]
fn test_tool_pairing_rejects_missing_result_in_chat_completions() {
    let req = make_test_captured(json!({
        "messages": [
            {
                "role": "assistant",
                "tool_calls": [
                    {"id": "call_missing", "type": "function", "function": {"name": "test_fn", "arguments": "{}"}}
                ]
            }
        ]
    }));
    let res = verify_captured_tool_pairing_with_error(&[req]);
    assert_eq!(
        res,
        Err(ToolPairingError::MissingResult {
            call_id: "call_missing".to_string(),
            request_index: 0
        })
    );
}

#[test]
fn test_tool_pairing_rejects_multiplicity_mismatch_hidden_by_set_equality() {
    // Demonstration of the exact user review defect:
    // Emitted call 1: call_123
    // Emitted call 2: call_123
    // Tool result 1:  call_123
    // A HashSet of call IDs would contain {"call_123"} for both calls and results,
    // falsely declaring equality. The occurrence-based ledger must reject this.
    let req = make_test_captured(json!({
        "input": [
            {"type": "function_call", "call_id": "call_123", "name": "test_fn", "arguments": "{}"},
            {"type": "function_call", "call_id": "call_123", "name": "test_fn", "arguments": "{}"},
            {"type": "function_call_output", "call_id": "call_123", "output": "ok"}
        ]
    }));
    assert!(
        !verify_captured_tool_pairing(std::slice::from_ref(&req)),
        "Multiplicity mismatch must fail verify_captured_tool_pairing"
    );
    assert!(
        matches!(
            verify_captured_tool_pairing_with_error(&[req]),
            Err(ToolPairingError::DuplicateEmittedCallId { .. })
                | Err(ToolPairingError::MultiplicityMismatch { .. })
        ),
        "Must return an explicit multiplicity or duplicate emitted call error"
    );
}

#[test]
fn test_tool_pairing_accepts_valid_historical_replay_across_requests() {
    // Request 0: User turn (0 tool calls, 0 results)
    let req0 = make_test_captured(json!({
        "messages": [
            {"role": "user", "content": "Initial user task"}
        ]
    }));

    // Request 1: Agent emits call_1 and receives result_1
    let req1 = make_test_captured(json!({
        "messages": [
            {"role": "user", "content": "Initial user task"},
            {
                "role": "assistant",
                "tool_calls": [
                    {"id": "call_1", "type": "function", "function": {"name": "test_fn", "arguments": "{}"}}
                ]
            },
            {"role": "tool", "tool_call_id": "call_1", "content": "result 1"}
        ]
    }));

    // Request 2: Resumed turn replays call_1 and result_1 in history without duplicating invocations
    let req2 = make_test_captured(json!({
        "messages": [
            {"role": "user", "content": "Initial user task"},
            {
                "role": "assistant",
                "tool_calls": [
                    {"id": "call_1", "type": "function", "function": {"name": "test_fn", "arguments": "{}"}}
                ]
            },
            {"role": "tool", "tool_call_id": "call_1", "content": "result 1"},
            {"role": "assistant", "content": "Task 1 completed"},
            {"role": "user", "content": "Follow-up turn 2 prompt"}
        ]
    }));

    // Request 3: Turn 2 emits call_2 and receives result_2 while replaying historical call_1
    let req3 = make_test_captured(json!({
        "messages": [
            {"role": "user", "content": "Initial user task"},
            {
                "role": "assistant",
                "tool_calls": [
                    {"id": "call_1", "type": "function", "function": {"name": "test_fn", "arguments": "{}"}}
                ]
            },
            {"role": "tool", "tool_call_id": "call_1", "content": "result 1"},
            {"role": "assistant", "content": "Task 1 completed"},
            {"role": "user", "content": "Follow-up turn 2 prompt"},
            {
                "role": "assistant",
                "tool_calls": [
                    {"id": "call_2", "type": "function", "function": {"name": "test_fn_2", "arguments": "{}"}}
                ]
            },
            {"role": "tool", "tool_call_id": "call_2", "content": "result 2"}
        ]
    }));

    let requests = vec![req0, req1, req2, req3];
    assert!(
        verify_captured_tool_pairing(&requests),
        "Legitimate historical replay must pass tool pairing validation"
    );
    assert_eq!(verify_captured_tool_pairing_with_error(&requests), Ok(()));
}

#[tokio::test]
async fn test_hermes_venv_resolution_failure_blocks_qualification_without_executing_launcher() {
    let fixture = WorkloadFixture::create(TaskFamily::F1BugFixing);
    let store = CwStore::open_in_memory().unwrap();
    let captured = Arc::new(Mutex::new(Vec::new()));
    let fake_bin = PathBuf::from("/nonexistent/isolated/path/to/hermes");

    let result = execute_hermes_session(
        &fake_bin,
        40000,
        "test-token",
        "cw114-test-blocked-hermes",
        &fixture,
        &store,
        TaskFamily::F1BugFixing,
        1,
        captured,
    )
    .await;

    assert_eq!(
        result.status, "BLOCKED",
        "Failing to resolve isolated venv interpreter must immediately BLOCK qualification"
    );
    assert!(
        result
            .failure_class
            .as_deref()
            .unwrap_or("")
            .contains("BLOCKED: Failed to resolve isolated virtual environment"),
        "Failure class must document the isolation resolution failure"
    );
}
