from storage import Storage

class Cache:
    def __init__(self, clock):
        self.clock = clock
        self.storage = Storage()

    def put(self, namespace, key, value, ttl):
        self.storage.put(namespace, key, value, self.clock() + ttl)

    def get(self, namespace, key, default=None):
        return self.storage.get(namespace, key, self.clock(), default)
