"""Build a reviewable CW-105 proposal without contacting the provider."""
import json
from pathlib import Path

from authorization import plan_digest
from schedule import schedule

MODEL = "qwen3.8-27b-ud"
ENDPOINT = "http://192.168.1.156:1234/v1"


def proposal():
    tasks = json.loads(Path(__file__).with_name("tasks.json").read_text())
    runs = [
        dict(entry, **tasks[entry["task"]])
        for entry in schedule()
    ]
    return {
        "version": 1,
        "provider": "Machinist LM Studio",
        "endpoint": ENDPOINT,
        "model": MODEL,
        "concurrency": 1,
        "max_task_requests": sum(run["request_cap"] for run in runs),
        "max_agent_seconds": sum(run["budget_seconds"] for run in runs),
        "provider_cost_cap_usd": None,
        "local_resource_cap": None,
        "status": "not_ready_for_authorization",
        "runs": runs,
    }


if __name__ == "__main__":
    plan = proposal()
    print(json.dumps({"plan_sha256": plan_digest(plan), "plan": plan}, indent=2))
