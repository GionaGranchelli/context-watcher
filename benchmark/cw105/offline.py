"""Rehearse one matched block using real OMP/CW and a scripted provider."""
import argparse
import json
import socket
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

from agent import enable_memory, offline_agent
from attempt import attempt
from budget import Budget
from capture import relay
from daemon import daemon
from report import summarize
from routing import route
from schedule import schedule
from test_capture import serving


class Mock(BaseHTTPRequestHandler):
    def do_POST(self):
        self.rfile.read(int(self.headers["Content-Length"]))
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        self.wfile.write(
            b'data: {"choices":[{"index":0,"delta":{"content":"cw105-rehearsed"},"finish_reason":null}]}\n\n'
            b'data: {"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}\n\n'
            b'data: {"choices":[],"usage":{"prompt_tokens":12,"completion_tokens":3}}\n\n'
            b'data: [DONE]\n\n'
        )

    def log_message(self, *_args):
        pass


def rehearse(repository, output, omp, cw):
    output.mkdir(mode=0o700)
    plan = list(schedule())[:3]
    budget = Budget(campaign_limit=3, seconds=120)
    results = []
    with serving(HTTPServer(("127.0.0.1", 0), Mock)) as upstream:
        for entry in plan:
            run_id, mode = entry["run_id"], entry["mode"]
            records = []
            budget.start(run_id, request_limit=1, seconds=30)

            @contextmanager
            def launch(worktree, root):
                with serving(relay(upstream, budget, run_id, records.append)) as capture:
                    with socket.socket() as api, socket.socket() as proxy:
                        api.bind(("127.0.0.1", 0))
                        proxy.bind(("127.0.0.1", 0))
                        api_port, proxy_port = api.getsockname()[1], proxy.getsockname()[1]
                    selected = route(mode, capture, proxy_port, worktree)
                    state = root / "omp"
                    argv, env = offline_agent(
                        omp, state, capture, "Reply with cw105-rehearsed.",
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

            # Scripted replies can qualify transport only. They earn no task credit.
            results.append(attempt(entry, repository, output / run_id, launch,
                                   lambda *_: None, seconds=20, records=records))
    report = summarize(results, plan)
    (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    return all(result["status"] == "complete" for result in results)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    for option in ("repository", "output", "omp", "cw"):
        parser.add_argument(f"--{option}", type=Path, required=True)
    args = parser.parse_args()
    raise SystemExit(not rehearse(*(getattr(args, name).resolve() for name in (
        "repository", "output", "omp", "cw",
    ))))
