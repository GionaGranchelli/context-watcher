import subprocess
import unittest
from unittest.mock import patch

from budget import BudgetExceeded
from machinist import check_loaded_model, read_sample


class MachinistTest(unittest.TestCase):
    @patch("machinist.subprocess.run", side_effect=subprocess.TimeoutExpired("ssh", 0.2))
    def test_model_inventory_uses_remaining_deadline(self, run):
        with self.assertRaisesRegex(BudgetExceeded, "inventory unavailable"):
            check_loaded_model("model", 100, remaining=lambda: 0.2)
        self.assertEqual(run.call_args.kwargs["timeout"], 0.2)

    @patch("machinist.subprocess.run", side_effect=subprocess.TimeoutExpired("ssh", 0.2))
    def test_gpu_sample_uses_remaining_deadline(self, run):
        with self.assertRaisesRegex(BudgetExceeded, "telemetry could not be read"):
            read_sample(max_watts=100, max_junction_c=80, max_age=5, remaining=lambda: 0.2)
        self.assertEqual(run.call_args.kwargs["timeout"], 0.2)
