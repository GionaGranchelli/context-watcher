#[test]
fn init_dry_run_leaves_filesystem_and_systemd_untouched() {
    let unique = uuid::Uuid::new_v4().to_string();
    let temp_root = std::env::temp_dir().join(format!("cw_dry_run_test_{unique}"));
    let temp_home = temp_root.join("home");
    let stub_bin_dir = temp_root.join("bin");
    std::fs::create_dir_all(&temp_home).unwrap();
    std::fs::create_dir_all(&stub_bin_dir).unwrap();

    let stub_log = temp_root.join("service_invoked.log");
    let stub_systemctl = stub_bin_dir.join("systemctl");
    let stub_script = format!(
        "#!/bin/sh\necho \"$@\" >> \"{}\"\nexit 0\n",
        stub_log.display()
    );
    std::fs::write(&stub_systemctl, stub_script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&stub_systemctl).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&stub_systemctl, perms).unwrap();
    }

    let orig_path = std::env::var("PATH").unwrap_or_default();
    let new_path = format!("{}:{orig_path}", stub_bin_dir.display());

    let cw_bin = env!("CARGO_BIN_EXE_cw");
    let status = std::process::Command::new(cw_bin)
        .args(["init", "--dry-run"])
        .env("HOME", &temp_home)
        .env("USERPROFILE", &temp_home)
        .env("PATH", new_path)
        .status()
        .expect("Failed to execute cw init --dry-run");

    assert!(status.success(), "cw init --dry-run must exit with code 0");

    // 1. Stubbed system service command was NEVER called
    assert!(
        !stub_log.exists(),
        "Dry-run must not invoke systemctl or modify system services"
    );

    // 2. No plugin or config files created in isolated test HOME
    assert!(
        !temp_home.join(".local/bin/cw").exists(),
        "Dry-run must not install binary to ~/.local/bin/cw"
    );
    assert!(
        !temp_home
            .join(".gemini/config/plugins/contextwatcher")
            .exists(),
        "Dry-run must not install Antigravity plugin"
    );
    assert!(
        !temp_home
            .join(".gemini/config/skills/context-watcher")
            .exists(),
        "Dry-run must not install global Antigravity skills"
    );
    assert!(
        !temp_home.join(".claude/commands/cw.md").exists(),
        "Dry-run must not write Claude commands"
    );
    assert!(
        !temp_home
            .join(".claude/commands/context-watcher.md")
            .exists(),
        "Dry-run must not write Claude commands"
    );
    assert!(
        !temp_home.join(".hermes/config.yaml").exists(),
        "Dry-run must not write or touch Hermes config"
    );

    let _ = std::fs::remove_dir_all(&temp_root);
}
