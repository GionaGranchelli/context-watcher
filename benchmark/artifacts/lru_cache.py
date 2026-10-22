"""Thread-safe LRU cache with optional per-entry TTL."""

import threading
import time
from collections import OrderedDict


class LRUCache:
    def __init__(self, capacity):
        if capacity < 1:
            raise ValueError("capacity must be >= 1")
        self._capacity = capacity
        self._data = OrderedDict()  # key -> (value, expires_at | None)
        self._lock = threading.Lock()
        self._hits = 0
        self._misses = 0
        self._evictions = 0

    def get(self, key, default=None):
        with self._lock:
            entry = self._data.get(key)
            if entry is None or (entry[1] is not None and time.monotonic() >= entry[1]):
                if entry is not None:
                    del self._data[key]  # lazily drop expired entry
                self._misses += 1
                return default
            value, _ = entry
            self._data.move_to_end(key)
            self._hits += 1
            return value

    def put(self, key, val, ttl_seconds=None):
        with self._lock:
            now = time.monotonic()
            expires = None if ttl_seconds is None else now + ttl_seconds
            self._data[key] = (val, expires)
            self._data.move_to_end(key)
            while len(self._data) > self._capacity:
                self._data.popitem(last=False)
                self._evictions += 1

    def evict_expired(self):
        """Remove all expired entries; returns the number removed."""
        now = time.monotonic()
        with self._lock:
            expired = [k for k, (_, exp) in self._data.items() if exp is not None and now >= exp]
            for key in expired:
                del self._data[key]
            return len(expired)

    def stats(self):
        with self._lock:
            return {"hits": self._hits, "misses": self._misses, "evictions": self._evictions}

    def __len__(self):
        with self._lock:
            return len(self._data)

    def __contains__(self, key):
        with self._lock:
            entry = self._data.get(key)
            if entry is None or (entry[1] is not None and time.monotonic() >= entry[1]):
                return False
            return True
