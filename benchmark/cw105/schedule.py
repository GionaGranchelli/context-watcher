"""Counterbalanced blocks for the frozen CW-105 campaign."""
import itertools
import json

REVISION = "790d5ed1394965cd8c68ffe14bf6a8fdb7504774"
TASKS = ("health-contract", "web-api-base-url", "board-query")
MODES = ("M0", "M1", "M2")


def schedule():
    for repetition, modes in enumerate(itertools.permutations(MODES), 1):
        offset = (repetition - 1) % len(TASKS)
        for task in TASKS[offset:] + TASKS[:offset]:
            for mode in modes:
                yield {
                    "run_id": f"{task}-{repetition}-{mode}",
                    "task": task,
                    "revision": REVISION,
                    "repetition": repetition,
                    "mode": mode,
                }


if __name__ == "__main__":
    print(json.dumps(list(schedule()), indent=2))
