use std::{
    fs,
    io::{self, Write},
    path::PathBuf,
};

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
