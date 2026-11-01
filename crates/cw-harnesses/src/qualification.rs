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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HarnessReadinessStatus {
    Ready,
    Warning,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessReadinessReport {
    pub harness: HarnessKind,
    pub binary_name: String,
    pub binary_path: Option<PathBuf>,
    pub version: Option<String>,
    pub is_installed: bool,
    pub credential_isolation_supported: bool,
    pub proxy_routing_supported: bool,
    pub status: HarnessReadinessStatus,
    pub notes: Vec<String>,
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

    let (is_installed, version, status, cred_supported, proxy_supported) = match &bin_path {
        Some(path) => {
            let ver = extract_binary_version(path);
            notes.push(format!("Found {} at {}", binary_name, path.display()));
            if let Some(ref v) = ver {
                notes.push(format!("Reported version: {v}"));
            }

            match harness {
                HarnessKind::Codex => (
                    true,
                    ver,
                    HarnessReadinessStatus::Ready,
                    true, // Codex supports shell_environment_policy.filters.CW_CONTROL_TOKEN="exclude"
                    true, // Responses API proxy routing fully qualified
                ),
                HarnessKind::Omp | HarnessKind::OpenCode => (
                    true,
                    ver,
                    HarnessReadinessStatus::Ready,
                    true, // OMP native runtime supports environment sanitization
                    true, // Completions API proxy routing qualified
                ),
                HarnessKind::Hermes => (
                    true,
                    ver,
                    HarnessReadinessStatus::Ready,
                    true, // Hermes trusted override supports mode 0600 and atomic unlink
                    true, // Plugin proxy routing qualified
                ),
                _ => (true, ver, HarnessReadinessStatus::Warning, false, true),
            }
        }
        None => {
            notes.push(format!("Binary '{binary_name}' not found in PATH"));
            (
                false,
                None,
                HarnessReadinessStatus::Unavailable,
                false,
                false,
            )
        }
    };

    HarnessReadinessReport {
        harness,
        binary_name: binary_name.to_string(),
        binary_path: bin_path,
        version,
        is_installed,
        credential_isolation_supported: cred_supported,
        proxy_routing_supported: proxy_supported,
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
        let codex_report = check_harness_readiness(HarnessKind::Codex);
        // On this host, codex is installed
        assert!(codex_report.is_installed);
        assert_eq!(codex_report.status, HarnessReadinessStatus::Ready);
        assert!(codex_report.credential_isolation_supported);
        assert!(codex_report.proxy_routing_supported);
    }
}
