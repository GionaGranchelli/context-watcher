"""
Unit test suite verifying the exact code produced by flint on 192.168.1.156:8090.
"""

import unittest
from scheduler import TaskScheduler

class TestFlintTaskScheduler(unittest.TestCase):
    def setUp(self):
        self.scheduler = TaskScheduler()

    def test_add_and_initial_status(self):
        self.scheduler.add_task("task_init", lambda: 100, max_retries=0)
        st = self.scheduler.status()
        self.assertEqual(st.get("task_init"), "pending")

    def test_successful_run(self):
        self.scheduler.add_task("math", lambda: 21 * 2, max_retries=1)
        results = self.scheduler.run_all()
        self.assertEqual(len(results), 1)
        self.assertEqual(results[0]["status"], "succeeded")
        self.assertEqual(results[0]["result"], 42)
        self.assertEqual(results[0]["attempts"], 1)
        self.assertEqual(self.scheduler.status()["math"], "succeeded")

    def test_retry_eventual_success(self):
        attempts = 0
        def intermittent():
            nonlocal attempts
            attempts += 1
            if attempts < 3:
                raise ValueError("temporary error")
            return "recovered"

        self.scheduler.add_task("retry_task", intermittent, max_retries=3)
        results = self.scheduler.run_all()
        self.assertEqual(results[0]["status"], "succeeded")
        self.assertEqual(results[0]["result"], "recovered")
        self.assertEqual(results[0]["attempts"], 3)
        self.assertEqual(self.scheduler.status()["retry_task"], "succeeded")

    def test_retry_exhaustion_failure(self):
        def always_fails():
            raise RuntimeError("permanent crash")

        self.scheduler.add_task("fatal", always_fails, max_retries=2)
        results = self.scheduler.run_all()
        self.assertEqual(results[0]["status"], "failed")
        self.assertIn("permanent crash", results[0]["error"])
        self.assertEqual(results[0]["attempts"], 3)
        self.assertEqual(self.scheduler.status()["fatal"], "failed")

    def test_multiple_sequential_tasks(self):
        for i in range(5):
            self.scheduler.add_task(f"t_{i}", lambda idx=i: idx * 10, max_retries=0)

        results = self.scheduler.run_all()
        self.assertEqual(len(results), 5)
        for i, res in enumerate(results):
            self.assertEqual(res["result"], i * 10)
            self.assertEqual(res["status"], "succeeded")

    def test_zero_retries_fails_on_single_attempt(self):
        self.scheduler.add_task("no_retry", lambda: 1 / 0, max_retries=0)
        results = self.scheduler.run_all()
        self.assertEqual(results[0]["attempts"], 1)
        self.assertEqual(results[0]["status"], "failed")

    def test_status_mapping_keys(self):
        self.scheduler.add_task("a", lambda: "a", max_retries=0)
        self.scheduler.add_task("b", lambda: "b", max_retries=0)
        st = self.scheduler.status()
        self.assertEqual(set(st.keys()), {"a", "b"})

if __name__ == "__main__":
    unittest.main()
