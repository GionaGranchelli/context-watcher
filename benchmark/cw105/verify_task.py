"""Run the independent acceptance checks for a benchmark task."""
import argparse
from pathlib import Path

from verify_api import verify_api
from verify_web import verify_web
from process import run
import os


def documents_api_override(worktree):
    readme = worktree / "README.md"
    if not readme.is_file():
        return False
    text = readme.read_text(encoding="utf-8")
    return "NUXT_PUBLIC_API_BASE_URL" in text


def project_checks(worktree, evidence):
    commands = (
        ("api-check", ["./services/api/gradlew", "-p", "services/api", "--offline", "--no-daemon", "check"]),
        ("web-typecheck", ["pnpm", "--filter", "web", "lint"]),
        ("web-test", ["pnpm", "--filter", "web", "test"]),
        ("web-build", ["pnpm", "--filter", "web", "build"]),
    )
    passed = True
    for name, command in commands:
        result = run(command, cwd=worktree, env=os.environ, log=evidence / f"{name}.log",
                     timeout=180, own_group=False)
        if result.timed_out or result.cancelled_reason:
            raise RuntimeError(f"{name} timed out")
        passed = passed and result.returncode == 0
    return passed


def verify_task(worktree, evidence, task):
    if task == "health-contract":
        return verify_api(worktree, evidence, task)
    if task == "web-api-base-url":
        return verify_web(worktree, evidence, task) and documents_api_override(worktree)
    if task == "board-query":
        api_passed = verify_api(worktree, evidence, task)
        web_passed = verify_web(worktree, evidence, task)
        checks_passed = project_checks(worktree, evidence)
        return api_passed and web_passed and checks_passed
    raise ValueError(f"Unknown benchmark task: {task}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("task", choices=("health-contract", "web-api-base-url", "board-query"))
    parser.add_argument("worktree", type=Path)
    parser.add_argument("evidence", type=Path)
    args = parser.parse_args()
    try:
        passed = verify_task(args.worktree.resolve(), args.evidence.resolve(), args.task)
    except Exception as error:
        parser.exit(2, f"Verification infrastructure failed: {error}\n")
    raise SystemExit(0 if passed else 1)
