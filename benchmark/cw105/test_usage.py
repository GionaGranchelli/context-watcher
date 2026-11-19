import unittest

from usage import Usage, parse_usage


class UsageTest(unittest.TestCase):
    def test_missing_usage_is_unobserved(self):
        self.assertIsNone(parse_usage({"usage": None}))
        self.assertIsNone(parse_usage({"choices": []}))

    def test_distinguishes_missing_cache_from_zero(self):
        usage = {"prompt_tokens": 13, "completion_tokens": 5, "total_tokens": 18}
        self.assertEqual(parse_usage({"usage": usage}), Usage(13, 5, None))
        for cached in (0, 11):
            usage["prompt_tokens_details"] = {"cached_tokens": cached}
            self.assertEqual(parse_usage({"usage": usage}), Usage(13, 5, cached))

    def test_rejects_invalid_counts(self):
        for usage in (
            {"prompt_tokens": True, "completion_tokens": 1},
            {"prompt_tokens": 1, "completion_tokens": -1},
            {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 1},
            {"prompt_tokens": 1, "completion_tokens": 1,
             "prompt_tokens_details": {"cached_tokens": 2}},
        ):
            with self.subTest(usage=usage), self.assertRaises(ValueError):
                parse_usage({"usage": usage})
