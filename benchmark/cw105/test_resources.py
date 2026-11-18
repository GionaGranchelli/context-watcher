import unittest

from budget import BudgetExceeded
from resources import GpuSample, check_sample


class ResourceTest(unittest.TestCase):
    limits = dict(expected_devices={"a", "b"}, max_watts=100, max_junction_c=80,
                  age=1, max_age=2)

    def test_complete_fresh_measurement(self):
        self.assertEqual(check_sample(
            [GpuSample("a", 45, 40), GpuSample("b", 55, 41)], **self.limits), 100)

    def test_refuses_missing_stale_overheated_and_over_budget_samples(self):
        good = [GpuSample("a", 45, 40), GpuSample("b", 55, 41)]
        cases = [
            (good[:1], self.limits),
            (good, dict(self.limits, age=3)),
            ([good[0], GpuSample("b", 55, 80)], self.limits),
            ([good[0], GpuSample("b", 56, 41)], self.limits),
            ([good[0], GpuSample("b", float("nan"), 41)], self.limits),
        ]
        for rows, limits in cases:
            with self.subTest(rows=rows, limits=limits), self.assertRaises(BudgetExceeded):
                check_sample(rows, **limits)
