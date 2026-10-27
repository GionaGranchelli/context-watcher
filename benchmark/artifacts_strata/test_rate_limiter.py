"""Comprehensive unit test suite for rate_limiter module.

Tests TokenBucketRateLimiter, SlidingWindowCounter, and RateLimiterRegistry
for correctness, thread safety, and edge-case validation.
"""

from __future__ import annotations

import math
import threading
import time
import unittest
from concurrent.futures import ThreadPoolExecutor

from rate_limiter import (
    RateLimiterRegistry,
    SlidingWindowCounter,
    TokenBucketRateLimiter,
)


class TestTokenBucketRateLimiter(unittest.TestCase):
    def test_instant_acquire_within_capacity(self) -> None:
        tb = TokenBucketRateLimiter(capacity=5.0, refill_rate_per_sec=1.0)
        self.assertTrue(tb.acquire(tokens=3))
        self.assertAlmostEqual(tb.available(), 2.0, places=1)
        self.assertTrue(tb.acquire(tokens=2))
        self.assertAlmostEqual(tb.available(), 0.0, places=1)
        # 0 tokens left: should fail instantly without timeout
        self.assertFalse(tb.acquire(tokens=1))

    def test_refill_over_time(self) -> None:
        tb = TokenBucketRateLimiter(capacity=10.0, refill_rate_per_sec=10.0)
        self.assertTrue(tb.acquire(10))
        self.assertAlmostEqual(tb.available(), 0.0, places=1)
        time.sleep(0.25)
        # Expect ~2.5 tokens refilled
        self.assertGreaterEqual(tb.available(), 2.0)
        self.assertTrue(tb.acquire(2))

    def test_timeout_blocks_until_refill(self) -> None:
        tb = TokenBucketRateLimiter(capacity=2.0, refill_rate_per_sec=5.0)
        self.assertTrue(tb.acquire(2))
        t0 = time.monotonic()
        # Needs 1 token: at 5 tokens/sec, should take ~0.2s
        self.assertTrue(tb.acquire(1, timeout=0.5))
        elapsed = time.monotonic() - t0
        self.assertGreaterEqual(elapsed, 0.15)
        self.assertLess(elapsed, 0.5)

    def test_timeout_expires_when_insufficient_time(self) -> None:
        tb = TokenBucketRateLimiter(capacity=2.0, refill_rate_per_sec=1.0)
        self.assertTrue(tb.acquire(2))
        t0 = time.monotonic()
        # Needs 1 token: takes 1.0s, timeout is 0.1s -> should fail
        self.assertFalse(tb.acquire(1, timeout=0.1))
        elapsed = time.monotonic() - t0
        self.assertGreaterEqual(elapsed, 0.09)
        self.assertLess(elapsed, 0.25)

    def test_concurrency_no_overadmission(self) -> None:
        tb = TokenBucketRateLimiter(capacity=50.0, refill_rate_per_sec=100.0)
        num_threads = 10
        calls_per_thread = 20
        start = time.monotonic()
        granted = 0
        lock = threading.Lock()

        def worker() -> None:
            nonlocal granted
            local_granted = 0
            for _ in range(calls_per_thread):
                if tb.acquire(1, timeout=0.05):
                    local_granted += 1
            with lock:
                granted += local_granted

        with ThreadPoolExecutor(max_workers=num_threads) as executor:
            list(executor.map(lambda _: worker(), range(num_threads)))

        elapsed = time.monotonic() - start
        max_possible = 50.0 + (100.0 * elapsed) + 1.0
        self.assertLessEqual(granted, max_possible)
        self.assertGreaterEqual(granted, 50)  # initial capacity guaranteed


class TestSlidingWindowCounter(unittest.TestCase):
    def test_basic_window_quota(self) -> None:
        sw = SlidingWindowCounter(window_seconds=1.0, max_requests=3)
        self.assertTrue(sw.acquire(1))
        self.assertTrue(sw.acquire(1))
        self.assertTrue(sw.acquire(1))
        # 4th request must fail
        self.assertFalse(sw.acquire(1))

    def test_window_sliding_recovery(self) -> None:
        sw = SlidingWindowCounter(window_seconds=0.2, max_requests=2)
        self.assertTrue(sw.acquire(2))
        self.assertFalse(sw.acquire(1))
        time.sleep(0.25)
        # Window expired; should allow requests again
        self.assertTrue(sw.acquire(1))

    def test_all_or_nothing_atomicity(self) -> None:
        sw = SlidingWindowCounter(window_seconds=1.0, max_requests=3)
        self.assertTrue(sw.acquire(2))
        # Try to acquire 2: only 1 slot left, must reject without partial deduction
        self.assertFalse(sw.acquire(2))
        self.assertEqual(sw.stats()["remaining"], 1)
        self.assertTrue(sw.acquire(1))


class TestRateLimiterRegistry(unittest.TestCase):
    def test_named_limiter_creation(self) -> None:
        reg = RateLimiterRegistry(
            default_factory=lambda name: TokenBucketRateLimiter(capacity=10.0, refill_rate_per_sec=5.0)
        )
        l1 = reg.get_or_create("tenant_a")
        l2 = reg.get_or_create("tenant_a")
        self.assertIs(l1, l2)
        self.assertIn("tenant_a", reg)
        self.assertEqual(len(reg), 1)

    def test_concurrent_get_or_create_singleton(self) -> None:
        reg = RateLimiterRegistry(
            default_factory=lambda name: SlidingWindowCounter(window_seconds=1.0, max_requests=10)
        )
        instances: list[SlidingWindowCounter] = []
        barrier = threading.Barrier(16)

        def worker() -> None:
            barrier.wait()
            inst = reg.get_or_create("shared_tenant")
            instances.append(inst)

        with ThreadPoolExecutor(max_workers=16) as executor:
            list(executor.map(lambda _: worker(), range(16)))

        self.assertEqual(len(instances), 16)
        first = instances[0]
        self.assertTrue(all(i is first for i in instances))

    def test_registry_aggregate_stats(self) -> None:
        reg = RateLimiterRegistry()
        reg.register("t1", SlidingWindowCounter(window_seconds=1.0, max_requests=5))
        reg.register("t2", TokenBucketRateLimiter(capacity=5.0, refill_rate_per_sec=5.0))
        reg.acquire("t1", 2)
        reg.acquire("t2", 1)
        stats = reg.stats()
        self.assertEqual(stats["limiters"], 2)
        self.assertEqual(stats["allowed"], 2)
        self.assertEqual(stats["requests"], 2)


class TestValidationAndEdgeCases(unittest.TestCase):
    def test_invalid_parameters_raise(self) -> None:
        with self.assertRaises(ValueError):
            TokenBucketRateLimiter(capacity=0, refill_rate_per_sec=1)
        with self.assertRaises(ValueError):
            TokenBucketRateLimiter(capacity=1, refill_rate_per_sec=-5)
        with self.assertRaises(ValueError):
            SlidingWindowCounter(window_seconds=0, max_requests=5)
        with self.assertRaises(ValueError):
            SlidingWindowCounter(window_seconds=1, max_requests=0)

    def test_invalid_tokens_or_timeout_raise(self) -> None:
        tb = TokenBucketRateLimiter(capacity=5, refill_rate_per_sec=1)
        with self.assertRaises(ValueError):
            tb.acquire(tokens=0)
        with self.assertRaises(ValueError):
            tb.acquire(tokens=-1)
        with self.assertRaises(ValueError):
            tb.acquire(tokens=1, timeout=-0.5)


if __name__ == "__main__":
    unittest.main()
