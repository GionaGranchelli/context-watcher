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
        self.assertEqual(plan["max_wall_seconds"], 108600)
        self.assertIsNone(plan["provider_cost_cap_nanodollars"])
        self.assertIsNone(plan["pricing"]["evidence"])
        self.assertEqual(plan["local_resource_cap"]["gpu_watts"], 707)
        self.assertEqual(plan_digest(plan), plan_digest(proposal()))
