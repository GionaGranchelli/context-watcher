use anyhow::{Context, Result};
use cw_core::calibration::{CalibrationMetricsReport, replay_historical_trace_file};
use std::path::{Path, PathBuf};

/// Find existing committed benchmark files relative to repository root or current directory.
fn locate_committed_datasets() -> Vec<PathBuf> {
    let candidates = [
        "docs/cw115_benchmark_data.json",
        "docs/cw115_four_mode_benchmark_data.json",
    ];
    let mut found = Vec::new();
    for rel in &candidates {
        let p = Path::new(rel);
        if p.exists() {
            found.push(p.to_path_buf());
        } else {
            // Try walking up to 3 parent directories
            let mut cur = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let mut resolved = None;
            for _ in 0..3 {
                let check = cur.join(rel);
                if check.exists() {
                    resolved = Some(check);
                    break;
                }
                if !cur.pop() {
                    break;
                }
            }
            if let Some(r) = resolved {
                found.push(r);
            }
        }
    }
    found
}

pub fn handle_calibrate(
    dataset_path: Option<String>,
    horizon: u32,
    json_output: bool,
) -> Result<()> {
    let files = match dataset_path {
        Some(p) => vec![PathBuf::from(p)],
        None => {
            let found = locate_committed_datasets();
            if found.is_empty() {
                anyhow::bail!("No benchmark datasets found. Please specify --dataset <path>");
            }
            found
        }
    };

    let mut reports: Vec<CalibrationMetricsReport> = Vec::new();

    for file in &files {
        let (rep, _) = replay_historical_trace_file(file, horizon)
            .with_context(|| format!("Failed to replay historical dataset at {:?}", file))?;
        reports.push(rep);
    }

    if json_output {
        if reports.len() == 1 {
            println!("{}", serde_json::to_string_pretty(&reports[0])?);
        } else {
            println!("{}", serde_json::to_string_pretty(&reports)?);
        }
        return Ok(());
    }

    // Terminal formatted report
    println!("┌──────────────────────────────────────────────────────────────────────────────┐");
    println!("│ contextWatcher — CW-117B Shadow Policy Empirical Calibration                 │");
    println!("└──────────────────────────────────────────────────────────────────────────────┘");

    for rep in &reports {
        println!();
        println!("  Dataset: {}", rep.dataset_source);
        if let Some(commit) = &rep.git_commit {
            println!("  Git Commit: {}", commit);
        }
        println!(
            "  Sessions Replayed: {} | Total Requests: {}",
            rep.total_sessions, rep.total_requests
        );
        println!("  ────────────────────────────────────────────────────────────────────────────");
        println!("  Cache-State Classification:");
        for (state, count) in &rep.cache_state_distribution {
            println!("    • {:<16} : {} requests", state, count);
        }
        println!(
            "  State Consistency Rate  : {:.2}%",
            rep.cache_agreement.overall_state_consistency_pct
        );
        println!(
            "    - WarmVerified hits retained : {}",
            rep.cache_agreement.warm_verified_retained_cache
        );
        println!(
            "    - WarmVerified drops to 0    : {}",
            rep.cache_agreement.warm_verified_cache_drops
        );
        println!(
            "    - ColdLikely initialization  : {}",
            rep.cache_agreement.cold_likely_subsequent_warm
        );
        println!();
        println!("  Economic Reconciliation (Model Pricing Profile vs Dataset Wire Cost):");
        println!(
            "    - Dataset Reported Cost      : ${:.6}",
            rep.economic_reconciliation.total_dataset_reported_cost_usd
        );
        println!(
            "    - Profile Recalculated Cost  : ${:.6}",
            rep.economic_reconciliation
                .total_profile_recalculated_cost_usd
        );
        println!(
            "    - Absolute Total Variance    : ${:.10}",
            rep.economic_reconciliation.absolute_cost_variance_usd
        );
        println!(
            "    - Max Single-Request Error   : ${:.10}",
            rep.economic_reconciliation.max_single_request_variance_usd
        );
        println!(
            "    - Pricing Profile Coverage   : {:.1}%",
            rep.economic_reconciliation.pricing_profile_coverage_pct
        );
        println!();
        println!("  Policy Decisions & Explanation Breakdown:");
        println!(
            "    - Total Evaluated Decisions  : {}",
            rep.policy_decisions.total_decisions
        );
        println!(
            "    - Defer (Preserve warm cache): {}",
            rep.policy_decisions.defer_count
        );
        println!(
            "    - Apply (Cold initial prompt): {}",
            rep.policy_decisions.apply_count
        );
        println!(
            "    - NoOp / Reject              : {} / {}",
            rep.policy_decisions.no_op_count, rep.policy_decisions.reject_count
        );
        for (reason, count) in &rep.policy_decisions.reasons_breakdown {
            println!("      * {:<32} : {}", reason, count);
        }
        println!(
            "    - Unverifiable Counterfactuals: {} (Hypothetical cold savings labeled unexecuted)",
            rep.policy_decisions.unverifiable_counterfactual_count
        );

        if !rep.discrepancies.is_empty() {
            println!();
            println!("  Empirical Discrepancy Findings (Policy Weaknesses & Limitations):");
            for d in &rep.discrepancies {
                println!("    [{}] Case {}: {}", d.severity, d.case_number, d.title);
                println!("      Session: {} (Req #{})", d.session_id, d.request_index);
                println!("      Phenomenon : {}", d.observed_phenomenon);
                println!("      Implication: {}", d.policy_implication);
                println!("      Remedy     : {}", d.recommendation);
            }
        }
    }

    Ok(())
}
