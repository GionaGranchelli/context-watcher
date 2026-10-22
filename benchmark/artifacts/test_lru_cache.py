import time
import unittest

from lru_cache import LRUCache


class TestLRUCache(unittest.TestCase):
    def test_put_and_get(self):
        c = LRUCache(2)
        c.put("a", 1)
        self.assertEqual(c.get("a"), 1)
        self.assertIsNone(c.get("nope"))

    def test_get_default(self):
        c = LRUCache(1)
        self.assertEqual(c.get("x", "fallback"), "fallback")

    def test_eviction_when_capacity_exceeded(self):
        c = LRUCache(2)
        c.put("a", 1)
        c.put("b", 2)
        c.get("a")  # refresh recency of "a"
        c.put("c", 3)  # evicts LRU -> "b"
        self.assertEqual(c.get("a"), 1)
        self.assertIsNone(c.get("b"))
        self.assertEqual(c.stats()["evictions"], 1)

    def test_update_does_not_evict(self):
        c = LRUCache(1)
        c.put("a", 1)
        c.put("a", 2)
        self.assertEqual(c.get("a"), 2)
        self.assertEqual(c.stats()["evictions"], 0)

    def test_stats_tracking(self):
        c = LRUCache(1)
        c.put("a", 1)
        c.get("a")  # hit
        c.get("b")  # miss
        c.put("c", 2)  # eviction of "a"
        s = c.stats()
        self.assertEqual(s, {"hits": 1, "misses": 1, "evictions": 1})

    def test_ttl_expiry(self):
        c = LRUCache(2)
        c.put("a", 1, ttl_seconds=0.05)
        time.sleep(0.06)
        self.assertIsNone(c.get("a"))  # lazy expiration -> miss
        s = c.stats()
        self.assertEqual(s["misses"], 1)

    def test_evict_expired(self):
        c = LRUCache(4)
        c.put("a", 1, ttl_seconds=0.05)
        c.put("b", 2)
        time.sleep(0.06)
        removed = c.evict_expired()
        self.assertEqual(removed, 1)
        self.assertIsNone(c.get("a"))
        self.assertEqual(c.get("b"), 2)


if __name__ == "__main__":
    unittest.main()
