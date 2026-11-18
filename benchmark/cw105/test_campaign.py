import unittest

from budget import Budget, BudgetExceeded
from campaign import execute_plan
from report import summarize
from schedule import schedule


class CampaignTest(unittest.TestCase):
    def test_incorrect_submission_stops_before_next_arm(self):
        plan = list(schedule())[:3]
        executable = [dict(row, request_cap=2, budget_seconds=10) for row in plan]
        budget = Budget(campaign_limit=6, seconds=30, clock=lambda: 0)
        started = []

        def execute(row):
            started.append(row["run_id"])
            budget.admit(row["run_id"])
            return dict(plan[0], status="complete", correct=False)

        results = execute_plan(executable, budget, execute)
        self.assertEqual(started, [plan[0]["run_id"]])
        report = summarize(results, plan)
        self.assertEqual(report["arms"]["M0"]["correct"], 0)
        self.assertEqual(report["arms"]["M1"]["missing"], 1)
        self.assertEqual(report["arms"]["M2"]["missing"], 1)
        with self.assertRaises(BudgetExceeded):
            budget.admit(plan[0]["run_id"])

    def test_expired_campaign_launches_nothing(self):
        clock = [0]
        budget = Budget(campaign_limit=1, seconds=10, clock=lambda: clock[0])
        clock[0] = 10
        self.assertEqual(execute_plan(
            [{"run_id": "expired", "request_cap": 1, "budget_seconds": 5}],
            budget, lambda _: self.fail("expired attempt launched"),
        ), [])
