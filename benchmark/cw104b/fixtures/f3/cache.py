# cache.py - skeleton
class CachedStore:
    def __init__(self, backing_store):
        self.backing_store = backing_store
        self.cache = {}
    def get(self, k):
        return None
    def put(self, k, v):
        pass
