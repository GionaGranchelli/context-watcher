import unittest

from evidence import request_metrics


class EvidenceTest(unittest.TestCase):
    def test_complete_usage_preserves_missing_cache_data(self):
        first = dict(complete=True, ttft_seconds=2, usage={
            "input_tokens": 5, "output_tokens": 2, "cached_input_tokens": 0,
        })
        second = dict(complete=True, ttft_seconds=4, usage={
            "input_tokens": 3, "output_tokens": 1, "cached_input_tokens": None,
        })
        result = request_metrics([first, second])
        self.assertEqual(result["input_tokens"], 8)
        self.assertEqual(result["output_tokens"], 3)
        self.assertIsNone(result["cached_input_tokens"])
        self.assertEqual(result["ttft_seconds"], 3)

    def test_partial_records_preserve_timing_without_total(self):
        result = request_metrics([dict(complete=False, usage=None, ttft_seconds=4)])
        self.assertFalse(result["telemetry_complete"])
        self.assertIsNone(result["input_tokens"])
        self.assertEqual(result["ttft_seconds"], 4)
