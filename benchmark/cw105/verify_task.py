"""Run the independent acceptance checks for a benchmark task."""
import argparse
from pathlib import Path

from verify_api import verify_api
from verify_web import verify_web


def verify_task(worktree, evidence, task):
    if task == "health-contract":
        return verify_api(worktree, evidence, task)
    if task == "web-api-base-url":
        return verify_web(worktree, evidence, task)
    if task == "board-query":
        api_passed = verify_api(worktree, evidence, task)
        web_passed = verify_web(worktree, evidence, task)
        return api_passed and web_passed
    raise ValueError(f"Unknown benchmark task: {task}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("task", choices=("health-contract", "web-api-base-url", "board-query"))
    parser.add_argument("worktree", type=Path)
    parser.add_argument("evidence", type=Path)
    args = parser.parse_args()
    raise SystemExit(not verify_task(args.worktree.resolve(), args.evidence.resolve(), args.task))
