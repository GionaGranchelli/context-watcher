"""Compare complete, correct matched runs and account for all planned runs."""
import argparse
import json
import math
from pathlib import Path
from statistics import median

from authorization import plan_digest
from schedule import MODES

METRICS = (
    "wall_seconds", "agent_seconds", "input_tokens", "output_tokens", "cached_input_tokens",
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


def summarize(results, plan, *, execution=None, campaign_type=None):
    if not isinstance(plan, dict) or not isinstance(plan.get("runs"), list):
        raise ValueError("Report requires the frozen execution plan")
    decision_rule = plan.get("value_decision_rule")
    if not isinstance(decision_rule, dict):
        raise ValueError("Frozen execution plan has no value decision rule")
    plan_rows = plan["runs"]
    plan_sha256 = plan_digest(plan)
    by_id = {entry["run_id"]: entry for entry in plan_rows}
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
    plan_campaign_type = plan.get("campaign_type", "full")
    if campaign_type is not None and campaign_type != plan_campaign_type:
        raise ValueError("Report campaign type does not match the frozen execution plan")
    campaign_type = plan_campaign_type

    groups = {}
    pairs = {}
    for entry in plan_rows:
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
        completed = sum(result.get("status") == "complete" for result in group)
        arms[mode] = {
            "planned": len(group), "missing": sum(not result for result in group),
            "complete": completed, "correct": len(good),
            "correctness_failures": sum(
                result.get("status") == "complete" and result.get("correct") is False
                for result in group
            ),
            "agent_failures": sum(result.get("status") == "failed" for result in group),
            "cancelled": sum(result.get("status") == "cancelled" for result in group),
            "incomplete": sum(result.get("status") == "incomplete" for result in group),
            "safety_stops": sum(bool(result.get("campaign_stop_reason")) for result in group),
            "completion_rate": completed / len(group) if group else None,
            "correctness_rate": len(good) / len(group) if group else None,
            "metrics": {
                metric: stats([v for r in good if (v := measured(r, metric)) is not None])
                for metric in METRICS
            },
        }

    def compare(reference, candidate, task=None, predicate=None):
        deltas = {metric: [] for metric in METRICS}
        token_reductions = []
        matched = 0
        for (task_id, _), group in pairs.items():
            if task is not None and task_id != task:
                continue
            if predicate is not None and not predicate(task_id, group):
                continue
            before, after = group.get(reference, {}), group.get(candidate, {})
            if accepted(before) and accepted(after):
                matched += 1
                for metric in METRICS:
                    left, right = measured(before, metric), measured(after, metric)
                    if left is not None and right is not None:
                        deltas[metric].append(right - left)
                if candidate == "M2" and reference == "M1":
                    left = measured(before, "input_tokens")
                    right = measured(after, "input_tokens")
                    if left is not None and right is not None and left > 0:
                        token_reductions.append((left - right) / left)
        return {"matched_correct_pairs": matched,
                "input_token_reduction_fraction": stats(token_reductions),
                "metrics": {name: stats(values) for name, values in deltas.items()}}

    comparisons = {
        "m1_minus_m0": compare("M0", "M1"),
        "m2_minus_m0": compare("M0", "M2"),
        "m2_minus_m1": compare("M1", "M2"),
    }
    by_task = {task: {
        "m1_minus_m0": compare("M0", "M1", task),
        "m2_minus_m0": compare("M0", "M2", task),
        "m2_minus_m1": compare("M1", "M2", task),
    } for task in sorted({entry["task"] for entry in plan_rows})}
    long_runs = [recorded.get(row["run_id"], {}) for row in plan_rows
                 if row["task"] == "board-query"]
    threshold = decision_rule["context_pressure_threshold_fraction"]
    pressure_by_mode = {
        mode: sum(
            result.get("mode") == mode
            and (value := measured(result, "peak_observed_context_fraction")) is not None
            and value >= threshold
            for result in long_runs
        ) for mode in MODES
    }
    pressure_pairs = sum(
        accepted(group.get("M1", {})) and accepted(group.get("M2", {}))
        and (value := measured(group["M1"], "peak_observed_context_fraction")) is not None
        and value >= threshold
        for (task_id, _), group in pairs.items() if task_id == "board-query"
    )
    pressure_comparison = compare(
        "M1", "M2",
        predicate=lambda task_id, group: task_id == "board-query"
        and accepted(group.get("M1", {})) and accepted(group.get("M2", {}))
        and (value := measured(group["M1"], "peak_observed_context_fraction")) is not None
        and value >= threshold,
    )
    pressure_reduction = pressure_comparison["input_token_reduction_fraction"]
    pressure_value_demonstrated = (
        pressure_pairs >= decision_rule["minimum_context_pressure_pairs"]
        and pressure_reduction["count"] >= decision_rule["minimum_context_pressure_pairs"]
        and pressure_reduction["median"] is not None
        and pressure_reduction["median"] >= decision_rule["minimum_median_input_token_reduction_fraction"]
    )
    execution_report = {
        "stop_reason": execution.stop_reason,
        "stopped_run_id": execution.stopped_run_id,
        "unstarted_runs": execution.unstarted_runs,
    } if execution is not None else {
        "stop_reason": next((result["campaign_stop_reason"] for result in results
                              if result.get("campaign_stop_reason")), None),
        "stopped_run_id": next((result["run_id"] for result in results
                                 if result.get("campaign_stop_reason")), None),
        "unstarted_runs": max(0, len(plan_rows) - len(results)),
    }
    task_outcomes = {}
    for task_id in sorted({entry["task"] for entry in plan_rows}):
        task_outcomes[task_id] = {}
        for mode in MODES:
            group = [recorded.get(row["run_id"], {}) for row in plan_rows
                     if row["task"] == task_id and row["mode"] == mode]
            task_outcomes[task_id][mode] = {
                "planned": len(group),
                "complete": sum(result.get("status") == "complete" for result in group),
                "correct": sum(accepted(result) for result in group),
                "m2_virtual_memory_runs": sum(
                    result.get("status") == "complete"
                    and (value := measured(result, "mcp_count")) is not None and value > 0
                    for result in group
                ) if mode == "M2" else None,
            }

    recommendation = {"label": "ITERATE", "reason": "Insufficient qualified full-campaign evidence"}
    if campaign_type != "full":
        recommendation["reason"] = "Pilot or offline rehearsal is operational qualification, not product-value evidence"
    elif (execution_report["stop_reason"] is not None or execution_report["unstarted_runs"]
          or len(results) < len(plan)):
        recommendation["reason"] = "Safety stop or incomplete schedule leaves the comparison incomplete"
    else:
        pair_counts = [by_task[task]["m2_minus_m1"]["matched_correct_pairs"]
                       for task in by_task]
        token_counts = [by_task[task]["m2_minus_m1"]["input_token_reduction_fraction"]["count"]
                        for task in by_task]
        if (sum(pair_counts) < decision_rule["minimum_correct_m1_m2_pairs"]
                or any(count < decision_rule["minimum_pairs_per_task"] for count in pair_counts)
                or any(count < decision_rule["minimum_pairs_per_task"] for count in token_counts)):
            recommendation["reason"] = "Too few correct matched M1/M2 pairs"
        elif any(
            task_outcomes[task]["M2"][field] < task_outcomes[task]["M1"][field]
            for task in task_outcomes for field in ("complete", "correct")
        ):
            recommendation["reason"] = "M2 has a task-level completion or correctness regression"
        elif any(
            task_outcomes[task]["M2"]["m2_virtual_memory_runs"]
            < decision_rule["minimum_m2_virtual_memory_runs_per_task"]
            for task in task_outcomes
        ):
            recommendation["reason"] = "M2 did not use virtual-memory tools enough in every task family"
        else:
            reduction = comparisons["m2_minus_m1"]["input_token_reduction_fraction"]
            target = decision_rule["minimum_median_input_token_reduction_fraction"]
            if reduction["count"] < decision_rule["minimum_correct_m1_m2_pairs"]:
                recommendation["reason"] = "Too few matched provider input-token measurements"
            elif reduction["median"] is not None and reduction["median"] >= target:
                recommendation = {
                    "label": "GO",
                    "reason": "M2 met the preregistered input-token target without task-level correctness or completion regression",
                    "scope": "long-context value demonstrated" if pressure_value_demonstrated
                             else "ordinary-length coding only",
                }
            else:
                recommendation = {
                    "label": "NO-GO",
                    "reason": "A complete qualified campaign missed the preregistered input-token target",
                }
    return {"long_task_context_pressure": {
                "threshold_fraction": threshold,
                "runs_at_or_above_threshold_by_mode": pressure_by_mode,
                "planned_runs": len(long_runs),
                "matched_m1_m2_pressure_pairs": pressure_pairs,
                "minimum_pairs_for_pressure_claim": decision_rule["minimum_context_pressure_pairs"],
                "claim_eligible": pressure_pairs >= decision_rule["minimum_context_pressure_pairs"],
                "m2_minus_m1_on_pressure_pairs": pressure_comparison,
                "long_context_value_demonstrated": pressure_value_demonstrated,
                "conclusion_scope": "context_pressure_value_demonstrated" if pressure_value_demonstrated
                                    else "ordinary_length_coding_only",
            },
            "campaign_execution": execution_report,
            "campaign_type": campaign_type,
            "value_decision_rule": decision_rule,
            "authorized_plan_sha256": plan_sha256,
            "recommendation": recommendation,
            "task_outcomes": task_outcomes,
            "timing_interpretation": {
                "relay_observed_response_timing": "Relay-measured TTFT and elapsed time, not provider-server timing",
                "tool_durations": "OMP event arrival intervals; subject to buffering and wrapper overhead",
                "provider_cache_state": "Unknown unless directly observed; latency differences are descriptive",
            },
            "arms": arms, "comparisons": comparisons, "comparisons_by_task": by_task}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--plan", type=Path, required=True)
    parser.add_argument("manifests", type=Path, nargs="+")
    args = parser.parse_args()
    plan = json.loads(args.plan.read_text())
    results = [json.loads(p.read_text()) for p in args.manifests]
    print(json.dumps(summarize(results, plan), indent=2))
