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

    def test_running_guard_observes_latched_fault_and_deadline(self):
        now = [0]
        budget = Budget(campaign_limit=1, seconds=20, clock=lambda: now[0])
        budget.start("a", request_limit=1, seconds=5)
        budget.admit("a")
        budget.check_running("a")
        now[0] = 5
        with self.assertRaisesRegex(BudgetExceeded, "deadline"):
            budget.check_running("a")
        budget.stop("usage fault")
        with self.assertRaisesRegex(BudgetExceeded, "usage fault"):
            budget.check_running("a")

    def test_finished_run_cannot_request_or_replay_but_next_run_can_start(self):
        budget = Budget(campaign_limit=3, seconds=10, clock=lambda: 0)
        budget.start("first", request_limit=2, seconds=10)
        budget.finish("first")
        with self.assertRaises(BudgetExceeded):
            budget.admit("first")
        with self.assertRaises(BudgetExceeded):
            budget.check_running("first")
        with self.assertRaises(ValueError):
            budget.start("first", request_limit=1, seconds=10)
        budget.start("second", request_limit=1, seconds=10)
        self.assertEqual(budget.admit("second"), 10)
