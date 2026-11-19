import unittest

from report import summarize
from plan import proposal
from schedule import schedule


def frozen_plan(rows, campaign_type="full", decision_rule=None):
    plan = proposal("pilot" if campaign_type == "pilot" else "full")
    plan["runs"] = list(rows)
    plan["campaign_type"] = campaign_type
    if decision_rule is not None:
        plan["value_decision_rule"] = decision_rule
    return plan


class ReportTest(unittest.TestCase):
    def test_missing_and_failed_runs_remain_in_denominators(self):
        plan = list(schedule())[:6]
        results = [
            dict(plan[0], status="failed", correct=False),
            dict(plan[1], status="complete", correct=True, wall_seconds=7, agent_seconds=4),
            dict(plan[2], status="complete", correct=True, wall_seconds=5, agent_seconds=3,
                 cached_input_tokens=0),
        ]
        report = summarize(results, frozen_plan(plan))
        self.assertEqual(report["arms"]["M0"]["planned"], 2)
        self.assertEqual(report["arms"]["M0"]["missing"], 1)
        self.assertEqual(report["arms"]["M0"]["complete"], 0)
        self.assertEqual(report["arms"]["M0"]["agent_failures"], 1)
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
            summarize([entry, entry], frozen_plan(plan))
        with self.assertRaises(ValueError):
            summarize([dict(entry, mode="M2")], frozen_plan(plan))

    def test_context_pressure_uses_observed_peak_only(self):
        rows = [row for row in schedule()
                if row["task"] == "board-query" and row["repetition"] == 1
                and row["mode"] in ("M1", "M2")]
        result = summarize([
            dict(rows[0], status="complete", correct=True, peak_observed_context_fraction=0.81),
            dict(rows[1], status="incomplete"),
        ], frozen_plan(rows))
        pressure = result["long_task_context_pressure"]
        self.assertEqual(pressure["threshold_fraction"], 0.8)
        self.assertEqual(pressure["runs_at_or_above_threshold_by_mode"],
                         {"M0": 0, "M1": 1, "M2": 0})
        self.assertFalse(pressure["claim_eligible"])
        self.assertFalse(pressure["long_context_value_demonstrated"])
        self.assertEqual(pressure["conclusion_scope"], "ordinary_length_coding_only")
        self.assertEqual(result["campaign_execution"]["unstarted_runs"], 0)
        self.assertIn("descriptive", result["timing_interpretation"]["provider_cache_state"])
        self.assertIn("not provider-server timing",
                      result["timing_interpretation"]["relay_observed_response_timing"])

    def test_context_pressure_claim_requires_three_correct_m1_m2_pairs(self):
        plan = [row for row in schedule()
                if row["task"] == "board-query" and row["repetition"] <= 3]
        results = []
        for row in plan:
            results.append(dict(row, status="complete", correct=True,
                                input_tokens=100,
                                peak_observed_context_fraction=(
                                    0.83 if row["mode"] == "M1" else 0.35
                                )))
        result = summarize(results, frozen_plan(plan))
        pressure = result["long_task_context_pressure"]
        self.assertEqual(pressure["matched_m1_m2_pressure_pairs"], 3)
        self.assertTrue(pressure["claim_eligible"])
        self.assertFalse(pressure["long_context_value_demonstrated"])
        self.assertEqual(pressure["conclusion_scope"], "ordinary_length_coding_only")
        self.assertEqual(pressure["m2_minus_m1_on_pressure_pairs"]["matched_correct_pairs"], 3)
        self.assertEqual(pressure["m2_minus_m1_on_pressure_pairs"]
                         ["input_token_reduction_fraction"]["median"], 0)

    def test_each_mode_is_paired_against_baseline_and_by_task(self):
        plan = list(schedule())[:3]
        results = [dict(row, status="complete", correct=True, wall_seconds=seconds)
                   for row, seconds in zip(plan, (12, 10, 7))]
        report = summarize(results, frozen_plan(plan))
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
        report = summarize(results, frozen_plan(plan))
        self.assertEqual(report["comparisons"]["m2_minus_m0"]["matched_correct_pairs"], 0)
        self.assertIsNone(report["comparisons"]["m2_minus_m0"]["metrics"]["wall_seconds"]["median"])
        self.assertEqual(report["comparisons"]["m2_minus_m1"]["matched_correct_pairs"], 1)

    def test_go_requires_matched_token_gain_and_no_task_regression(self):
        plan = list(proposal()["runs"])
        results = []
        for row in plan:
            result = dict(row, status="complete", correct=True, input_tokens=100,
                          mcp_count=0, peak_observed_context_fraction=0.85)
            if row["mode"] == "M2":
                result.update(input_tokens=80, mcp_count=3,
                              peak_observed_context_fraction=0.35)
            results.append(result)
        report = summarize(results, frozen_plan(plan))
        self.assertEqual(report["recommendation"]["label"], "GO")
        self.assertEqual(report["recommendation"]["scope"], "long-context value demonstrated")

    def test_frozen_decision_rule_is_digest_bound_and_pressure_value_is_separate(self):
        from authorization import plan_digest

        plan = list(proposal()["runs"])
        rule = dict(proposal()["value_decision_rule"],
                    context_pressure_threshold_fraction=0.90)
        frozen = frozen_plan(plan, decision_rule=rule)
        results = []
        for row in plan:
            result = dict(row, status="complete", correct=True, input_tokens=100,
                          mcp_count=0, peak_observed_context_fraction=0.85)
            if row["mode"] == "M2":
                result.update(input_tokens=80, mcp_count=3,
                              peak_observed_context_fraction=0.35)
            if row["task"] == "board-query":
                result["input_tokens"] = 100
            results.append(result)

        report = summarize(results, frozen)
        self.assertEqual(report["authorized_plan_sha256"], plan_digest(frozen))
        self.assertEqual(report["value_decision_rule"], rule)
        self.assertEqual(report["long_task_context_pressure"]["threshold_fraction"], 0.90)
        self.assertEqual(report["long_task_context_pressure"]["conclusion_scope"],
                         "ordinary_length_coding_only")
        self.assertEqual(report["recommendation"]["scope"], "ordinary-length coding only")

    def test_pilot_never_claims_product_value(self):
        plan = list(proposal("pilot")["runs"])
        results = [dict(row, status="complete", correct=True, input_tokens=10, mcp_count=1)
                   for row in plan]
        report = summarize(results, frozen_plan(plan, "pilot"))
        self.assertEqual(report["recommendation"]["label"], "ITERATE")

    def test_uncontrolled_cache_latency_gain_cannot_produce_go(self):
        plan = list(proposal()["runs"])
        results = []
        for row in plan:
            result = dict(row, status="complete", correct=True, input_tokens=100,
                          wall_seconds=10, mcp_count=0)
            if row["mode"] == "M2":
                result.update(wall_seconds=5, mcp_count=3)
            results.append(result)
        report = summarize(results, frozen_plan(plan))
        self.assertEqual(report["recommendation"]["label"], "NO-GO")
        self.assertIn("descriptive", report["timing_interpretation"]["provider_cache_state"])
