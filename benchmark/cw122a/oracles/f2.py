import json
import inspect
import unittest
from storage import Storage
from service import Cache

class CacheAcceptance(unittest.TestCase):
    def test_public_signatures(self):
        self.assertEqual(str(inspect.signature(Cache.put)), '(self, namespace, key, value, ttl)')
        self.assertEqual(str(inspect.signature(Cache.get)), '(self, namespace, key, default=None)')
        self.assertEqual(str(inspect.signature(Storage.put)), '(self, namespace, key, value, expires)')
        self.assertEqual(str(inspect.signature(Storage.get)), '(self, namespace, key, now, default=None)')

    def test_namespace_isolation(self):
        cache = Cache(lambda: 10)
        cache.put('a', 'same', 'one', 5)
        cache.put('b', 'same', 'two', 5)
        self.assertEqual(cache.get('a', 'same'), 'one')
        self.assertEqual(cache.get('b', 'same'), 'two')
        self.assertEqual(cache.get('c', 'same', 'missing'), 'missing')

    def test_exact_expiry_and_falsy_values(self):
        now = [10]
        cache = Cache(lambda: now[0])
        cache.put('a', 'key', False, 2)
        self.assertIs(cache.get('a', 'key'), False)
        now[0] = 12
        self.assertEqual(cache.get('a', 'key', 'expired'), 'expired')

    def test_negative_ttl_rejected(self):
        with self.assertRaises(ValueError):
            Cache(lambda: 0).put('a', 'key', 'v', -1)

if __name__ == '__main__':
    result = unittest.TextTestRunner(verbosity=2).run(unittest.defaultTestLoader.loadTestsFromTestCase(CacheAcceptance))
    completed = {'tests_run': result.testsRun, 'passed': result.wasSuccessful()}
    print('CW122_ORACLE_COMPLETED:' + json.dumps(completed, sort_keys=True))
    raise SystemExit(0 if completed['tests_run'] == 4 and completed['passed'] else 1)
