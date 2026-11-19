"""Run one prepared offline attempt and retain evidence outside its worktree."""
import json
import os
import subprocess
import time
from pathlib import Path
from dataclasses import asdict

from evidence import request_metrics
from process import run
from tool_metrics import tool_metrics
from workspace import checkout, cleanup, diff, snapshot


def attempt(entry, repository, root, launch, verify, *, seconds, records=None, guard=None, context_window=None, remaining=None):
    """Use a context manager supplied by the offline transport for argv and env."""
    started = time.monotonic()
    root.mkdir(mode=0o700)
    worktree = root / "workspace"
    revision = entry["revision"]
    result = dict(entry, status="incomplete", correct=None)
    checked_out = False
    baseline = None
    patch = root / "final.patch"
    try:
        checkout(repository, worktree, revision, remaining=remaining)
        checked_out = True
        if entry.get("task") == "web-api-base-url":
            seed = Path(__file__).with_name("fixtures") / "debug-seed.patch"
            subprocess.run(["git", "-C", str(worktree), "apply", "--index", str(seed)],
                           check=True, timeout=min(30, remaining()) if remaining else 30)
        baseline = snapshot(worktree, remaining=remaining)
        if remaining is not None:
            remaining()
        with launch(worktree, root) as (argv, env):
            agent = run(argv, cwd=worktree, env=env, log=root / "agent.log", timeout=seconds, guard=guard)
        result.update(agent=asdict(agent), agent_seconds=agent.elapsed_seconds)
        if agent.timed_out or agent.cancelled_reason:
            result["status"] = "cancelled"
        elif agent.returncode:
            result["status"] = "failed"
        else:
            patch.write_bytes(diff(worktree, baseline or revision, remaining=remaining))
            result.update(correct=verify(worktree, root), status="complete")
    except (OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
        result.update(status="incomplete", error=str(error))
    finally:
        if checked_out:
            try:
                if not patch.exists():
                    patch.write_bytes(diff(worktree, baseline or revision, remaining=remaining))
                cleanup(repository, worktree, remaining=remaining)
            except (OSError, RuntimeError, subprocess.SubprocessError) as error:
                result.update(status="incomplete", cleanup_error=str(error))
        if records is not None and (root / "agent.log").exists():
            result.update(tool_metrics(root / "agent.log"))
        if records is not None:
            with (root / "requests.jsonl").open("x", encoding="utf-8") as output:
                for record in records:
                    output.write(json.dumps(record) + "\n")
            result.update(request_metrics(records, context_window=context_window))
            if result["status"] == "complete" and not result["telemetry_complete"]:
                result["status"] = "incomplete"
        result["wall_seconds"] = time.monotonic() - started
        temporary = root / "manifest.tmp"
        with temporary.open("x", encoding="utf-8") as output:
            json.dump(result, output, indent=2)
            output.flush()
            os.fsync(output.fileno())
        temporary.replace(root / "manifest.json")
    return result
