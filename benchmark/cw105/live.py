"""Run the scored benchmark only with a reviewed, authorized execution plan."""
import argparse
import json
from functools import partial
from pathlib import Path

from agent import CONTEXT_WINDOW, MAX_OUTPUT_TOKENS
from budget import Budget
from campaign import execute_plan, run_attempt
from live_transport import authorize_transport
from machinist import read_sample
from plan import MODEL, proposal
from report import summarize
from request_policy import check_request
from schedule import schedule
from spend import Spend
from verify_task import verify_task


def run_live(plan, document, *, checkout, merged_revision, repository, output, omp, cw):
    if plan.get("status") != "ready_for_authorization":
        raise ValueError("Live execution plan has not passed readiness review")
    frozen = proposal()
    for field in ("runs", "max_requests", "max_wall_seconds", "concurrency"):
        if plan.get(field) != frozen[field]:
            raise ValueError(f"Execution envelope differs from the frozen {field}")
    if output.exists():
        raise FileExistsError(output)
    rates = plan["pricing"]
    if not rates.get("evidence"):
        raise ValueError("Provider pricing evidence is required")
    spend = Spend(cap=plan["provider_cost_cap_nanodollars"],
                  input_rate=rates["input_nanodollars_per_token"],
                  output_rate=rates["output_nanodollars_per_token"],
                  max_input=CONTEXT_WINDOW, max_output=MAX_OUTPUT_TOKENS)
    limits = plan["local_resource_cap"]
    guard = partial(read_sample, max_watts=limits["gpu_watts"],
                    max_junction_c=limits["junction_c"], max_age=limits["sample_max_age_seconds"])
    guard()
    registry = Path.home() / ".local/state/cw105-claims"
    connect = authorize_transport(document, plan, registry, checkout=checkout,
                                    merged_revision=merged_revision)
    output.mkdir(mode=0o700)
    (output / "plan.json").write_text(json.dumps(plan, indent=2) + "\n")
    budget = Budget(campaign_limit=plan["max_requests"], seconds=plan["max_wall_seconds"])
    policy = partial(check_request, model=MODEL, max_output_tokens=MAX_OUTPUT_TOKENS)
    metadata_by_id = {row["run_id"]: row for row in schedule()}

    def execute(row):
        run_id = row["run_id"]
        return run_attempt(
            metadata_by_id[run_id], repository, output / run_id, omp, cw, None, budget,
            prompt=row["prompt"], seconds=row["budget_seconds"], spend=spend, guard=guard,
            request_policy=policy, connect=connect,
            verify=lambda worktree, evidence: verify_task(worktree, evidence, row["task"]),
        )

    results = execute_plan(plan["runs"], budget, execute)
    (output / "report.json").write_text(json.dumps(summarize(results), indent=2) + "\n")
    return len(results) == len(plan["runs"]) and all(r["correct"] is True for r in results)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    for option in ("plan", "authorization", "checkout", "repository", "output", "omp", "cw"):
        parser.add_argument(f"--{option}", type=Path, required=True)
    parser.add_argument("--merged-revision", required=True)
    args = parser.parse_args()
    raise SystemExit(not run_live(
        json.loads(args.plan.read_text()), json.loads(args.authorization.read_text()),
        merged_revision=args.merged_revision,
        **{name: getattr(args, name).resolve() for name in (
            "checkout", "repository", "output", "omp", "cw",
        )},
    ))
