"""Rehearse one matched block using real OMP/CW and a scripted provider."""
import argparse
from functools import partial
import json
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

from budget import Budget
from report import summarize
from plan import proposal
from request_policy import check_request
from spend import Spend
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
    spend = Spend(cap=0, input_rate=0, output_rate=0, max_input=262144, max_output=4096)
    policy = partial(check_request, model="qwen3.8-27b-ud", max_output_tokens=4096)
    execution = [dict(entry, request_cap=1, budget_seconds=30) for entry in plan]
    with serving(HTTPServer(("127.0.0.1", 0), Mock)) as upstream:
        def execute(entry):
            metadata = {key: entry[key] for key in plan[0]}
            return run_attempt(
                metadata, repository, output / entry["run_id"], omp, cw, upstream, budget,
                prompt="Reply with cw105-rehearsed.", verify=lambda *_: None, seconds=20,
                spend=spend, request_policy=policy,
            )

        result = execute_plan(execution, budget, execute)
    plan_document = proposal()
    plan_document["runs"] = plan
    plan_document["campaign_type"] = "offline"
    (output / "plan.json").write_text(json.dumps(plan_document, indent=2) + "\n")
    report = summarize(result.results, plan_document, execution=result)
    (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    return len(result.results) == len(plan) and all(
        r["status"] == "complete" for r in result.results
    )


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    for option in ("repository", "output", "omp", "cw"):
        parser.add_argument(f"--{option}", type=Path, required=True)
    args = parser.parse_args()
    raise SystemExit(not rehearse(*(getattr(args, name).resolve() for name in (
        "repository", "output", "omp", "cw",
    ))))
