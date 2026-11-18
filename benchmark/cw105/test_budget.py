import unittest

from budget import Budget, BudgetExceeded


class BudgetTest(unittest.TestCase):
    def test_caps_are_shared_across_runs_and_retries(self):
        budget = Budget(campaign_limit=3, seconds=10, clock=lambda: 0)
        for run_id in ("a", "b"):
            budget.start(run_id, request_limit=2, seconds=5)
        self.assertEqual(budget.admit("a"), 5)
        budget.admit("a")
        with self.assertRaises(BudgetExceeded):
            budget.admit("a")
        budget.admit("b")
        with self.assertRaises(BudgetExceeded):
            budget.admit("b")

    def test_unknown_expired_and_replayed_runs_are_denied(self):
        now = [0]
        budget = Budget(campaign_limit=3, seconds=5, clock=lambda: now[0])
        with self.assertRaises(BudgetExceeded):
            budget.admit("unknown")
        budget.start("a", request_limit=3, seconds=10)
        now[0] = 5
        with self.assertRaises(BudgetExceeded):
            budget.admit("a")
        now[0] = 4
        with self.assertRaises(ValueError):
            budget.start("a", request_limit=3, seconds=10)
