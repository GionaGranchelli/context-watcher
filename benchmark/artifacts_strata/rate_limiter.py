"""Thread-safe rate limiters for concurrent producers.

Two algorithms, one registry:

* ``TokenBucketRateLimiter`` — burst capacity, steady refill. Good for
  throughput shaping where short bursts are acceptable.
* ``SlidingWindowCounter`` — strict request count over a rolling window.
  Good for hard per-window quotas (per-tenant API limits).
* ``RateLimiterRegistry`` — named limiters, created on first use, shared
  across threads.

Invariants
----------
1. ``acquire`` is atomic: either the full ``tokens`` amount is consumed and
   ``True`` is returned, or nothing is consumed and ``False`` is returned.
   Partial admission never happens.
2. No over-admission. Admitted work is always bounded by capacity/refill
   (token bucket) or by ``max_requests`` inside the window (sliding window).
3. Locks are held only for O(1) bookkeeping. When a caller must wait for a
   quota to become available, it sleeps *outside* the lock, so waiting
   callers never block unrelated callers.
4. All timing uses ``time.monotonic`` (injectable clock for tests), so wall
   clock adjustments and NTP jumps cannot distort quotas.

Waiting is best-effort: a thread that sleeps and retries can lose a quota to
a thread that arrived later. That is the trade-off for not holding the lock
across sleeps; strict FIFO ordering would require a per-limiter condition
variable and is deliberately not added here.
"""

from __future__ import annotations

import math
import threading
import time
from collections import deque
from collections.abc import Callable, Iterator
from typing import Protocol

__all__ = [
    "RateLimiter",
    "RateLimiterRegistry",
]

Stats = dict[str, int | float]
Clock = Callable[[], float]


class RateLimiter(Protocol):
    """Common interface implemented by both limiters."""

    def acquire(self, tokens: int = 1, timeout: float | None = None) -> bool: ...

    def stats(self) -> dict[str, int | float]: ...


def _validate_amount(value: float, name: str) -> float:
    number = float(value)
    if number <= 0.0 or not math.isfinite(number):
        raise ValueError(f"{name} must be a positive finite number, got {value!r}")
    return number


def _validate_tokens(tokens: int) -> int:
    if int(tokens) <= 0:
        raise ValueError(f"tokens must be a positive integer, got {tokens!r}")
    return int(tokens)


def _validate_timeout(timeout: float | None) -> float | None:
    if timeout is None:
        return None
    value = float(timeout)
    if value < 0.0 or not math.isfinite(value):
        raise ValueError(f"timeout must be a non-negative finite number, got {timeout!r}")
    return value


class TokenBucketRateLimiter:
    """Classic token bucket: ``capacity`` tokens, refilled at ``refill_rate_per_sec``.

    Refill is lazy — tokens are materialised on read, not by a background
    timer, so an idle limiter costs nothing.
    """

    def __init__(
        self,
        capacity: float,
        refill_rate_per_sec: float,
        clock: Clock = time.monotonic,
    ) -> None:
        self._capacity = _validate_amount(capacity, "capacity")
        self._refill_rate = _validate_amount(refill_rate_per_sec, "refill_rate_per_sec")
        self._clock = clock
        self._lock = threading.Lock()
        self._tokens = self._capacity
        self._last_refill = clock()
        self._requests = 0
        self._allowed = 0
        self._denied = 0
        self._tokens_granted = 0.0

    @property
    def capacity(self) -> float:
        return self._capacity

    @property
    def refill_rate_per_sec(self) -> float:
        return self._refill_rate

    def available(self) -> float:
        """Tokens available right now (does not count as a request in stats)."""
        with self._lock:
            self._refill_locked()
            return self._tokens

    def acquire(self, tokens: int = 1, timeout: float | None = None) -> bool:
        """Atomically consume ``tokens``.

        ``timeout is None`` means a single attempt: succeed now or fail now.
        Otherwise block up to ``timeout`` seconds, re-checking after each
        sleep until the bucket holds ``tokens`` or the deadline expires.
        """
        need = _validate_tokens(tokens)
        deadline = None if timeout is None else self._clock() + _validate_timeout(timeout)

        while True:
            with self._lock:
                self._requests += 1
                self._refill_locked()
                if self._tokens >= need:
                    self._tokens -= need
                    self._tokens_granted += need
                    self._allowed += 1
                    return True
                if deadline is None:
                    self._denied += 1
                    return False
                deficit = need - self._tokens
                remaining = deadline - self._clock()
                if remaining <= 0.0:
                    self._denied += 1
                    return False
                wait = min(deficit / self._refill_rate, remaining)
            # Sleep unlocked: other callers keep making progress while we wait.
            time.sleep(wait)

    def reset(self) -> None:
        """Refill to full capacity and zero the counters."""
        with self._lock:
            self._tokens = self._capacity
            self._last_refill = self._clock()
            self._requests = 0
            self._allowed = 0
            self._denied = 0
            self._tokens_granted = 0.0

    def stats(self) -> dict[str, int | float]:
        with self._lock:
            self._refill_locked()
            return {
                "type": "token_bucket",
                "capacity": self._capacity,
                "refill_rate_per_sec": self._refill_rate,
                "available_tokens": self._tokens,
                "requests": self._requests,
                "allowed": self._allowed,
                "denied": self._denied,
                "tokens_granted": self._tokens_granted,
            }

    def _refill_locked(self) -> None:
        now = self._clock()
        elapsed = now - self._last_refill
        if elapsed > 0.0:
            self._tokens = min(self._capacity, self._tokens + elapsed * self._refill_rate)
            self._last_refill = now


class SlidingWindowCounter:
    """Rolling window of ``max_requests`` admissions over ``window_seconds``.

    Admission timestamps are kept in a deque and pruned lazily on read, so the
    window slides continuously instead of in fixed buckets.
    """

    def __init__(
        self,
        window_seconds: float,
        max_requests: int,
        clock: Clock = time.monotonic,
    ) -> None:
        self._window = _validate_amount(window_seconds, "window_seconds")
        self._max_requests = _validate_amount(max_requests, "max_requests")
        if int(max_requests) != self._max_requests:
            raise ValueError(f"max_requests must be an integer, got {max_requests!r}")
        self._max_requests = int(self._max_requests)
        self._clock = clock
        self._lock = threading.Lock()
        self._events: deque[float] = deque()
        self._requests = 0
        self._allowed = 0
        self._denied = 0

    @property
    def window_seconds(self) -> float:
        return self._window

    @property
    def max_requests(self) -> int:
        return self._max_requests

    def current_count(self) -> int:
        """Admissions still inside the window."""
        with self._lock:
            self._prune_locked()
            return len(self._events)

    def acquire(self, tokens: int = 1, timeout: float | None = None) -> bool:
        """Atomically admit ``tokens`` requests.

        Same contract as the token bucket: all-or-nothing, single attempt when
        ``timeout is None``, otherwise wait up to ``timeout`` for enough old
        admissions to fall out of the window.
        """
        need = _validate_tokens(tokens)
        deadline = None if timeout is None else self._clock() + _validate_timeout(timeout)

        while True:
            with self._lock:
                self._requests += 1
                self._prune_locked()
                free = self._max_requests - len(self._events)
                if free >= need:
                    now = self._clock()
                    for _ in range(need):
                        self._events.append(now)
                    self._allowed += 1
                    return True
                if deadline is None:
                    self._denied += 1
                    return False
                remaining = deadline - self._clock()
                if remaining <= 0.0:
                    self._denied += 1
                    return False
                # The (need - free)-th oldest admission must expire for `need`
                # slots to open up; the deque is ordered by timestamp.
                must_expire = self._events[need - free - 1]
                wait = min(must_expire + self._window - self._clock(), remaining)
            time.sleep(wait)

    def reset(self) -> None:
        """Drop the window contents and zero the counters."""
        with self._lock:
            self._events.clear()
            self._requests = 0
            self._allowed = 0
            self._denied = 0

    def stats(self) -> dict[str, int | float]:
        with self._lock:
            self._prune_locked()
            return {
                "type": "sliding_window",
                "window_seconds": self._window,
                "max_requests": self._max_requests,
                "current_count": len(self._events),
                "remaining": self._max_requests - len(self._events),
                "requests": self._requests,
                "allowed": self._allowed,
                "denied": self._denied,
            }

    def _prune_locked(self) -> None:
        cutoff = self._clock() - self._window
        events = self._events
        while events and events[0] <= cutoff:
            events.popleft()


class RateLimiterRegistry:
    """Named limiters shared across threads.

    ``default_factory`` builds a limiter on first use; it runs *outside* the
    registry lock, so a factory that re-enters the registry cannot deadlock.
    """

    def __init__(
        self,
        default_factory: Callable[[str], RateLimiter] | None = None,
    ) -> None:
        self._default_factory = default_factory
        self._lock = threading.Lock()
        self._limiters: dict[str, RateLimiter] = {}

    def register(self, name: str, limiter: RateLimiter) -> RateLimiter:
        """Insert or replace a limiter; returns the stored instance."""
        if not name:
            raise ValueError("name must be a non-empty string")
        with self._lock:
            self._limiters[name] = limiter
        return limiter

    def get(self, name: str) -> RateLimiter | None:
        with self._lock:
            return self._limiters.get(name)

    def get_or_create(self, name: str, factory: Callable[[str], RateLimiter] | None = None) -> RateLimiter:
        """Return the limiter for ``name``, creating it if absent.

        Concurrent callers may each build a limiter; the first one to store it
        wins and every caller receives that same instance, so no caller ends
        up with a private limiter.
        """
        if not name:
            raise ValueError("name must be a non-empty string")
        with self._lock:
            existing = self._limiters.get(name)
        if existing is not None:
            return existing
        builder = factory or self._default_factory
        if builder is None:
            raise KeyError(f"no limiter registered for {name!r} and no factory available")
        created = builder(name)
        with self._lock:
            winner = self._limiters.get(name)
            if winner is None:
                self._limiters[name] = created
                return created
            return winner

    def acquire(
        self,
        name: str,
        tokens: int = 1,
        timeout: float | None = None,
        factory: Callable[[str], RateLimiter] | None = None,
    ) -> bool:
        limiter = self.get_or_create(name, factory)
        return limiter.acquire(tokens, timeout)

    def remove(self, name: str) -> RateLimiter | None:
        with self._lock:
            return self._limiters.pop(name, None)

    def clear(self) -> None:
        with self._lock:
            self._limiters.clear()

    def names(self) -> list[str]:
        with self._lock:
            return sorted(self._limiters)

    def __contains__(self, name: str) -> bool:
        with self._lock:
            return name in self._limiters

    def __len__(self) -> int:
        with self._lock:
            return len(self._limiters)

    def __iter__(self) -> Iterator[str]:
        return iter(self.names())

    def stats(self) -> dict[str, int | float]:
        """Aggregate snapshot; per-limiter stats are read outside the registry lock."""
        with self._lock:
            snapshot = dict(self._limiters)
        entries: dict[str, dict[str, int | float]] = {}
        total_requests = 0
        total_allowed = 0
        total_denied = 0
        for name, limiter in snapshot.items():
            entry = limiter.stats()
            entries[name] = entry
            total_requests += int(entry.get("requests", 0))
            total_allowed += int(entry.get("allowed", 0))
            total_denied += int(entry.get("denied", 0))
        return {
            "limiters": len(entries),
            "requests": total_requests,
            "allowed": total_allowed,
            "denied": total_denied,
            "entries": entries,
        }
