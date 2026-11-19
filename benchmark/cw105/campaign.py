"""Shared attempt launch through the loopback capture path."""
import json
import socket
import sys
from dataclasses import dataclass
from pathlib import Path
import threading
from contextlib import contextmanager

from agent import CONTEXT_WINDOW, enable_memory, offline_agent
from attempt import attempt
from budget import BudgetExceeded
from capture import relay
from daemon import daemon
from routing import route
from plan import MCP_CALLS_PER_RUN
from process import ProcessStillRunning


@dataclass(frozen=True)
class PlanExecution:
    results: tuple[dict, ...]
    stop_reason: str | None
    stopped_run_id: str | None
    unstarted_runs: int


@contextmanager
def serving(server, *, on_exit=None, shutdown_timeout=2):
    with server:
        thread = threading.Thread(target=server.serve_forever, args=(0.01,), daemon=True)
        thread.start()
        try:
            yield server.server_port
        finally:
            if on_exit is not None:
                on_exit()
            shutdown_errors = []

            def shutdown():
                try:
                    server.shutdown()
                except Exception as error:
                    shutdown_errors.append(error)

            stopper = threading.Thread(target=shutdown, daemon=True)
            stopper.start()
            stopper.join(shutdown_timeout)
            if stopper.is_alive():
                raise ProcessStillRunning("Relay shutdown did not finish")
            if shutdown_errors:
                raise ProcessStillRunning(f"Relay shutdown failed: {shutdown_errors[0]}")
            thread.join(shutdown_timeout)
            if thread.is_alive():
                raise ProcessStillRunning("Relay serving thread did not exit")


def run_attempt(entry, repository, root, omp, cw, upstream, budget, *,
                prompt, verify, seconds, spend=None, guard=None, request_policy=None, connect=None):
    run_id, mode = entry["run_id"], entry["mode"]
    records = []

    @contextmanager
    def launch(worktree, root):
        wire_dir = root / "wire"
        wire_dir.mkdir(mode=0o700)
        with serving(relay(upstream, budget, run_id, records.append, spend=spend, guard=guard,
                           request_policy=request_policy, connect=connect, wire_dir=wire_dir),
                     on_exit=lambda: budget.finish(run_id)) as capture:
            with socket.socket() as api, socket.socket() as proxy:
                api.bind(("127.0.0.1", 0))
                proxy.bind(("127.0.0.1", 0))
                api_port, proxy_port = api.getsockname()[1], proxy.getsockname()[1]
            selected = route(mode, capture, proxy_port, worktree)
            state = root / "omp"
            argv, env = offline_agent(
                omp, state, capture, prompt,
                mode=mode, proxy_port=proxy_port, workspace=worktree,
            )
            with daemon(cw, selected, workspace=worktree, state=root / "cw",
                        api_port=api_port, proxy_port=proxy_port,
                        remaining=budget.campaign_remaining) as control:
                if control:
                    profile = state / "models.json"
                    config = json.loads(profile.read_text())
                    config["providers"]["cw105"]["headers"] = {
                        "x-cw-session-id": run_id, "x-cw-control-token": control,
                    }
                    profile.write_text(json.dumps(config))
                if selected.virtual_memory:
                    mcp_config = enable_memory(
                        argv, state, binary=cw, daemon_state=root / "cw",
                        api_port=api_port, session_id=run_id, workspace=worktree,
                    )
                else:
                    mcp_config = None
                wrapper = Path(__file__).with_name("network_guard.py").resolve()
                observer = Path(__file__).with_name("observe_agent.py").resolve()
                observer_env = dict(env)
                if mode == "M2":
                    observer_env["CW105_MCP_CALL_LIMIT"] = str(MCP_CALLS_PER_RUN)
                try:
                    yield [sys.executable, str(wrapper),
                           str(capture if mode == "M0" else proxy_port),
                           sys.executable, str(observer), str(root / "tool-timing.jsonl"), *argv], observer_env
                finally:
                    if mcp_config is not None:
                        mcp_config.unlink(missing_ok=True)

    def supervise():
        budget.check_running(run_id)
        if guard is not None:
            try:
                guard()
            except BudgetExceeded as error:
                budget.stop(str(error))
                raise

    return attempt(entry, repository, root, launch, verify,
                   seconds=seconds, records=records, guard=supervise,
                   context_window=CONTEXT_WINDOW, remaining=budget.campaign_remaining)


def execute_plan(plan, budget, execute):
    """Continue after outcome failures; stop only when execution safety is uncertain."""
    plan = list(plan)
    results = []
    for index, entry in enumerate(plan):
        try:
            budget.start(entry["run_id"], request_limit=entry["request_cap"],
                         seconds=entry["budget_seconds"])
        except BudgetExceeded as error:
            return PlanExecution(tuple(results), str(error), entry["run_id"], len(plan) - index)
        try:
            result = execute(entry)
        except Exception as error:
            reason = f"attempt_execution_exception:{type(error).__name__}"
            budget.stop(reason)
            return PlanExecution(tuple(results), reason, entry["run_id"], len(plan) - index - 1)
        results.append(result)
        if result.get("campaign_stop_reason"):
            reason = result["campaign_stop_reason"]
            budget.stop(reason)
            return PlanExecution(tuple(results), reason, entry["run_id"], len(plan) - index - 1)
    return PlanExecution(tuple(results), None, None, 0)
