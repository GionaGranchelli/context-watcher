"""Run an agent or oracle with bounded time and durable output."""
import os
import signal
import subprocess
import time
from dataclasses import dataclass

from budget import BudgetExceeded


@dataclass(frozen=True)
class ProcessResult:
    returncode: int
    elapsed_seconds: float
    timed_out: bool
    cancelled_reason: str | None = None


def stop_group(process):
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    process.wait()


def run(command, *, cwd, env, log, timeout, guard=None, own_group=True):
    started = time.monotonic()
    fd = os.open(log, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "wb") as output:
        if guard is not None:
            try:
                guard()
            except BudgetExceeded as error:
                output.flush()
                os.fsync(output.fileno())
                return ProcessResult(-signal.SIGKILL, time.monotonic() - started, False, str(error))
        process = subprocess.Popen(
            command, cwd=cwd, env=env, stdin=subprocess.DEVNULL,
            stdout=output, stderr=subprocess.STDOUT, start_new_session=own_group,
        )
        timed_out = False
        cancelled_reason = None
        try:
            while process.poll() is None:
                if guard is not None:
                    guard()
                remaining = timeout - (time.monotonic() - started)
                if remaining <= 0:
                    timed_out = True
                    break
                try:
                    process.wait(timeout=min(1, remaining) if guard is not None else remaining)
                except subprocess.TimeoutExpired:
                    continue
        except BudgetExceeded as error:
            cancelled_reason = str(error)
        finally:
            if own_group:
                stop_group(process)
            elif process.poll() is None:
                process.kill()
                process.wait()
            output.flush()
            os.fsync(output.fileno())
    return ProcessResult(process.returncode, time.monotonic() - started, timed_out, cancelled_reason)
