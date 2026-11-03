import unittest
from store import KeyValueStore
from cache import CachedStore

class TestCacheSystem(unittest.TestCase):
    def test_cached_lookup(self):
        backend = KeyValueStore()
        backend.put('user_1', 'Alice')
        cached = CachedStore(backend)
        self.assertEqual(cached.get('user_1'), 'Alice')
        backend.put('user_1', 'MutatedDirectly')
        # Should still return cached value 'Alice'
        self.assertEqual(cached.get('user_1'), 'Alice')

    def test_put_updates_both(self):
        backend = KeyValueStore()
        cached = CachedStore(backend)
        cached.put('key', 'val')
        self.assertEqual(cached.get('key'), 'val')
        self.assertEqual(backend.get('key'), 'val')

if __name__ == '__main__':
    unittest.main()
