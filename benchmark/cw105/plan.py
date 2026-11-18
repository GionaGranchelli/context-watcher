"""Build a reviewable CW-105 proposal without contacting the provider."""
import json
from pathlib import Path

from authorization import plan_digest
from schedule import schedule

MODEL = "qwen3.8-27b-ud"
ATTEMPT_OVERHEAD_SECONDS = 600
SMOKE_MAX_REQUESTS = 4
SMOKE_MAX_SECONDS = 600
ENDPOINT = "http://192.168.1.156:1234/v1"


def proposal():
    tasks = json.loads(Path(__file__).with_name("tasks.json").read_text())
    runs = [
        dict(entry, **tasks[entry["task"]])
        for entry in schedule()
    ]
    tasks_requests = sum(run["request_cap"] for run in runs)
    agent_seconds = sum(run["budget_seconds"] for run in runs)
    return {
        "version": 1,
        "provider": "Machinist LM Studio",
        "endpoint": ENDPOINT,
        "model": MODEL,
        "concurrency": 1,
        "max_task_requests": tasks_requests,
        "max_agent_seconds": agent_seconds,
        "max_requests": tasks_requests + SMOKE_MAX_REQUESTS,
        "max_wall_seconds": agent_seconds + len(runs) * ATTEMPT_OVERHEAD_SECONDS + SMOKE_MAX_SECONDS,
        "smoke_max_requests": SMOKE_MAX_REQUESTS,
        "smoke_max_seconds": SMOKE_MAX_SECONDS,
        "attempt_overhead_seconds": ATTEMPT_OVERHEAD_SECONDS,
        "provider_cost_cap_nanodollars": None,
        "local_resource_cap": None,
        "status": "not_ready_for_authorization",
        "runs": runs,
    }


if __name__ == "__main__":
    plan = proposal()
    print(json.dumps({"plan_sha256": plan_digest(plan), "plan": plan}, indent=2))
