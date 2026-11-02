use anyhow::Result;
use cw_core::ir::HarnessKind;
use cw_harnesses::qualification::{
    HarnessReadinessReport, HarnessReadinessStatus, audit_all_harness_readiness,
    check_harness_readiness,
};
use serde::Serialize;

#[derive(Serialize)]
struct ReadinessSummary {
    live_budget_authorized_usd: Option<f64>,
    harnesses: Vec<HarnessReadinessReport>,
}

pub fn run_readiness(harness: String, json_mode: bool) -> Result<()> {
    let reports = match harness.to_lowercase().as_str() {
        "all" => audit_all_harness_readiness(),
        "codex" => vec![check_harness_readiness(HarnessKind::Codex)],
        "omp" | "opencode" => vec![check_harness_readiness(HarnessKind::OpenCode)],
        "hermes" => vec![check_harness_readiness(HarnessKind::Hermes)],
        "claude" => vec![check_harness_readiness(HarnessKind::ClaudeCode)],
        "aider" => vec![check_harness_readiness(HarnessKind::Aider)],
        "pi" => vec![check_harness_readiness(HarnessKind::Pi)],
        "agy" | "antigravity" => vec![check_harness_readiness(HarnessKind::Antigravity)],
        other => {
            eprintln!("Unknown harness '{other}'. Checking all standard harnesses...");
            audit_all_harness_readiness()
        }
    };

    let budget = cw_core::spend_guard::parse_authorization(
        std::env::var("CW_LIVE_AUTH_USD")
            .or_else(|_| std::env::var("CW_BUDGET_LIMIT_USD"))
            .ok()
            .as_deref(),
    )
    .ok();

    if json_mode {
        let summary = ReadinessSummary {
            live_budget_authorized_usd: budget,
            harnesses: reports,
        };
        println!("{}", serde_json::to_string_pretty(&summary)?);
        return Ok(());
    }

    println!("┌────────────────────────────────────────────────────────────────────────┐");
    println!("│ contextWatcher (`cw readiness`) — Live Harness & Budget Preconditions  │");
    println!("└────────────────────────────────────────────────────────────────────────┘");

    let budget_str = match budget {
        Some(b) => format!("${b:.2} USD (AUTHORIZED)"),
        None => "UNAUTHORIZED ($0.00 / fail-closed)".to_string(),
    };
    println!("  Budget Safeguard Envelope : {budget_str}");
    println!();

    for r in &reports {
        let status_badge = match r.status {
            HarnessReadinessStatus::FunctionallyVerified => "🟢 FUNCTIONALLY VERIFIED",
            HarnessReadinessStatus::Configured => "🟡 CONFIGURED (not functionally verified)",
            HarnessReadinessStatus::Installed => "🟡 INSTALLED (no isolated proxy config)",
            HarnessReadinessStatus::Detected => "🟠 DETECTED (binary does not run)",
            HarnessReadinessStatus::Unavailable => "🔴 UNAVAILABLE",
        };
        let ver = r.version.as_deref().unwrap_or("unknown");
        let bin = r
            .binary_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "not found".to_string());

        println!("  • {:<12} : {status_badge}", format!("{:?}", r.harness));
        println!("    Binary Path  : {bin}");
        println!("    Version      : {ver}");
        println!(
            "    Credentials  : {}",
            if r.credential_isolation_declared {
                "Declared by adapter (CW_CONTROL_TOKEN env filter); not verified"
            } else {
                "Not declared"
            }
        );
        println!(
            "    Proxy Route  : {}",
            if r.proxy_config_available {
                "Isolated config available; not verified"
            } else {
                "No generated config"
            }
        );
        for n in &r.notes {
            println!("    Note         : {n}");
        }
        println!();
    }

    Ok(())
}
