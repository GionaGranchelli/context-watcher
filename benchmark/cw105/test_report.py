import unittest

from report import summarize
from schedule import schedule


class ReportTest(unittest.TestCase):
    def test_missing_and_failed_runs_remain_in_denominators(self):
        plan = list(schedule())[:6]
        results = [
            dict(plan[0], status="failed", correct=False),
            dict(plan[1], status="complete", correct=True, wall_seconds=7),
            dict(plan[2], status="complete", correct=True, wall_seconds=5,
                 cached_input_tokens=0),
        ]
        report = summarize(results, plan)
        self.assertEqual(report["arms"]["M0"]["planned"], 2)
        self.assertEqual(report["arms"]["M0"]["missing"], 1)
        self.assertEqual(report["arms"]["M0"]["complete"], 0)
        self.assertEqual(report["matched_correct_pairs"], 1)
        self.assertEqual(report["m2_minus_m1"]["wall_seconds"], {"count": 1, "median": -2})
        self.assertEqual(report["m2_minus_m1"]["cached_input_tokens"]["count"], 0)

    def test_refuses_duplicate_or_mislabeled_results(self):
        plan = list(schedule())[:1]
        entry = dict(plan[0], status="incomplete")
        with self.assertRaises(ValueError):
            summarize([entry, entry], plan)
        with self.assertRaises(ValueError):
            summarize([dict(entry, mode="M2")], plan)
