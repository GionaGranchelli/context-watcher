import unittest
from math_ops import divide_or_fallback, safe_mean

class TestMathOps(unittest.TestCase):
    def test_divide(self):
        self.assertEqual(divide_or_fallback(10, 2), 5.0)
        self.assertEqual(divide_or_fallback(10, 0, fallback=99.0), 99.0)

    def test_mean(self):
        self.assertEqual(safe_mean([1, 2, 3]), 2.0)
        self.assertEqual(safe_mean([]), 0.0)

if __name__ == '__main__':
    unittest.main()
