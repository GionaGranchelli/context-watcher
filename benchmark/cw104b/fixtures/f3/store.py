# store.py
class KeyValueStore:
    def __init__(self):
        self._data = {}
    def put(self, k, v):
        self._data[k] = v
    def get(self, k):
        return self._data.get(k)
