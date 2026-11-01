use anyhow::{Context, Result};
use cw_core::calibration::{
    CalibrationMetricsReport, ProspectiveCalibrationReport, build_prospective_calibration_report,
    generate_synthetic_prospective_fixture, replay_historical_trace_file,
};
use cw_store::CwStore;
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
    synthetic: bool,
    session: Option<String>,
    json_output: bool,
    db_path: &Path,
) -> Result<()> {
    if synthetic {
        let (_decisions, _outcomes, _correlations, _horizons, report) =
            generate_synthetic_prospective_fixture();
        if json_output {
            println!("{}", serde_json::to_string_pretty(&report)?);
            return Ok(());
        }
        render_prospective_report(&report);
        return Ok(());
    }

    if let Some(session_id) = session {
        let store = CwStore::open(db_path)?;
        let decisions = store.list_shadow_decision_evidence(Some(&session_id))?;
        let turns = store.list_turns(&session_id)?;
        let session_info = store.get_session(&session_id)?;
        let (provider, model) = session_info
            .map(|s| (s.provider, s.model))
            .unwrap_or((cw_core::ir::ProviderKind::Anthropic, "unknown".to_string()));
        let pricing = cw_core::policy::find_pricing_profile(provider, &model);
        let mut outcomes = Vec::new();
        for t in &turns {
            outcomes.push(
                cw_core::calibration::ObservedProviderOutcome::from_completed_turn(
                    t,
                    provider,
                    &model,
                    pricing.as_ref(),
                    t.created_at,
                ),
            );
        }
        let correlations = store.list_correlated_decision_outcomes(Some(&session_id))?;
        let horizons = store.list_prospective_horizon_observations(Some(&session_id))?;

        let report = build_prospective_calibration_report(
            &decisions,
            &outcomes,
            &correlations,
            &horizons,
            None,
            None,
        );
        if json_output {
            println!("{}", serde_json::to_string_pretty(&report)?);
            return Ok(());
        }
        render_prospective_report(&report);
        return Ok(());
    }

    let files = match dataset_path {
        Some(p) => vec![PathBuf::from(p)],
        None => {
            let found = locate_committed_datasets();
            if found.is_empty() {
                anyhow::bail!(
                    "No benchmark datasets found. Please specify --dataset <path> or use --synthetic"
                );
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
            "  Transition Analysis (Eligible non-terminal transitions: {} | Excluded terminal: {}):",
            rep.cache_agreement.total_eligible_transitions,
            rep.cache_agreement.terminal_requests_excluded
        );
        println!(
            "    - Warm Cache Retention Rate : {:.2}% ({}/{} transitions retained hits)",
            rep.cache_agreement.warm_retention_pct,
            rep.cache_agreement.warm_to_warm_retained,
            rep.cache_agreement.warm_verified_transitions
        );
        println!(
            "    - Warm Cache Drops to 0     : {} requests",
            rep.cache_agreement.warm_to_cold_drops
        );
        println!(
            "    - Cold Priming Rate         : {:.2}% ({}/{} initialized warm hits)",
            rep.cache_agreement.cold_primed_pct,
            rep.cache_agreement.cold_to_warm_primed,
            rep.cache_agreement.cold_likely_transitions
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
            rep.policy_decisions.total_evaluated_decisions
        );
        println!(
            "    - Omitted (Missing Candidates): {}",
            rep.policy_decisions.not_evaluated_missing_candidates_count
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

fn render_prospective_report(rep: &ProspectiveCalibrationReport) {
    println!("┌──────────────────────────────────────────────────────────────────────────────┐");
    println!("│ contextWatcher — CW-117C Prospective Shadow Calibration                      │");
    println!("└──────────────────────────────────────────────────────────────────────────────┘");
    println!();
    println!("  Report Title: {}", rep.report_title);
    if let Some(commit) = &rep.git_commit {
        println!("  Git Commit  : {}", commit);
    }
    println!("  Generated At: {}", rep.generated_at.to_rfc3339());
    println!("  ────────────────────────────────────────────────────────────────────────────");
    println!(
        "  [1] Measured Provider Telemetry (Observed Requests: {}):",
        rep.measured.total_requests_observed
    );
    println!(
        "    - Total Measured Cost        : ${:.6}",
        rep.measured.total_measured_cost_usd
    );
    println!(
        "    - Total Measured Tokens      : {} cached / {} uncached ({} total input)",
        rep.measured.total_measured_cached_tokens,
        rep.measured.total_measured_uncached_tokens,
        rep.measured.total_measured_input_tokens
    );
    println!(
        "    - Overall Measured Hit Ratio : {:.2}%",
        rep.measured.overall_measured_cache_hit_ratio * 100.0
    );
    if let Some(ttfb) = rep.measured.mean_ttfb_ms {
        println!("    - Mean TTFB                  : {:.1} ms", ttfb);
    }
    if let Some(lat) = rep.measured.mean_request_latency_ms {
        println!("    - Mean Request Latency       : {:.1} ms", lat);
    }
    println!();
    println!(
        "  [2] Modeled Policy Projections (Total Evaluations: {}):",
        rep.modeled.total_evaluations
    );
    println!(
        "    - Decisions                  : {} Apply | {} Defer | {} Reject | {} Ignore",
        rep.modeled.decisions_apply,
        rep.modeled.decisions_defer,
        rep.modeled.decisions_reject,
        rep.modeled.decisions_ignore
    );
    println!(
        "    - Modeled Baseline Cost      : ${:.6}",
        rep.modeled.modeled_baseline_cost_usd
    );
    println!(
        "    - Modeled Optimized Cost     : ${:.6}",
        rep.modeled.modeled_optimized_cost_usd
    );
    println!(
        "    - Projected Gross Savings    : ${:.6}",
        rep.modeled.projected_gross_savings_usd
    );
    for (reason, count) in &rep.modeled.reasons_breakdown {
        println!("      * {:<36} : {}", reason, count);
    }
    println!();
    println!("  [3] Unverifiable Counterfactuals:");
    println!(
        "    - Deferred Candidates        : {}",
        rep.unverifiable.deferred_candidates_count
    );
    println!(
        "    - Rejected Candidates        : {}",
        rep.unverifiable.rejected_candidates_count
    );
    println!(
        "    - Rationale                  : {}",
        rep.unverifiable.unverifiable_reason
    );
    for note in &rep.unverifiable.notes {
        println!("      * {}", note);
    }
    println!();
    println!("  [4] Decision-to-Outcome Correlation:");
    println!(
        "    - Total Pre-Dispatch Decisions : {}",
        rep.correlation.total_decisions
    );
    println!(
        "    - Matched Immediate Outbound   : {}",
        rep.correlation.matched_immediate
    );
    println!(
        "    - Cache State Consistent       : {} requests",
        rep.correlation.cache_state_consistent_count
    );
    println!(
        "    - Cache State Inconsistent     : {} requests",
        rep.correlation.cache_state_inconsistent_count
    );
    println!(
        "    - Mean Single-Turn Cost Delta  : ${:.6}",
        rep.correlation.mean_cost_delta_usd
    );
    println!();
    println!(
        "  [5] Multi-Turn Horizon Tracking (Horizons: {}):",
        rep.horizon.total_horizons_tracked
    );
    println!(
        "    - Fully Observed Horizons      : {}",
        rep.horizon.fully_observed_horizons
    );
    println!(
        "    - Truncated (Session End)      : {}",
        rep.horizon.truncated_horizons
    );
    println!(
        "    - Mean Observed Turns          : {:.1}",
        rep.horizon.mean_horizon_observed_turns
    );
    println!(
        "    - Observed Cache Retention     : {:.2}%",
        rep.horizon.overall_cache_retention_ratio * 100.0
    );
    println!(
        "    - Warm Cache Drops Detected    : {}",
        rep.horizon.warm_drops_detected
    );
    println!();
    println!("  [6] Integrity & Safety Invariants:");
    for note in &rep.notes {
        println!("    • {}", note);
    }
}
