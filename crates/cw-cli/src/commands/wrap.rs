use anyhow::{anyhow, bail, Result};
use std::path::PathBuf;
use uuid::Uuid;

/// RAII cleanup guard for temporary override handoff files (e.g. for Hermes).
/// Automatically unlinks the tracked file on drop (such as on child exit, write failure, or early return).
pub(crate) struct OverrideFileCleanup(pub(crate) Option<std::path::PathBuf>);

impl OverrideFileCleanup {
    pub(crate) fn new() -> Self {
        Self(None)
    }

    pub(crate) fn track(&mut self, path: std::path::PathBuf) {
        self.0 = Some(path);
    }

    #[cfg(test)]
    pub(crate) fn path(&self) -> Option<&std::path::Path> {
        self.0.as_deref()
    }
}

impl Drop for OverrideFileCleanup {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Validates that a session ID consists of 1..=128 ASCII alphanumeric, underscore, or hyphen characters.
pub(crate) fn valid_proxy_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Strips any `x-cw-*` internal headers from newline-separated header strings.
pub(crate) fn without_cw_headers(existing: &str) -> String {
    let lines: Vec<String> = existing
        .lines()
        .filter(|line| !is_cw_header(line))
        .map(str::to_owned)
        .collect();
    lines.join("\n")
}

/// Strips any `x-cw-*` internal headers from comma-separated header strings.
pub(crate) fn without_cw_comma_separated_headers(existing: &str) -> String {
    let mut headers = Vec::new();
    let mut start = 0;
    for (index, ch) in existing.char_indices() {
        if ch == ','
            && existing[index + 1..]
                .split_once(':')
                .is_some_and(|(name, _)| !name.trim().is_empty() && !name.contains(','))
        {
            headers.push(&existing[start..index]);
            start = index + 1;
        }
    }
    headers.push(&existing[start..]);
    headers
        .into_iter()
        .filter(|header| !is_cw_header(header))
        .map(str::trim)
        .filter(|header| !header.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

fn is_cw_header(header: &str) -> bool {
    header
        .split_once(':')
        .is_some_and(|(name, _)| name.trim().to_ascii_lowercase().starts_with("x-cw-"))
}

pub(crate) fn codex_proxy_config_args(proxy_port: u16) -> Vec<String> {
    cw_harnesses::qualification::codex_proxy_config_args(proxy_port)
}

/// Launches a harness child process with loopback proxy routing and sanitized environment.
pub fn run_wrap(proxy_port: u16, session_id: Option<String>, cmd: Vec<String>) -> Result<()> {
    if cmd.is_empty() {
        bail!("No command specified to wrap");
    }

    let bin = &cmd[0];
    let is_codex = PathBuf::from(bin)
        .file_stem()
        .is_some_and(|name| name.eq_ignore_ascii_case("codex"));
    let is_hermes = PathBuf::from(bin)
        .file_stem()
        .is_some_and(|name| name.eq_ignore_ascii_case("hermes"));
    let is_gemini = PathBuf::from(bin)
        .file_stem()
        .is_some_and(|name| name.eq_ignore_ascii_case("gemini"));

    let mut args = Vec::new();
    if is_codex {
        args.extend(codex_proxy_config_args(proxy_port));
    }

    let mut extracted_session_id = session_id;
    let mut tail_iter = cmd[1..].iter().cloned();
    while let Some(arg) = tail_iter.next() {
        if (is_hermes || extracted_session_id.is_none()) && arg == "--session-id" {
            if let Some(val) = tail_iter.next() {
                extracted_session_id = Some(val);
                continue;
            }
        } else if (is_hermes || extracted_session_id.is_none())
            && let Some(val) = arg.strip_prefix("--session-id=")
        {
            extracted_session_id = Some(val.to_owned());
            continue;
        }
        args.push(arg);
    }

    let session_id = extracted_session_id.or_else(|| std::env::var("CW_SESSION_ID").ok());
    if session_id
        .as_deref()
        .is_some_and(|id| !valid_proxy_session_id(id))
    {
        bail!("Invalid session ID; use 1-128 ASCII letters, digits, '_' or '-'");
    }

    let token = if is_codex {
        Some(cw_core::control::control_token(true)?)
    } else {
        None
    };

    let anthropic_headers =
        without_cw_headers(&std::env::var("ANTHROPIC_CUSTOM_HEADERS").unwrap_or_default());
    let gemini_headers = without_cw_comma_separated_headers(
        &std::env::var("GEMINI_CLI_CUSTOM_HEADERS").unwrap_or_default(),
    );

    let mut process = std::process::Command::new(bin);
    process
        .args(args)
        .env(
            "ANTHROPIC_BASE_URL",
            format!("http://127.0.0.1:{proxy_port}/anthropic"),
        )
        .env(
            "OPENAI_BASE_URL",
            format!("http://127.0.0.1:{proxy_port}/openai/v1"),
        )
        .env(
            "OPENAI_API_BASE",
            format!("http://127.0.0.1:{proxy_port}/openai/v1"),
        )
        .env(
            "GOOGLE_GEMINI_BASE_URL",
            format!("http://127.0.0.1:{proxy_port}/gemini"),
        )
        .env("ANTHROPIC_CUSTOM_HEADERS", anthropic_headers);

    if is_gemini {
        process.env("GEMINI_CLI_CUSTOM_HEADERS", gemini_headers);
    }

    process
        .env_remove("CW_CONTROL_TOKEN")
        .env_remove("CW_CONTROL_TOKEN_FILE");

    if let Some(token) = token {
        process.env("CW_CONTROL_TOKEN", token);
    }

    let mut cleanup_guard = OverrideFileCleanup::new();
    if is_hermes {
        let _ = cw_core::control::control_token(true);
        process.env_remove("CW_SESSION_ID").env(
            "DEEPSEEK_BASE_URL",
            format!("http://127.0.0.1:{proxy_port}/deepseek"),
        );
        process.env("CW_CONTROL_TOKEN_FILE", cw_core::control::token_path());
        if let Some(session_id) = session_id.as_deref() {
            process.env("CW_SESSION_ID", session_id);
            let parent = cw_core::control::token_path()
                .parent()
                .map(|p| p.to_path_buf())
                .ok_or_else(|| anyhow!("Failed to resolve control token parent directory"))?;
            std::fs::create_dir_all(&parent)?;
            let override_uuid = Uuid::new_v4().to_string();
            let override_path = parent.join(format!("hermes-override-{override_uuid}.json"));
            let cwd_str = std::env::current_dir()
                .ok()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            let payload = serde_json::json!({
                "session_id": session_id,
                "workspace": cwd_str,
                "ts": chrono::Utc::now().timestamp(),
            });
            let bytes = serde_json::to_vec(&payload)?;
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            let mut f = opts.open(&override_path)?;
            // Track file for cleanup IMMEDIATELY upon creation so subsequent errors/writes do not leave untracked files
            cleanup_guard.track(override_path.clone());
            use std::io::Write;
            f.write_all(&bytes)?;
            process.env("CW_HERMES_OVERRIDE_FILE", &override_path);
        }
    } else if let Some(session_id) = session_id.as_deref() {
        process.env("CW_SESSION_ID", session_id);
    } else {
        process.env("CW_SESSION_ID", format!("wrap_{}", Uuid::new_v4().simple()));
    }

    let status = process.status();
    // Explicitly drop cleanup_guard before std::process::exit because process::exit does not run Rust destructors
    drop(cleanup_guard);
    let status = status?;
    std::process::exit(status.code().unwrap_or(1));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapper_removes_cw_credentials_from_harness_environment() {
        let headers = without_cw_headers(
            "x-existing: yes\nx-cw-session-id: forged\nX-CW-CONTROL-TOKEN: stale\nX-CW-PRIVATE: value",
        );
        assert_eq!(headers, "x-existing: yes");
        assert_eq!(
            without_cw_comma_separated_headers(
                "x-existing: yes, x-cw-session-id: forged, authorization: Bearer a,b, X-CW-PRIVATE: stale"
            ),
            "x-existing: yes, authorization: Bearer a,b"
        );
        assert!(valid_proxy_session_id("pi_session-12"));
        assert!(!valid_proxy_session_id("../other"));
        assert!(!valid_proxy_session_id(""));
    }

    #[test]
    fn codex_wrapper_uses_proxy_provider_and_environment_backed_headers() {
        let args = codex_proxy_config_args(5432);
        let settings = args.join(" ");
        assert!(settings.contains("model_provider=\"context_watcher\""));
        assert!(settings.contains("127.0.0.1:5432/openai/v1"));
        assert!(settings.contains("x-cw-session-id\"=\"CW_SESSION_ID"));
        assert!(settings.contains("x-cw-control-token\"=\"CW_CONTROL_TOKEN"));
        assert!(settings.contains("env_key=\"OPENAI_API_KEY\""));
        assert!(settings.contains("shell_environment_policy.filters.CW_CONTROL_TOKEN"));
        assert!(!settings.contains("private-token"));
    }

    #[test]
    fn regression_test_override_file_cleanup_on_normal_exit_without_consumption() {
        let temp_dir = std::env::temp_dir().join(format!("cw_wrap_test_{}", Uuid::new_v4()));
        let _ = std::fs::create_dir_all(&temp_dir);
        let override_path = temp_dir.join("hermes-override-unconsumed.json");
        std::fs::write(&override_path, b"{\"test\": true}").unwrap();
        assert!(override_path.exists());

        // Simulate wrapper process lifecycle where child exits without consuming the handoff
        let mut cleanup_guard = OverrideFileCleanup::new();
        cleanup_guard.track(override_path.clone());
        assert_eq!(cleanup_guard.path(), Some(override_path.as_path()));
        // Explicitly drop cleanup_guard before exit
        drop(cleanup_guard);

        assert!(
            !override_path.exists(),
            "Override file must be removed when cleanup guard is dropped on normal exit"
        );
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn regression_test_override_file_cleanup_on_write_failure() {
        let temp_dir =
            std::env::temp_dir().join(format!("cw_wrap_test_write_fail_{}", Uuid::new_v4()));
        let _ = std::fs::create_dir_all(&temp_dir);
        let override_path = temp_dir.join("hermes-override-write-fail.json");

        let simulate_write_failure = || -> std::io::Result<()> {
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create_new(true);
            let mut _f = opts.open(&override_path)?;
            let mut cleanup_guard = OverrideFileCleanup::new();
            cleanup_guard.track(override_path.clone());

            // Simulate write failure returning early via `?`
            Err(std::io::Error::other("simulated write_all disk failure"))?;
            Ok(())
        };

        let result = simulate_write_failure();
        assert!(result.is_err());
        assert!(
            !override_path.exists(),
            "Override file must be removed by cleanup guard if write failure occurs immediately after creation"
        );
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn header_stripping_preserves_complex_non_cw_headers() {
        let input = "authorization: Bearer secret-token\n\
                     X-Custom-Header: custom-val\n\
                     x-cw-session-id: injected-session\n\
                     X-CW-CONTROL-TOKEN: token123\n\
                     x-cw-workspace: /path/to/ws\n\
                     Content-Type: application/json\n\
                     X-CW-DRY-RUN: 1";
        let cleaned = without_cw_headers(input);
        assert_eq!(
            cleaned,
            "authorization: Bearer secret-token\n\
             X-Custom-Header: custom-val\n\
             Content-Type: application/json"
        );
        assert_eq!(without_cw_headers(""), "");
    }

    #[test]
    fn comma_separated_header_stripping_edge_cases() {
        let input = "Authorization: Bearer abc,def, X-CW-SESSION-ID: s1, X-Custom: val:with:colons, x-cw-token: t2, Accept: text/event-stream";
        let cleaned = without_cw_comma_separated_headers(input);
        assert_eq!(
            cleaned,
            "Authorization: Bearer abc,def, X-Custom: val:with:colons, Accept: text/event-stream"
        );
        assert_eq!(without_cw_comma_separated_headers(""), "");
    }

    #[test]
    fn session_id_validation_contract() {
        // Valid ASCII alphanumeric, underscore, hyphen, 1..=128
        assert!(valid_proxy_session_id("a"));
        assert!(valid_proxy_session_id("my_session-123"));
        assert!(valid_proxy_session_id(&"x".repeat(128)));

        // Invalid: empty, >128, path traversal, whitespace, shell metacharacters
        assert!(!valid_proxy_session_id(""));
        assert!(!valid_proxy_session_id(&"x".repeat(129)));
        assert!(!valid_proxy_session_id("../parent"));
        assert!(!valid_proxy_session_id("sess/ion"));
        assert!(!valid_proxy_session_id("sess ion"));
        assert!(!valid_proxy_session_id("sess;rm"));
        assert!(!valid_proxy_session_id("sess\0null"));
        assert!(!valid_proxy_session_id("sess\nnewline"));
    }
}
