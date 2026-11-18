"""Rehearse one matched block using real OMP/CW and a scripted provider."""
import argparse
import json
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

from budget import Budget
from report import summarize
from schedule import schedule
from campaign import execute_plan, run_attempt, serving


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
    execution = [dict(entry, request_cap=1, budget_seconds=30) for entry in plan]
    with serving(HTTPServer(("127.0.0.1", 0), Mock)) as upstream:
        def execute(entry):
            metadata = {key: entry[key] for key in plan[0]}
            return run_attempt(
                metadata, repository, output / entry["run_id"], omp, cw, upstream, budget,
                prompt="Reply with cw105-rehearsed.", verify=lambda *_: None, seconds=20,
            )

        results = execute_plan(execution, budget, execute)
    report = summarize(results, plan)
    (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    return len(results) == len(plan) and all(r["status"] == "complete" for r in results)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    for option in ("repository", "output", "omp", "cw"):
        parser.add_argument(f"--{option}", type=Path, required=True)
    args = parser.parse_args()
    raise SystemExit(not rehearse(*(getattr(args, name).resolve() for name in (
        "repository", "output", "omp", "cw",
    ))))
