"""Run one prepared offline attempt and retain evidence outside its worktree."""
import json
import os
import subprocess
from dataclasses import asdict

from evidence import request_metrics
from process import run
from tool_metrics import tool_metrics
from workspace import checkout, cleanup, diff


def attempt(entry, repository, root, launch, verify, *, seconds, records=None, guard=None):
    """Use a context manager supplied by the offline transport for argv and env."""
    root.mkdir(mode=0o700)
    worktree = root / "workspace"
    revision = entry["revision"]
    result = dict(entry, status="incomplete", correct=None)
    checked_out = False
    patch = root / "final.patch"
    try:
        checkout(repository, worktree, revision)
        checked_out = True
        with launch(worktree, root) as (argv, env):
            agent = run(argv, cwd=worktree, env=env, log=root / "agent.log", timeout=seconds, guard=guard)
        result.update(agent=asdict(agent), wall_seconds=agent.elapsed_seconds)
        if agent.timed_out or agent.cancelled_reason:
            result["status"] = "cancelled"
        elif agent.returncode:
            result["status"] = "failed"
        else:
            patch.write_bytes(diff(worktree, revision))
            result.update(correct=verify(worktree, root), status="complete")
    except (OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
        result.update(status="incomplete", error=str(error))
    finally:
        if checked_out:
            try:
                if not patch.exists():
                    patch.write_bytes(diff(worktree, revision))
                cleanup(repository, worktree)
            except (OSError, subprocess.SubprocessError) as error:
                result.update(status="incomplete", cleanup_error=str(error))
        if records is not None and (root / "agent.log").exists():
            result.update(tool_metrics(root / "agent.log"))
        if records is not None:
            with (root / "requests.jsonl").open("x", encoding="utf-8") as output:
                for record in records:
                    output.write(json.dumps(record) + "\n")
            result.update(request_metrics(records))
            if result["status"] == "complete" and not result["telemetry_complete"]:
                result["status"] = "incomplete"
        temporary = root / "manifest.tmp"
        with temporary.open("x", encoding="utf-8") as output:
            json.dump(result, output, indent=2)
            output.flush()
            os.fsync(output.fileno())
        temporary.replace(root / "manifest.json")
    return result
