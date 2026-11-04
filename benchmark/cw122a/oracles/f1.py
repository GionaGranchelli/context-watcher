"""Independent oracle: executed outside the agent-editable repository."""
import json
import inspect
import unittest
from retry import retry_delays

class RetryAcceptance(unittest.TestCase):
    def test_public_signature(self):
        self.assertEqual(str(inspect.signature(retry_delays)), '(attempts, base=0.25, cap=10.0)')

    def test_capped_sequence(self):
        self.assertEqual(retry_delays(8), [.25, .5, 1., 2., 4., 8., 10., 10.])
        self.assertEqual(retry_delays(4, 1, 2), [1, 2, 2, 2])

    def test_empty(self):
        self.assertEqual(retry_delays(0), [])

    def test_invalid(self):
        for args in [(-1,), (2, 0), (2, -1), (2, 2, 1)]:
            with self.subTest(args=args), self.assertRaises(ValueError):
                retry_delays(*args)

if __name__ == '__main__':
    result = unittest.TextTestRunner(verbosity=2).run(unittest.defaultTestLoader.loadTestsFromTestCase(RetryAcceptance))
    completed = {'tests_run': result.testsRun, 'passed': result.wasSuccessful()}
    print('CW122_ORACLE_COMPLETED:' + json.dumps(completed, sort_keys=True))
    raise SystemExit(0 if completed['tests_run'] == 4 and completed['passed'] else 1)
