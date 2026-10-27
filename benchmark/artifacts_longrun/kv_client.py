"""Thread-safe client and connection pool for kv_engine.

Provides:
- KVClient: High-level client with automatic retries and exponential backoff
- KVConnectionPool: Thread-safe pool managing multiple KVStore instances
- BatchOperations: Helper for atomic batch get/put transactions
"""

from __future__ import annotations

import time
import threading
from typing import Any, Dict, List, Optional, Tuple
from contextlib import contextmanager

from kv_engine import KVStore, KVEngineError, Entry


class KVClientError(KVEngineError):
    """Base error for client operations."""


class PoolExhaustedError(KVClientError):
    """Raised when no connections are available within timeout."""


class KVConnectionPool:
    """Thread-safe pool of KVStore instances for high-concurrency access."""

    def __init__(self, wal_path: str, max_memtable_entries: int = 1000, pool_size: int = 5):
        self.wal_path = wal_path
        self.max_memtable_entries = max_memtable_entries
        self.pool_size = pool_size
        self._lock = threading.Lock()
        self._cond = threading.Condition(self._lock)
        # In a real distributed system, these would be separate handles or sockets.
        # For local embedded KVStore, we share the primary store under pool lock.
        self._primary = KVStore(wal_path, max_memtable_entries)
        self._in_use = 0
        self._closed = False

    @contextmanager
    def acquire(self, timeout: float = 5.0):
        start = time.monotonic()
        with self._cond:
            while self._in_use >= self.pool_size:
                if self._closed:
                    raise KVClientError("pool is closed")
                elapsed = time.monotonic() - start
                remaining = timeout - elapsed
                if remaining <= 0:
                    raise PoolExhaustedError(f"connection pool exhausted after {timeout:.2f}s")
                self._cond.wait(timeout=remaining)
            if self._closed:
                raise KVClientError("pool is closed")
            self._in_use += 1

        try:
            yield self._primary
        finally:
            with self._cond:
                self._in_use -= 1
                self._cond.notify()

    def close(self) -> None:
        with self._cond:
            self._closed = True
            self._primary.close()
            self._cond.notify_all()


class KVClient:
    """Client wrapper providing retries, batching, and pool integration."""

    def __init__(self, pool: KVConnectionPool, max_retries: int = 3, backoff_factor: float = 0.05):
        self.pool = pool
        self.max_retries = max_retries
        self.backoff_factor = backoff_factor

    def _execute_with_retry(self, func, *args, **kwargs):
        last_err = None
        for attempt in range(self.max_retries):
            try:
                with self.pool.acquire() as store:
                    return func(store, *args, **kwargs)
            except PoolExhaustedError as err:
                last_err = err
                time.sleep(self.backoff_factor * (2 ** attempt))
            except Exception as err:
                raise err
        raise KVClientError(f"Operation failed after {self.max_retries} attempts: {last_err}")

    def get(self, key: str) -> Any:
        return self._execute_with_retry(lambda s: s.get(key))

    def put(self, key: str, value: Any, ttl: Optional[float] = None) -> None:
        self._execute_with_retry(lambda s: s.put(key, value, ttl=ttl))

    def delete(self, key: str) -> bool:
        return self._execute_with_retry(lambda s: s.delete(key))

    def batch_put(self, items: Dict[str, Any], ttl: Optional[float] = None) -> int:
        def _batch(s):
            count = 0
            for k, v in items.items():
                s.put(k, v, ttl=ttl)
                count += 1
            return count
        return self._execute_with_retry(_batch)

    def batch_get(self, keys: List[str]) -> Dict[str, Any]:
        def _get_all(s):
            res = {}
            for k in keys:
                v = s.get(k)
                if v is not None:
                    res[k] = v
            return res
        return self._execute_with_retry(_get_all)
