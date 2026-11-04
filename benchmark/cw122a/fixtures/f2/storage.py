"""In-memory storage used by the namespace cache service."""
class Storage:
    def __init__(self):
        self.entries = {}

    def put(self, namespace, key, value, expires):
        self.entries[key] = (value, expires)

    def get(self, namespace, key, now, default=None):
        entry = self.entries.get(key)
        if entry is None or entry[1] < now:
            return default
        return entry[0]
