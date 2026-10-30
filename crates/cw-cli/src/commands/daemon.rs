use anyhow::Result;

/// Checks if the daemon is currently listening on loopback `127.0.0.1:{port}`.
pub fn is_daemon_running(port: u16) -> bool {
    use std::net::TcpStream;
    use std::time::Duration;
    if let Ok(addr) = format!("127.0.0.1:{port}").parse() {
        TcpStream::connect_timeout(&addr, Duration::from_millis(150)).is_ok()
    } else {
        false
    }
}

/// Starts the contextWatcher daemon detached in the background.
/// Checks systemd user service first on Linux; falls back to spawning detached child process with `cw.pid`/`cw.log`.
pub fn start_daemon_detached(port: u16, proxy_port: u16, workspace: Option<String>) -> Result<()> {
    if is_daemon_running(port) {
        println!("contextWatcher daemon is already running in background.");
        println!("  • Web UI : http://127.0.0.1:{port}");
        println!("  • Proxy  : http://127.0.0.1:{proxy_port}");
        return Ok(());
    }

    // Try systemd user service first if available on Linux
    let has_systemctl = std::process::Command::new("systemctl")
        .args(["--user", "is-enabled", "contextwatcher.service"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if has_systemctl {
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "start", "contextwatcher.service"])
            .status();
        std::thread::sleep(std::time::Duration::from_millis(300));
        if is_daemon_running(port) {
            println!("⚡ Started contextWatcher background daemon via systemd user service.");
            println!("   • Web UI : http://127.0.0.1:{port}");
            println!("   • Proxy  : http://127.0.0.1:{proxy_port}");
            return Ok(());
        }
    }

    // Fallback: spawn detached child process with ~/.cw/cw.log
    let cw_dir = cw_core::home_dir().join(".cw");
    std::fs::create_dir_all(&cw_dir)?;
    let log_path = cw_dir.join("cw.log");
    let pid_path = cw_dir.join("cw.pid");

    let exe = std::env::current_exe()?;
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    let err_file = log_file.try_clone()?;

    let mut cmd = std::process::Command::new(exe);
    cmd.args([
        "serve",
        "--port",
        &port.to_string(),
        "--proxy-port",
        &proxy_port.to_string(),
    ]);
    if let Some(ws) = workspace {
        cmd.args(["--workspace", &ws]);
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(log_file))
        .stderr(std::process::Stdio::from(err_file));

    let child = cmd.spawn()?;
    let pid = child.id();
    std::fs::write(&pid_path, pid.to_string())?;

    for _ in 0..15 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        if is_daemon_running(port) {
            break;
        }
    }

    if is_daemon_running(port) {
        println!("⚡ Started contextWatcher daemon in background (PID: {pid})");
        println!("   • Web UI : http://127.0.0.1:{port}");
        println!("   • Proxy  : http://127.0.0.1:{proxy_port}");
        println!("   • Logs   : {}", log_path.display());
    } else {
        println!(
            "⚠️ Daemon process spawned (PID: {pid}), waiting for port {port}. Logs: {}",
            log_path.display()
        );
    }

    Ok(())
}

/// Stops the contextWatcher daemon via systemd user service and PID file.
pub fn stop_daemon(port: u16) -> Result<()> {
    // 1. Try systemd user service stop
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "stop", "contextwatcher.service"])
        .status();

    // 2. Also check ~/.cw/cw.pid
    let pid_path = cw_core::home_dir().join(".cw/cw.pid");
    if let Ok(pid_str) = std::fs::read_to_string(&pid_path) {
        let p = pid_str.trim();
        #[cfg(windows)]
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/PID", p])
            .status();
        #[cfg(not(windows))]
        let _ = std::process::Command::new("kill").arg(p).status();
        let _ = std::fs::remove_file(&pid_path);
    }

    std::thread::sleep(std::time::Duration::from_millis(200));
    if !is_daemon_running(port) {
        println!("✓ contextWatcher background daemon stopped.");
    } else {
        println!("⚠️ Daemon still active on port {port}.");
    }
    Ok(())
}

/// Restarts the background daemon.
pub fn restart_daemon(port: u16, proxy_port: u16, workspace: Option<String>) -> Result<()> {
    stop_daemon(port)?;
    start_daemon_detached(port, proxy_port, workspace)?;
    Ok(())
}

/// Prints current daemon status and PID if running.
pub fn status_daemon(port: u16) -> Result<()> {
    let running = is_daemon_running(port);
    let pid_path = cw_core::home_dir().join(".cw/cw.pid");
    let pid = std::fs::read_to_string(&pid_path)
        .ok()
        .map(|s| s.trim().to_string());

    if running {
        println!("🟢 contextWatcher daemon is RUNNING (Web UI: http://127.0.0.1:{port})");
        if let Some(p) = pid {
            println!("   PID: {p}");
        }
    } else {
        println!("🔴 contextWatcher daemon is NOT running.");
        println!("   Run `cw daemon start` to start it in the background.");
    }
    Ok(())
}
