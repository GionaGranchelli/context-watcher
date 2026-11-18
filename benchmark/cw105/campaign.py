"""Shared attempt launch through the loopback capture path."""
import json
import socket
import threading
from contextlib import contextmanager

from agent import CONTEXT_WINDOW, enable_memory, offline_agent
from attempt import attempt
from budget import BudgetExceeded
from capture import relay
from daemon import daemon
from routing import route


@contextmanager
def serving(server):
    with server:
        thread = threading.Thread(target=server.serve_forever, args=(0.01,))
        thread.start()
        try:
            yield server.server_port
        finally:
            server.shutdown()
            thread.join()


def run_attempt(entry, repository, root, omp, cw, upstream, budget, *,
                prompt, verify, seconds, spend=None, guard=None, request_policy=None, connect=None):
    run_id, mode = entry["run_id"], entry["mode"]
    records = []

    @contextmanager
    def launch(worktree, root):
        with serving(relay(upstream, budget, run_id, records.append, spend=spend, guard=guard,
                           request_policy=request_policy, connect=connect)) as capture:
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
                        api_port=api_port, proxy_port=proxy_port) as control:
                if control:
                    profile = state / "models.json"
                    config = json.loads(profile.read_text())
                    config["providers"]["cw105"]["headers"] = {
                        "x-cw-session-id": run_id, "x-cw-control-token": control,
                    }
                    profile.write_text(json.dumps(config))
                if selected.virtual_memory:
                    enable_memory(argv, state, binary=cw, daemon_state=root / "cw",
                                  api_port=api_port, session_id=run_id)
                yield argv, env

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
                   context_window=CONTEXT_WINDOW)


def execute_plan(plan, budget, execute):
    """Run serially and leave later rows unstarted after a guard breach."""
    results = []
    for entry in plan:
        try:
            budget.start(entry["run_id"], request_limit=entry["request_cap"],
                         seconds=entry["budget_seconds"])
        except BudgetExceeded:
            break
        result = execute(entry)
        results.append(result)
        if result["status"] != "complete" or result.get("correct") is False:
            budget.stop(f"Attempt did not pass: {entry['run_id']}")
            break
    return results
