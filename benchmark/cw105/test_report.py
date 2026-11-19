import unittest

from report import summarize
from schedule import schedule


class ReportTest(unittest.TestCase):
    def test_missing_and_failed_runs_remain_in_denominators(self):
        plan = list(schedule())[:6]
        results = [
            dict(plan[0], status="failed", correct=False),
            dict(plan[1], status="complete", correct=True, wall_seconds=7, agent_seconds=4),
            dict(plan[2], status="complete", correct=True, wall_seconds=5, agent_seconds=3,
                 cached_input_tokens=0),
        ]
        report = summarize(results, plan)
        self.assertEqual(report["arms"]["M0"]["planned"], 2)
        self.assertEqual(report["arms"]["M0"]["missing"], 1)
        self.assertEqual(report["arms"]["M0"]["complete"], 0)
        self.assertEqual(report["arms"]["M0"]["completion_rate"], 0)
        self.assertEqual(report["arms"]["M1"]["completion_rate"], 0.5)
        self.assertEqual(report["arms"]["M1"]["correctness_rate"], 0.5)
        self.assertEqual(report["comparisons"]["m2_minus_m1"]["matched_correct_pairs"], 1)
        self.assertEqual(report["comparisons"]["m2_minus_m1"]["metrics"]["wall_seconds"], {"count": 1, "median": -2})
        self.assertEqual(report["comparisons"]["m2_minus_m1"]["metrics"]["agent_seconds"], {"count": 1, "median": -1})
        self.assertEqual(report["comparisons"]["m2_minus_m1"]["metrics"]["cached_input_tokens"]["count"], 0)

    def test_refuses_duplicate_or_mislabeled_results(self):
        plan = list(schedule())[:1]
        entry = dict(plan[0], status="incomplete")
        with self.assertRaises(ValueError):
            summarize([entry, entry], plan)
        with self.assertRaises(ValueError):
            summarize([dict(entry, mode="M2")], plan)

    def test_context_pressure_uses_observed_peak_only(self):
        rows = [row for row in schedule() if row["task"] == "board-query"][:2]
        result = summarize([
            dict(rows[0], status="complete", correct=True, peak_observed_context_fraction=0.81),
            dict(rows[1], status="incomplete"),
        ], rows)
        self.assertEqual(result["long_task_context_pressure"], {
            "threshold_fraction": 0.8, "runs_at_or_above_threshold": 1, "planned_runs": 2,
        })

    def test_each_mode_is_paired_against_baseline_and_by_task(self):
        plan = list(schedule())[:3]
        results = [dict(row, status="complete", correct=True, wall_seconds=seconds)
                   for row, seconds in zip(plan, (12, 10, 7))]
        report = summarize(results, plan)
        self.assertEqual(report["comparisons"]["m1_minus_m0"]["metrics"]["wall_seconds"]["median"], -2)
        self.assertEqual(report["comparisons"]["m2_minus_m0"]["metrics"]["wall_seconds"]["median"], -5)
        task = plan[0]["task"]
        pair = report["comparisons_by_task"][task]["m2_minus_m1"]
        self.assertEqual(pair["matched_correct_pairs"], 1)
        self.assertEqual(pair["metrics"]["wall_seconds"]["median"], -3)

    def test_incorrect_baseline_is_excluded_from_paired_metrics(self):
        plan = list(schedule())[:3]
        results = [dict(row, status="complete", correct=(row["mode"] != "M0"), wall_seconds=1)
                   for row in plan]
        report = summarize(results, plan)
        self.assertEqual(report["comparisons"]["m2_minus_m0"]["matched_correct_pairs"], 0)
        self.assertIsNone(report["comparisons"]["m2_minus_m0"]["metrics"]["wall_seconds"]["median"])
        self.assertEqual(report["comparisons"]["m2_minus_m1"]["matched_correct_pairs"], 1)
