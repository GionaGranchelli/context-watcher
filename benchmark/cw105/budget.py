"""Request admission for a single benchmark campaign."""
import threading
import time


class BudgetExceeded(RuntimeError):
    pass


class Budget:
    def __init__(self, *, campaign_limit, seconds, clock=time.monotonic):
        if type(campaign_limit) is not int or campaign_limit <= 0:
            raise ValueError("Campaign request cap must be a positive integer")
        if type(seconds) is not int or seconds <= 0:
            raise ValueError("Campaign duration must be a positive integer")
        self._clock = clock
        self._deadline = clock() + seconds
        self._remaining = campaign_limit
        self._runs = {}
        self._finished = set()
        self._stop_reason = None
        self._lock = threading.Lock()

    def start(self, run_id, *, request_limit, seconds):
        if any(type(n) is not int or n <= 0 for n in (request_limit, seconds)):
            raise ValueError("Run limits must be positive integers")
        with self._lock:
            if self._stop_reason is not None or self._clock() >= self._deadline:
                raise BudgetExceeded(self._stop_reason or "Campaign request-admission deadline exceeded")
            if run_id in self._runs:
                raise ValueError("Run ID already admitted")
            deadline = min(self._deadline, self._clock() + seconds)
            self._runs[run_id] = (request_limit, deadline)

    def finish(self, run_id):
        with self._lock:
            if run_id not in self._runs:
                raise ValueError("Run was not admitted")
            self._finished.add(run_id)

    def stop(self, reason):
        with self._lock:
            if self._stop_reason is None:
                self._stop_reason = reason

    def campaign_remaining(self):
        with self._lock:
            if self._stop_reason is not None:
                raise BudgetExceeded(self._stop_reason)
            remaining = self._deadline - self._clock()
            if remaining <= 0:
                raise BudgetExceeded("Campaign request-admission deadline exceeded")
            return remaining

    def check_running(self, run_id):
        """Stop a running agent on a latched fault or expired deadline."""
        with self._lock:
            if self._stop_reason is not None:
                raise BudgetExceeded(self._stop_reason)
            run = self._runs.get(run_id)
            if run is None or run_id in self._finished or self._clock() >= run[1]:
                raise BudgetExceeded("Run deadline exceeded or run not admitted")

    def admit(self, run_id):
        """Debit before I/O. Failed or cancelled attempts are never refunded."""
        with self._lock:
            if self._stop_reason is not None:
                raise BudgetExceeded(self._stop_reason)
            remaining, deadline = self._runs.get(run_id, (0, self._deadline))
            seconds_left = deadline - self._clock()
            if run_id in self._finished or seconds_left <= 0 or self._remaining <= 0 or remaining <= 0:
                raise BudgetExceeded("Request admission denied")
            self._runs[run_id] = (remaining - 1, deadline)
            self._remaining -= 1
            return seconds_left
