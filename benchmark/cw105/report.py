"""Compare complete, correct matched runs and account for all planned runs."""
import argparse
import json
import math
from pathlib import Path
from statistics import median

from schedule import MODES, schedule

METRICS = (
    "wall_seconds", "input_tokens", "output_tokens", "cached_input_tokens",
    "ttft_seconds", "provider_cost_usd", "tool_seconds", "mcp_seconds",
    "peak_observed_context_fraction", "tool_count", "mcp_count",
)


def measured(result, metric):
    value = result.get(metric)
    if value is None:
        return None
    if type(value) not in (int, float) or not math.isfinite(value) or value < 0:
        raise ValueError(f"Invalid metric {metric}")
    return value


def summarize(results, plan=None):
    plan = list(schedule() if plan is None else plan)
    by_id = {entry["run_id"]: entry for entry in plan}
    recorded = {}
    for result in results:
        key = result["run_id"]
        if key in recorded or key not in by_id:
            raise ValueError(f"Duplicate or unknown run ID {key}")
        if any(result.get(field) != value for field, value in by_id[key].items()):
            raise ValueError(f"Run metadata does not match schedule for {key}")
        if result.get("status") not in ("complete", "failed", "cancelled", "incomplete"):
            raise ValueError(f"Invalid run status for {key}")
        recorded[key] = result

    groups = {}
    pairs = {}
    for entry in plan:
        result = recorded.get(entry["run_id"], {})
        groups.setdefault(entry["mode"], []).append(result)
        pairs.setdefault((entry["task"], entry["repetition"]), {})[entry["mode"]] = result

    def accepted(result):
        return result.get("status") == "complete" and result.get("correct") is True

    def stats(values):
        return {"count": len(values), "median": median(values) if values else None}

    arms = {}
    for mode in MODES:
        group = groups.get(mode, [])
        good = [result for result in group if accepted(result)]
        arms[mode] = {
            "planned": len(group), "missing": sum(not result for result in group),
            "complete": sum(result.get("status") == "complete" for result in group),
            "correct": len(good),
            "metrics": {
                metric: stats([v for r in good if (v := measured(r, metric)) is not None])
                for metric in METRICS
            },
        }

    deltas = {metric: [] for metric in METRICS}
    matched = 0
    for group in pairs.values():
        before, after = group.get("M1", {}), group.get("M2", {})
        if accepted(before) and accepted(after):
            matched += 1
            for metric in METRICS:
                left, right = measured(before, metric), measured(after, metric)
                if left is not None and right is not None:
                    deltas[metric].append(right - left)
    long_runs = [recorded.get(row["run_id"], {}) for row in plan
                 if row["task"] == "board-query"]
    pressure_observed = sum(
        1 for result in long_runs
        if (value := measured(result, "peak_observed_context_fraction")) is not None
        and value >= 0.8
    )
    return {"long_task_context_pressure": {
                "threshold_fraction": 0.8,
                "runs_at_or_above_threshold": pressure_observed,
                "planned_runs": len(long_runs),
            },
            "arms": arms, "matched_correct_pairs": matched,
            "m2_minus_m1": {name: stats(values) for name, values in deltas.items()}}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifests", type=Path, nargs="+")
    args = parser.parse_args()
    print(json.dumps(summarize(json.loads(p.read_text()) for p in args.manifests), indent=2))
