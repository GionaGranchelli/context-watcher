"""Small task scheduler with bounded retries generated directly by machinist-flint (40B Q8_0) in OMP."""

from __future__ import annotations

import threading
from dataclasses import dataclass
from typing import Any, Callable


@dataclass
class Task:
    name: str
    fn: Callable[[], Any]
    max_retries: int = 0
    state: str = "pending"
    attempts: int = 0
    result: Any = None
    error: BaseException | None = None


class TaskScheduler:
    def __init__(self) -> None:
        self._tasks: dict[str, Task] = {}
        self._lock = threading.RLock()

    def add_task(self, name: str, fn: Callable[[], Any], max_retries: int) -> None:
        task = Task(name=name, fn=fn, max_retries=max_retries)
        with self._lock:
            self._tasks[name] = task

    def run_all(self) -> list[dict[str, Any]]:
        with self._lock:
            tasks = list(self._tasks.values())
        results: list[dict[str, Any]] = []
        for task in tasks:
            results.append(self._run_task(task))
        return results

    def status(self) -> dict[str, str]:
        with self._lock:
            return {name: task.state for name, task in self._tasks.items()}

    def _run_task(self, task: Task) -> dict[str, Any]:
        task.state = "running"
        task.result = None
        task.error = None
        max_attempts = task.max_retries + 1

        for attempt in range(1, max_attempts + 1):
            task.attempts = attempt
            try:
                task.result = task.fn()
                task.state = "succeeded"
                task.error = None
                return {
                    "name": task.name,
                    "status": "succeeded",
                    "result": task.result,
                    "attempts": task.attempts,
                }
            except Exception as exc:
                task.error = exc
                task.state = "failed" if attempt == max_attempts else "retrying"

        task.state = "failed"
        task.result = None
        return {
            "name": task.name,
            "status": "failed",
            "result": None,
            "attempts": task.attempts,
            "error": repr(task.error),
        }
