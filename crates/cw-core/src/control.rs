use std::{
    fs,
    io::{self, Write},
    path::PathBuf,
};

/// Canonical IDs carried in authenticated `x-cw-session-id` headers.
pub fn is_valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

pub fn token_path() -> PathBuf {
    std::env::var_os("CW_CONTROL_TOKEN_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::home_dir().join(".local/share/context-watcher/control-token"))
}

pub fn control_token(create: bool) -> io::Result<String> {
    if let Ok(token) = std::env::var("CW_CONTROL_TOKEN") {
        return validate(token);
    }
    let path = token_path();
    match fs::symlink_metadata(&path) {
        Ok(meta) => {
            if !meta.file_type().is_file() {
                return Err(io::Error::other(
                    "Control token must be a regular private file",
                ));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if meta.permissions().mode() & 0o077 != 0 {
                    return Err(io::Error::other("Control token permissions must be 0600"));
                }
            }
            return validate(fs::read_to_string(path)?.trim().to_string());
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound && create => {}
        Err(error) => return Err(error),
    }
    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        }
    }
    let token = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(&path) {
        Ok(mut file) => {
            file.write_all(token.as_bytes())?;
            file.sync_all()?;
            Ok(token)
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => control_token(false),
        Err(error) => Err(error),
    }
}
fn validate(token: String) -> io::Result<String> {
    if token.len() < 32
        || token.len() > 128
        || !token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(io::Error::other(
            "Control token must contain 32 to 128 URL-safe characters",
        ));
    }
    Ok(token)
}

/// Validate an authenticated operator policy snapshot before displaying or acting on it.
pub fn validate_pin_policy_preview(
    body: serde_json::Value,
    session: &str,
) -> io::Result<serde_json::Value> {
    if body["session_id"].as_str() != Some(session)
        || !body["enabled"].is_boolean()
        || !body["session_terminated"].is_boolean()
        || !body["reinforcement_supported"].is_boolean()
        || body["risk_warning"]
            .as_str()
            .is_none_or(|s| s.trim().is_empty())
        || body["activation_scope"]
            .as_str()
            .is_none_or(|s| s.trim().is_empty())
        || !body["preview_basis"].is_string()
        || !body["stored_pins"].is_array()
        || !body["automatic_generation"]["may_generate_after_opt_in"].is_boolean()
        || !body["automatic_generation"]["auto_cache_safe_pin"].is_boolean()
        || !body["automatic_generation"]["auto_heal_low_health"].is_boolean()
        || !body["automatic_generation"]["heal_strategy"].is_string()
    {
        return Err(io::Error::other(
            "Invalid or unavailable policy preview; consent state is unconfirmed",
        ));
    }
    for pin in body["stored_pins"]
        .as_array()
        .ok_or_else(|| io::Error::other("Missing stored pins"))?
    {
        if !pin["rule_id"].is_string()
            || !matches!(
                pin.get("eligible_in_latest_request_preview_after_opt_in"),
                Some(serde_json::Value::Null | serde_json::Value::Bool(_))
            )
            || !pin["label"].is_string()
            || !pin["content"].is_string()
            || !matches!(
                pin["status"].as_str(),
                Some("draft" | "committed" | "disabled")
            )
        {
            return Err(io::Error::other("Invalid stored pin in policy preview"));
        }
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    #[test]
    fn authenticated_session_id_contract() {
        for valid in ["omp_123-ABC".to_string(), "a".repeat(128)] {
            assert!(super::is_valid_session_id(&valid));
        }
        for invalid in [
            "".to_string(),
            "a".repeat(129),
            "bad/id".into(),
            "bad id".into(),
            "é".into(),
            "bad\nheader".into(),
        ] {
            assert!(!super::is_valid_session_id(&invalid));
        }
    }
}
