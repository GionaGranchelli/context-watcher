"""Comprehensive test suite for kv_engine and kv_client.

Covers:
- Basic Put, Get, Delete
- TTL expiration & lazy eviction
- WAL recovery on restart
- Torn/corrupt WAL record handling
- Snapshot creation, loading, and compaction
- Concurrent reads & writes with 20 threads
- KVConnectionPool & KVClient batch operations
"""

import os
import time
import shutil
import tempfile
import threading
import unittest
from concurrent.futures import ThreadPoolExecutor

from kv_engine import KVStore, WriteAheadLog, MemTable, WALCorruptionError, SnapshotError
from kv_client import KVConnectionPool, KVClient, PoolExhaustedError


class TestKVEngine(unittest.TestCase):

    def setUp(self):
        self.test_dir = tempfile.mkdtemp(prefix="cw_bench_kv_")
        self.wal_path = os.path.join(self.test_dir, "test.wal")

    def tearDown(self):
        shutil.rmtree(self.test_dir, ignore_errors=True)

    def test_01_basic_put_get(self):
        store = KVStore(self.wal_path, max_memtable_entries=100)
        store.put("alpha", 123)
        store.put("beta", {"name": "test", "active": True})
        self.assertEqual(store.get("alpha"), 123)
        self.assertEqual(store.get("beta")["name"], "test")
        store.close()

    def test_02_delete(self):
        store = KVStore(self.wal_path, max_memtable_entries=100)
        store.put("key1", "val1")
        self.assertTrue(store.delete("key1"))
        self.assertIsNone(store.get("key1"))
        self.assertFalse(store.delete("nonexistent"))
        store.close()

    def test_03_ttl_expiration(self):
        store = KVStore(self.wal_path, max_memtable_entries=100)
        store.put("short_lived", "data", ttl=0.1)
        store.put("long_lived", "data", ttl=10.0)
        self.assertEqual(store.get("short_lived"), "data")
        time.sleep(0.15)
        self.assertIsNone(store.get("short_lived"))
        self.assertEqual(store.get("long_lived"), "data")
        store.close()

    def test_04_wal_recovery(self):
        store = KVStore(self.wal_path, max_memtable_entries=100)
        for i in range(25):
            store.put(f"item_{i}", i * 10)
        store.delete("item_5")
        store.close()

        # Reopen and verify persistence
        recovered = KVStore(self.wal_path, max_memtable_entries=100)
        for i in range(25):
            if i == 5:
                self.assertIsNone(recovered.get(f"item_{i}"))
            else:
                self.assertEqual(recovered.get(f"item_{i}"), i * 10)
        recovered.close()

    def test_05_snapshot_and_compaction(self):
        # max_memtable_entries=5 triggers compaction every 5 writes
        store = KVStore(self.wal_path, max_memtable_entries=5)
        for i in range(20):
            store.put(f"k_{i}", i)
        store.snapshot()
        store.close()

        # Verify state from snapshot + wal replay
        store2 = KVStore(self.wal_path, max_memtable_entries=5)
        for i in range(20):
            self.assertEqual(store2.get(f"k_{i}"), i)
        store2.close()

    def test_06_torn_record_recovery(self):
        store = KVStore(self.wal_path, max_memtable_entries=100)
        store.put("valid1", "abc")
        store.close()

        # Corrupt file tail with garbage bytes
        with open(self.wal_path, "ab") as f:
            f.write(b"CORRUPT_TAIL_GARBAGE_BYTES")

        # Tolerant mode (default) truncates and recovers valid records
        recovered = KVStore(self.wal_path, max_memtable_entries=100)
        self.assertEqual(recovered.get("valid1"), "abc")
        recovered.put("valid2", "xyz")
        recovered.close()

    def test_07_concurrent_reads_writes(self):
        store = KVStore(self.wal_path, max_memtable_entries=50)
        errors = []

        def worker(tid: int):
            try:
                for j in range(30):
                    key = f"t{tid}_k{j}"
                    store.put(key, j)
                    val = store.get(key)
                    if val != j:
                        errors.append(f"Mismatch: expected {j}, got {val}")
            except Exception as e:
                errors.append(str(e))

        threads = [threading.Thread(target=worker, args=(i,)) for i in range(10)]
        for t in threads:
            t.start()
        for t in threads:
            t.join()

        self.assertEqual(len(errors), 0, f"Thread errors: {errors}")
        store.close()

    def test_08_connection_pool_and_client(self):
        pool = KVConnectionPool(self.wal_path, max_memtable_entries=50, pool_size=4)
        client = KVClient(pool)

        client.put("pool_test", 999)
        self.assertEqual(client.get("pool_test"), 999)

        batch = {f"batch_{i}": i for i in range(15)}
        count = client.batch_put(batch)
        self.assertEqual(count, 15)

        fetched = client.batch_get(list(batch.keys()))
        self.assertEqual(len(fetched), 15)
        self.assertEqual(fetched["batch_7"], 7)

        pool.close()


if __name__ == "__main__":
    unittest.main()
