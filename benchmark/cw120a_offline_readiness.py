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
    return schedule


def run(output: Path):
    schedule = build_schedule()
    assert len(schedule) == 12
    assert sum(row["execution_order_position"] == 0 and row["mode"] == "M1" for row in schedule) == 3
    assert sum(row["execution_order_position"] == 0 and row["mode"] == "M2" for row in schedule) == 3
    assert len({row["block_id"] for row in schedule}) == 6
    for task in TASKS:
        assert sum(row["task_family"] == task and row["mode"] == "M1" for row in schedule) == 3
        assert sum(row["task_family"] == task and row["mode"] == "M2" for row in schedule) == 3
    for block_id in {row["block_id"] for row in schedule}:
        block = [row for row in schedule if row["block_id"] == block_id]
        assert [row["mode"] for row in block] in (["M1", "M2"], ["M2", "M1"])
        assert [row["execution_order_position"] for row in block] == [0, 1]

    sessions = []
    for item in schedule:
        session = cw118.simulate_offline_session(item, model="gpt-6-luna")
        assert session["synthetic_telemetry"] is True
        assert session["evidence_provenance"] == "SyntheticFixtureReplay"
        assert session["execution_outcome"]["acceptance_test_passed"] is True
        if item["mode"] == "M1":
            assert session["optimization_evidence"]["mutation_count"] == 0
        else:
            assert session["optimization_evidence"]["mutation_count"] == 1
        sessions.append(session)

    indexed = {(s["task_family"], s["repetition"], s["mode"]): s for s in sessions}
    for task in TASKS:
        for repetition in range(REPETITIONS):
            m1 = indexed[(task, repetition, "M1")]
            m2 = indexed[(task, repetition, "M2")]
            assert m1["fixture_hash"] == m2["fixture_hash"]

    analysis = cw118.analyze_campaign_results(sessions)
    report = {
        "campaign": "CW-120A offline readiness rehearsal",
        "source_git_commit": cw118.get_git_commit(),
        "provider_calls": 0,
        "actual_provider_spend_usd": 0.0,
        "synthetic_costs_are_not_product_evidence": True,
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
