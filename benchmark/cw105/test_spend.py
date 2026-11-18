import unittest

from budget import BudgetExceeded
from spend import Spend
from usage import Usage


class SpendTest(unittest.TestCase):
    def test_reserves_upper_bound_and_charges_observed_usage(self):
        spend = Spend(cap=30, input_rate=2, output_rate=3, max_input=10, max_output=3)
        spend.reserve()
        self.assertEqual(spend.settle(Usage(2, 1, None)), 7)
        with self.assertRaises(BudgetExceeded):
            spend.reserve()

    def test_missing_usage_freezes_admission_even_at_zero_price(self):
        spend = Spend(cap=0, input_rate=0, output_rate=0, max_input=10, max_output=3)
        spend.reserve()
        with self.assertRaises(BudgetExceeded):
            spend.settle(None)
        with self.assertRaises(BudgetExceeded):
            spend.reserve()

    def test_missing_price_is_rejected(self):
        with self.assertRaises(ValueError):
            Spend(cap=0, input_rate=None, output_rate=0, max_input=10, max_output=3)
