"""Run the scored benchmark only with a reviewed, authorized execution plan."""
import argparse
import json
import hashlib
import os
from dataclasses import asdict
from datetime import datetime, timezone
from threading import Lock
from functools import partial
import sys

from process import run
from pathlib import Path

from agent import CONTEXT_WINDOW, MAX_OUTPUT_TOKENS
from budget import Budget
from campaign import execute_plan, run_attempt
from live_transport import authorize_transport
from machinist import check_loaded_model, read_sample
from plan import MODEL, proposal
from report import summarize
from request_policy import check_request
from schedule import REVISION, schedule
from spend import Spend


def binary_identities(omp, cw):
    identities = {}
    for name, path in (("omp", omp), ("cw", cw)):
        with path.open("rb") as binary:
            identities[name] = hashlib.file_digest(binary, "sha256").hexdigest()
    return identities


def score(worktree, evidence, task, budget, resource_guard):
    verifier = Path(__file__).with_name("verify_task.py").resolve()

    def guard():
        budget.campaign_remaining()
        resource_guard()
        budget.campaign_remaining()

    result = run([sys.executable, str(verifier), task, str(worktree), str(evidence)],
                 cwd=worktree, env=os.environ, log=evidence / "verifier.log",
                 timeout=budget.campaign_remaining(), guard=guard)
    if result.timed_out or result.cancelled_reason:
        reason = result.cancelled_reason or "Campaign deadline exceeded during verification"
        budget.stop(reason)
        raise RuntimeError(f"Verification cancelled: {reason}")
    if result.returncode not in (0, 1):
        raise RuntimeError("Verification infrastructure failed; see verifier.log")
    return result.returncode == 0


def run_live(plan, document, *, checkout, merged_revision, repository, output, omp, cw):
    if plan.get("status") != "ready_for_authorization":
        raise ValueError("Live execution plan has not passed readiness review")
    frozen = proposal()
    for field in ("runs", "debug_seed_sha256", "max_requests", "max_wall_seconds", "concurrency",
                  "smoke_max_requests", "smoke_max_seconds"):
        if plan.get(field) != frozen[field]:
            raise ValueError(f"Execution envelope differs from the frozen {field}")
    if plan.get("binaries_sha256") != binary_identities(omp, cw):
        raise ValueError("Execution binaries do not match the approved hashes")
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
    sample = partial(read_sample, max_watts=limits["gpu_watts"],
                    max_junction_c=limits["junction_c"], max_age=limits["sample_max_age_seconds"])
    loaded_model = check_loaded_model(MODEL, CONTEXT_WINDOW)
    initial_sample = sample()
    registry = Path.home() / ".local/state/cw105-claims"
    connect = authorize_transport(document, plan, registry, checkout=checkout,
                                    merged_revision=merged_revision)
    output.mkdir(mode=0o700)
    (output / "plan.json").write_text(json.dumps(plan, indent=2) + "\n")
    (output / "model-preflight.json").write_text(json.dumps(loaded_model, indent=2) + "\n")
    budget = Budget(campaign_limit=plan["max_requests"], seconds=plan["max_wall_seconds"])
    resource_lock = Lock()

    def record_sample(reading):
        rows, watts = reading
        with (output / "gpu-samples.jsonl").open("a", encoding="utf-8") as log:
            log.write(json.dumps({"utc": datetime.now(timezone.utc).isoformat(),
                                  "watts": watts, "devices": [asdict(row) for row in rows]}) + "\n")
            log.flush()
            os.fsync(log.fileno())

    def guard():
        with resource_lock:
            record_sample(sample())

    record_sample(initial_sample)
    policy = partial(check_request, model=MODEL, max_output_tokens=MAX_OUTPUT_TOKENS)
    metadata_by_id = {row["run_id"]: row for row in schedule()}

    def execute(row):
        run_id = row["run_id"]
        return run_attempt(
            metadata_by_id[run_id], repository, output / run_id, omp, cw, None, budget,
            prompt=row["prompt"], seconds=row["budget_seconds"], spend=spend, guard=guard,
            request_policy=policy, connect=connect,
            verify=lambda worktree, evidence: score(worktree, evidence, row["task"], budget, guard),
        )

    smoke_entry = {"run_id": "provider-smoke", "mode": "M0", "revision": REVISION}
    smoke_seconds = plan["smoke_max_seconds"]
    budget.start(smoke_entry["run_id"], request_limit=plan["smoke_max_requests"],
                 seconds=smoke_seconds)
    smoke = run_attempt(
        smoke_entry, repository, output / "provider-smoke", omp, cw, None, budget,
        prompt="Reply with hi.", verify=lambda *_: None, seconds=smoke_seconds,
        spend=spend, guard=guard, request_policy=policy, connect=connect,
    )
    if smoke["status"] != "complete" or not smoke.get("telemetry_complete"):
        budget.stop("Provider smoke did not capture complete usage")
        results = []
    else:
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
