"""Forward OMP output unchanged and timestamp tool events as they arrive."""
import json
import os
from pathlib import Path
import subprocess
import sys
import time


def observe(command, timing_path):
    fd = os.open(timing_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w", encoding="utf-8") as timing:
        child = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                 stderr=subprocess.STDOUT)
        fragmented = False
        while chunk := child.stdout.readline(1024 * 1024):
            observed = time.monotonic_ns()
            sys.stdout.buffer.write(chunk)
            sys.stdout.buffer.flush()
            complete_line = chunk.endswith(b"\n")
            if not fragmented and complete_line:
                try:
                    event = json.loads(chunk)
                except (ValueError, UnicodeError):
                    event = None
                if isinstance(event, dict) and event.get("type") in (
                    "tool_execution_start", "tool_execution_end"
                ):
                    timing.write(json.dumps({"type": event["type"],
                                             "toolCallId": event.get("toolCallId"),
                                             "observed_ns": observed}) + "\n")
                    timing.flush()
            fragmented = not complete_line
        child.stdout.close()
        code = child.wait()
        timing.flush()
        os.fsync(timing.fileno())
    return code


if __name__ == "__main__":
    if len(sys.argv) < 3:
        raise SystemExit("Usage: observe_agent.py TIMING_PATH COMMAND [ARG ...]")
    raise SystemExit(observe(sys.argv[2:], Path(sys.argv[1])))
