import unittest

from authorization import plan_digest
from plan import proposal


class PlanTest(unittest.TestCase):
    def test_frozen_workload_and_ceiling(self):
        plan = proposal()
        self.assertEqual(len(plan["runs"]), 54)
        self.assertEqual(plan["max_task_requests"], 1008)
        self.assertEqual(plan["max_agent_seconds"], 75600)
        self.assertEqual(plan["max_requests"], 1012)
        self.assertEqual(plan["mcp_calls_per_run"], 64)
        self.assertEqual(plan["max_mcp_calls"], 1152)
        self.assertEqual(plan["max_admission_window_seconds"], 108600)
        self.assertIn("finalization", plan["wall_time_contract"])
        self.assertIn("No harness-level retries", plan["retry_policy"])
        self.assertEqual(plan["provider_cost_cap_nanodollars"], 0)
        self.assertIn("https://lmstudio.ai/docs/app/offline", plan["pricing"]["evidence"])
        self.assertEqual(plan["local_resource_cap"]["gpu_watts"], 707)
        self.assertEqual(plan_digest(plan), plan_digest(proposal()))

    def test_pilot_is_one_frozen_matched_board_query_block(self):
        plan = proposal("pilot")
        self.assertEqual(plan["campaign_type"], "pilot")
        self.assertEqual([(row["task"], row["repetition"], row["mode"])
                          for row in plan["runs"]], [
            ("board-query", 1, "M0"), ("board-query", 1, "M1"),
            ("board-query", 1, "M2"),
        ])
        self.assertEqual(plan["max_task_requests"], 96)
        self.assertEqual(plan["max_requests"], 100)
        self.assertEqual(plan["max_mcp_calls"], 64)
        self.assertEqual(plan["max_admission_window_seconds"], 9600)
