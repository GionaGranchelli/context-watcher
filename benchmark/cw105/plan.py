"""Build a reviewable CW-105 proposal without contacting the provider."""
import json
import hashlib
from pathlib import Path

from authorization import ENDPOINT, MODEL, PROVIDER, plan_digest
from schedule import schedule

ATTEMPT_OVERHEAD_SECONDS = 600
SMOKE_MAX_REQUESTS = 4
SMOKE_MAX_SECONDS = 600
MCP_CALLS_PER_RUN = 64


def proposal(campaign_type="full"):
    if campaign_type not in ("full", "pilot"):
        raise ValueError("Campaign type must be full or pilot")
    tasks = json.loads(Path(__file__).with_name("tasks.json").read_text())
    runs = [
        dict(entry, **tasks[entry["task"]])
        for entry in schedule()
    ]
    if campaign_type == "pilot":
        runs = [row for row in runs
                if row["task"] == "board-query" and row["repetition"] == 1]
    runs = [dict(row, campaign_type=campaign_type) for row in runs]
    tasks_requests = sum(run["request_cap"] for run in runs)
    agent_seconds = sum(run["budget_seconds"] for run in runs)
    return {
        "version": 1,
        "campaign_type": campaign_type,
        "debug_seed_sha256": hashlib.sha256(
            Path(__file__).with_name("fixtures").joinpath("debug-seed.patch").read_bytes()
        ).hexdigest(),
        "provider": PROVIDER,
        "benchmark_revision": None,
        "binaries_sha256": None,
        "endpoint": ENDPOINT,
        "model": MODEL,
        "concurrency": 1,
        "max_task_requests": tasks_requests,
        "mcp_calls_per_run": MCP_CALLS_PER_RUN,
        "max_mcp_calls": sum(run["mode"] == "M2" for run in runs) * MCP_CALLS_PER_RUN,
        "max_agent_seconds": agent_seconds,
        "max_requests": tasks_requests + SMOKE_MAX_REQUESTS,
        "max_admission_window_seconds": agent_seconds + len(runs) * ATTEMPT_OVERHEAD_SECONDS + SMOKE_MAX_SECONDS,
        "wall_time_contract": "Request-admission deadline; finalization and surviving-process cleanup may overrun",
        "smoke_max_requests": SMOKE_MAX_REQUESTS,
        "smoke_max_seconds": SMOKE_MAX_SECONDS,
        "attempt_overhead_seconds": ATTEMPT_OVERHEAD_SECONDS,
        "provider_cost_cap_nanodollars": 0,
        "local_resource_cap": {
            "gpu_watts": 707,
            "junction_c": 80,
            "sample_max_age_seconds": 5,
        },
        "pricing": {
            "input_nanodollars_per_token": 0,
            "output_nanodollars_per_token": 0,
            "evidence": "https://lmstudio.ai/docs/app/offline and Machinist lms ps (Local)",
        },
        "cache_policy": "Report observed usage and unknown initial cache state",
        "retry_policy": "No harness-level retries; record any OMP retries as actual requests",
        "value_decision_rule": {
            "minimum_correct_m1_m2_pairs": 9,
            "minimum_pairs_per_task": 3,
            "minimum_m2_virtual_memory_runs_per_task": 3,
            "minimum_median_input_token_reduction_fraction": 0.10,
            "go_requires_no_completion_or_correctness_regression_by_task": True,
            "context_pressure_threshold_fraction": 0.80,
            "minimum_context_pressure_pairs": 3,
            "insufficient_evidence": "ITERATE",
            "qualified_campaign_without_input_token_target": "NO-GO",
        },
        "service_tier": "Server default; no service tier override",
        "status": "not_ready_for_authorization",
        "runs": runs,
    }


if __name__ == "__main__":
    import argparse

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--campaign", choices=("pilot", "full"), default="full")
    args = parser.parse_args()
    plan = proposal(args.campaign)
    print(json.dumps({"plan_sha256": plan_digest(plan), "plan": plan}, indent=2))
