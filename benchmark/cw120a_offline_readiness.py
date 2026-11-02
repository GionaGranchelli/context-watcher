#!/usr/bin/env python3
"""Zero-provider-call CW-120A schedule and reporting rehearsal."""

import argparse
import json
import random
from pathlib import Path

import cw118_product_value as cw118


SEED = 1202026
TASKS = ("F1", "F3")
MODES = ("M1", "M2")
REPETITIONS = 3
PIN_DIRECTIVE = (
    "Keep the requested change focused. Inspect relevant code and tests before editing. "
    "Run the relevant tests after editing and report any failures honestly."
)
TASK_INSTRUCTIONS = {
    "F1": "Fix divide_or_fallback so division by zero returns the supplied fallback. "
    "Make safe_mean return 0.0 for empty input. Preserve other behavior and run the tests.",
    "F3": "Implement CachedStore so get caches non-None backing-store values and put updates "
    "both cache and backing store. Preserve the tested behavior and run the tests.",
}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def validate_schedule(schedule):
    require(len(schedule) == 12, "schedule must contain 12 sessions")
    require(
        sum(row["execution_order_position"] == 0 and row["mode"] == "M1" for row in schedule) == 3,
        "schedule must contain three M1-first blocks",
    )
    require(
        sum(row["execution_order_position"] == 0 and row["mode"] == "M2" for row in schedule) == 3,
        "schedule must contain three M2-first blocks",
    )
    require(len({row["block_id"] for row in schedule}) == 6, "schedule must contain six matched blocks")
    for task in TASKS:
        for mode in MODES:
            require(
                sum(row["task_family"] == task and row["mode"] == mode for row in schedule) == 3,
                f"{task} must have three {mode} sessions",
            )
    for block_id in {row["block_id"] for row in schedule}:
        block = [row for row in schedule if row["block_id"] == block_id]
        require(
            [row["mode"] for row in block] in (["M1", "M2"], ["M2", "M1"]),
            f"{block_id} must contain one session per mode",
        )
        require(
            [row["execution_order_position"] for row in block] == [0, 1],
            f"{block_id} must have ordered positions 0 and 1",
        )


def build_schedule():
    rng = random.Random(SEED)
    blocks = [(task, rep) for task in TASKS for rep in range(REPETITIONS)]
    rng.shuffle(blocks)
    first_modes = ["M1", "M2"] * (len(blocks) // 2)
    rng.shuffle(first_modes)
    schedule = []
    for index, ((task, repetition), first_mode) in enumerate(zip(blocks, first_modes)):
        block_id = f"{task}-R{repetition + 1}"
        block_modes = (first_mode, "M2" if first_mode == "M1" else "M1")
        for position, mode in enumerate(block_modes):
            schedule.append({
                "campaign_id": "cw120a_product_value_readiness",
                "block_id": block_id,
                "session_id": f"cw120a_{task.lower()}_r{repetition + 1}_{mode.lower()}",
                "task_family": task,
                "repetition": repetition,
                "mode": mode,
                "execution_order_position": position,
                "global_schedule_index": index * 2 + position,
                "seed": SEED,
            })
    validate_schedule(schedule)
    return schedule


def run(output: Path):
    schedule = build_schedule()
    sessions = []
    for item in schedule:
        session = cw118.simulate_offline_session(item, model="gpt-6-luna")
        require(session["synthetic_telemetry"] is True, "rehearsal session must use synthetic telemetry")
        require(
            session["evidence_provenance"] == "SyntheticFixtureReplay",
            "rehearsal session has unexpected evidence provenance",
        )
        require(
            session["execution_outcome"]["acceptance_test_passed"] is True,
            f"synthetic acceptance failed for {item['session_id']}",
        )
        if item["mode"] == "M1":
            require(
                session["optimization_evidence"]["mutation_count"] == 0,
                f"M1 session {item['session_id']} must have zero mutations",
            )
        else:
            require(
                session["optimization_evidence"]["mutation_count"] == 1,
                f"M2 session {item['session_id']} must have exactly one mutation",
            )
        session["execution_outcome"]["task_success"] = None
        session["execution_outcome"]["acceptance_scope"] = (
            "predefined_solution_fixture_tests_only; no agent-generated task solution"
        )
        simulated_evidence = session.pop("optimization_evidence")
        simulated_evidence["evidence_status"] = "SIMULATION_ONLY"
        simulated_evidence["applied_to_request"] = False
        session["simulated_optimization_evidence"] = simulated_evidence
        session["optimization_evidence"] = {
            "evidence_status": "NOT_EVALUATED",
            "mutation_count": None,
            "estimated_kv_break_tokens": None,
            "behavioral_steering_injected": None,
            "egress_verified": False,
        }
        sessions.append(session)

    indexed = {(s["task_family"], s["repetition"], s["mode"]): s for s in sessions}
    for task in TASKS:
        for repetition in range(REPETITIONS):
            m1 = indexed[(task, repetition, "M1")]
            m2 = indexed[(task, repetition, "M2")]
            require(
                m1["fixture_hash"] == m2["fixture_hash"],
                f"{task}-R{repetition + 1} modes must start from identical fixtures",
            )

    analysis = cw118.analyze_campaign_results(sessions)
    require(len(analysis["paired_comparisons"]) == 6, "analysis must contain six M1/M2 pairs")
    analysis["accounting"]["success_attempt_total_cost_usd"] = None
    require(
        analysis["matched_pairs_m1_vs_m0"] == 0
        and analysis["economic_summary"]["mean_proxy_overhead_ms"] is None,
        "proxy overhead must remain unevaluated without M0 sessions",
    )
    report = {
        "campaign": "CW-120A offline readiness rehearsal",
        "source_git_commit": cw118.get_git_commit(),
        "provider_calls": 0,
        "codex_sessions": 0,
        "proxy_mutations": 0,
        "actual_provider_spend_usd": 0.0,
        "synthetic_costs_are_not_product_evidence": True,
        "evidence_scope": "SYNTHETIC_REHEARSAL_ONLY",
        "independent_mock_test_references": [
            {
                "test": "test_cw118_real_mock_provider_proxy_execution_m0_m1_m2",
                "scope": "Separate deterministic HTTP proxy test; not included in this simulated campaign.",
            },
            {
                "test": "test_cw118_cache_safe_policy_qualification_defer_and_pin",
                "scope": "Separate deterministic policy and PinContext egress test; not included in this simulated campaign.",
            },
        ],
        "configuration": {
            "families": list(TASKS),
            "repetitions": REPETITIONS,
            "modes": list(MODES),
            "task_instructions": TASK_INSTRUCTIONS,
            "pin_directive": PIN_DIRECTIVE,
            "codex_version": "must be recorded and held constant at future execution",
            "model": "gpt-6-luna",
        },
        "model_pricing_profile": "gpt-6-luna standard short-context rates; rehearsal only",
        "seed": SEED,
        "schedule": schedule,
        "analysis": analysis,
        "sessions": sessions,
    }
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(f"Offline readiness passed: 12 synthetic sessions, 6 matched pairs; provider calls=0; report={output}")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, default=Path("target/cw120a-offline-readiness.json"))
    args = parser.parse_args()
    run(args.output)


if __name__ == "__main__":
    main()
