import unittest

from authorization import plan_digest
from plan import proposal


class PlanTest(unittest.TestCase):
    def test_frozen_workload_and_ceiling(self):
        plan = proposal()
        self.assertEqual(len(plan["runs"]), 54)
        self.assertEqual(plan["max_task_requests"], 1008)
        self.assertEqual(plan["max_agent_seconds"], 75600)
        self.assertIsNone(plan["provider_cost_cap_usd"])
        self.assertEqual(plan_digest(plan), plan_digest(proposal()))
