import json
import unittest

from budget import BudgetExceeded
from request_policy import check_request


class RequestPolicyTest(unittest.TestCase):
    def test_requires_bound_model_and_explicit_output_limit(self):
        valid = {"model": "fixture", "max_tokens": 8, "stream": True}
        check_request(json.dumps(valid), model="fixture", max_output_tokens=8)
        for change in ({"model": "other"}, {"max_tokens": None}, {"max_tokens": 9},
                       {"max_tokens": True}, {"stream": False}, {"n": 2},
                       {"max_completion_tokens": 8}):
            with self.subTest(change=change), self.assertRaises(BudgetExceeded):
                check_request(json.dumps(dict(valid, **change)),
                              model="fixture", max_output_tokens=8)
