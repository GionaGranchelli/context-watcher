use cw_core::ir::HarnessKind;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Returns Codex CLI configuration flags to route requests through contextWatcher's authenticated Responses proxy.
pub fn codex_proxy_config_args(proxy_port: u16) -> Vec<String> {
    [
        "model_provider=\"context_watcher\"".to_owned(),
        "model_providers.context_watcher.name=\"contextWatcher\"".to_owned(),
        format!(
            "model_providers.context_watcher.base_url=\"http://127.0.0.1:{proxy_port}/openai/v1\""
        ),
        "model_providers.context_watcher.wire_api=\"responses\"".to_owned(),
        "model_providers.context_watcher.requires_openai_auth=true".to_owned(),
        "model_providers.context_watcher.env_key=\"OPENAI_API_KEY\"".to_owned(),
        "model_providers.context_watcher.env_http_headers={\"x-cw-session-id\"=\"CW_SESSION_ID\",\"x-cw-control-token\"=\"CW_CONTROL_TOKEN\"}".to_owned(),
        "shell_environment_policy.filters.CW_CONTROL_TOKEN=\"exclude\"".to_owned(),
    ]
    .into_iter()
    .flat_map(|setting| ["--config".to_owned(), setting])
    .collect()
}

/// Returns direct native provider configuration for Codex CLI (bypassing ContextWatcher proxy).
pub fn codex_direct_config_args() -> Vec<String> {
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

/// Generates isolated models.json content for OMP pointing to contextWatcher's proxy.
pub fn omp_proxy_models_config(proxy_port: u16, model_id: &str) -> serde_json::Value {
    serde_json::json!({
        "providers": {
            "cw-proxy": {
                "baseUrl": format!("http://127.0.0.1:{proxy_port}/openai/v1"),
                "api": "openai-completions",
                "apiKey": "cw-authenticated",
                "models": [{
                    "id": model_id,
                    "name": format!("contextWatcher ({model_id})"),
                    "contextWindow": 131072,
                    "maxTokens": 4096
                }]
            }
        }
    })
}

/// Readiness is a ladder; each rung requires evidence beyond the previous one.
/// Static inspection can reach at most `Configured`; `FunctionallyVerified` is only
/// granted via [`mark_functionally_verified`] after an observed proxied request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum HarnessReadinessStatus {
    /// Binary not found on PATH.
    Unavailable,
    /// Binary found but `--version` did not run successfully.
    Detected,
    /// Binary found and executes (`--version` succeeded).
    Installed,
    /// Installed, and contextWatcher can generate an isolated proxy configuration for it.
    Configured,
    /// Configured, and a proxied request through contextWatcher was actually observed.
    FunctionallyVerified,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessReadinessReport {
    pub harness: HarnessKind,
    pub binary_name: String,
    pub binary_path: Option<PathBuf>,
    pub version: Option<String>,
    pub is_installed: bool,
    /// The adapter *declares* credential isolation (e.g. env filters). Declared, not verified.
    pub credential_isolation_declared: bool,
    /// contextWatcher can generate an isolated proxy-routing config for this harness.
    pub proxy_config_available: bool,
    pub functionally_verified: bool,
    pub verification_evidence: Option<String>,
    pub status: HarnessReadinessStatus,
    pub notes: Vec<String>,
}

/// Promote a `Configured` report to `FunctionallyVerified` given observed evidence
/// (e.g. a session id whose requests were recorded by the proxy). Refuses otherwise.
pub fn mark_functionally_verified(
    report: &mut HarnessReadinessReport,
    evidence: impl Into<String>,
) -> bool {
    if report.status != HarnessReadinessStatus::Configured {
        return false;
    }
    report.functionally_verified = true;
    report.verification_evidence = Some(evidence.into());
    report.status = HarnessReadinessStatus::FunctionallyVerified;
    true
}

/// Discovers a binary by name across standard PATH directories.
pub fn find_binary(binary_name: &str) -> Option<PathBuf> {
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join(binary_name);
            if candidate.is_file() {
                // Check if executable (Unix check)
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if let Ok(meta) = candidate.metadata()
                        && meta.permissions().mode() & 0o111 != 0
                    {
                        return Some(candidate);
                    }
                }
                #[cfg(not(unix))]
                return Some(candidate);
            }
        }
    }
    None
}

/// Extracts version from a binary using `--version`.
pub fn extract_binary_version(binary_path: &Path) -> Option<String> {
    let output = Command::new(binary_path).arg("--version").output().ok()?;
    if output.status.success() {
        let text = String::from_utf8_lossy(&output.stdout);
        let trimmed = text.lines().next().unwrap_or("").trim().to_string();
        if !trimmed.is_empty() {
            return Some(trimmed);
        }
    }
    None
}

/// Evaluates harness readiness for controlled live execution.
pub fn check_harness_readiness(harness: HarnessKind) -> HarnessReadinessReport {
    let binary_name = match harness {
        HarnessKind::Codex => "codex",
        HarnessKind::Omp | HarnessKind::OpenCode => "omp",
        HarnessKind::Hermes => "hermes",
        HarnessKind::Aider => "aider",
        HarnessKind::ClaudeCode => "claude",
        HarnessKind::Pi => "pi",
        HarnessKind::Antigravity => "agy",
        HarnessKind::GenericProxy => "proxy",
    };

    let bin_path = find_binary(binary_name);
    let mut notes = Vec::new();
    let version = bin_path.as_deref().and_then(extract_binary_version);
    // Only adapters with a generated, isolated proxy config count as configurable.
    let (cred_declared, proxy_config) = match harness {
        HarnessKind::Codex => (true, true), // codex_proxy_config_args + env filter
        HarnessKind::Omp | HarnessKind::OpenCode => (false, true), // omp_proxy_models_config
        _ => (false, false),
    };

    let status = match (&bin_path, &version) {
        (None, _) => {
            notes.push(format!("Binary '{binary_name}' not found in PATH"));
            HarnessReadinessStatus::Unavailable
        }
        (Some(path), None) => {
            notes.push(format!("Found {} but `--version` failed", path.display()));
            HarnessReadinessStatus::Detected
        }
        (Some(path), Some(v)) => {
            notes.push(format!("Found {} ({v})", path.display()));
            if proxy_config {
                HarnessReadinessStatus::Configured
            } else {
                notes.push("No generated isolated proxy config for this harness".into());
                HarnessReadinessStatus::Installed
            }
        }
    };
    if status >= HarnessReadinessStatus::Configured {
        notes.push("Not functionally verified: no proxied request observed".into());
    }

    HarnessReadinessReport {
        harness,
        binary_name: binary_name.to_string(),
        is_installed: version.is_some(),
        binary_path: bin_path,
        version,
        credential_isolation_declared: cred_declared && status >= HarnessReadinessStatus::Installed,
        proxy_config_available: proxy_config,
        functionally_verified: false,
        verification_evidence: None,
        status,
        notes,
    }
}

/// Evaluates a suite of harnesses and returns an inventory.
pub fn audit_all_harness_readiness() -> Vec<HarnessReadinessReport> {
    vec![
        check_harness_readiness(HarnessKind::Codex),
        check_harness_readiness(HarnessKind::OpenCode),
        check_harness_readiness(HarnessKind::Hermes),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_codex_proxy_config_args_structure() {
        let args = codex_proxy_config_args(4401);
        assert!(args.contains(&"--config".to_string()));
        assert!(
            args.iter()
                .any(|a| a.contains("http://127.0.0.1:4401/openai/v1"))
        );
        assert!(
            args.iter().any(
                |a| a.contains("shell_environment_policy.filters.CW_CONTROL_TOKEN=\"exclude\"")
            )
        );
    }

    #[test]
    fn test_codex_direct_config_args_structure() {
        let args = codex_direct_config_args();
        assert!(args.contains(&"--config".to_string()));
        assert!(args.iter().any(|a| a.contains("https://api.openai.com/v1")));
        assert!(
            args.iter()
                .any(|a| a.contains("model_provider=\"openai_direct\""))
        );
    }

    #[test]
    fn test_omp_proxy_models_config_structure() {
        let cfg = omp_proxy_models_config(4401, "gpt-4o-mini");
        assert_eq!(
            cfg["providers"]["cw-proxy"]["baseUrl"],
            "http://127.0.0.1:4401/openai/v1"
        );
        assert_eq!(
            cfg["providers"]["cw-proxy"]["models"][0]["id"],
            "gpt-4o-mini"
        );
    }

    #[test]
    fn test_find_binary_existing_and_nonexistent() {
        // python3 is always present
        let py = find_binary("python3");
        assert!(py.is_some());

        // nonexistent binary
        let none = find_binary("nonexistent_binary_xyz_123");
        assert!(none.is_none());
    }

    #[test]
    fn test_check_harness_readiness_on_system() {
        let mut r = check_harness_readiness(HarnessKind::Codex);
        // Static checks never claim functional verification.
        assert!(!r.functionally_verified);
        assert_ne!(r.status, HarnessReadinessStatus::FunctionallyVerified);
        if r.is_installed {
            assert_eq!(r.status, HarnessReadinessStatus::Configured);
            assert!(mark_functionally_verified(
                &mut r,
                "session sess_x observed"
            ));
            assert_eq!(r.status, HarnessReadinessStatus::FunctionallyVerified);
        } else {
            assert!(r.status < HarnessReadinessStatus::Installed);
            assert!(!mark_functionally_verified(&mut r, "x"));
        }
    }

    #[test]
    fn test_unconfigurable_harness_never_reaches_configured() {
        let mut r = check_harness_readiness(HarnessKind::Hermes);
        assert!(r.status <= HarnessReadinessStatus::Installed);
        assert!(!mark_functionally_verified(&mut r, "x"));
    }
}
