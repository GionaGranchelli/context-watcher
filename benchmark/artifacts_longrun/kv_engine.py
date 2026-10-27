"""Crash-safe, thread-safe key-value engine: WAL + bounded memtable + snapshot.

Components
----------
``WriteAheadLog(filepath)``
    Append-only, CRC32-framed record log, fsync'd per record. Every mutation is
    durably recorded *before* the in-memory state changes, so an acknowledged
    write is never lost to a crash. A torn record at the tail (crash mid-append)
    is detected by CRC and dropped on recovery.

``MemTable``
    Bounded in-memory write buffer (``key -> Entry``) holding everything written
    since the last compaction. Tombstones are kept so deletes survive compaction.

``KVStore(wal_path, max_memtable_entries)``
    Memtable + compacted base + WAL, guarded by one ``threading.RLock``.
    ``get``/``put``/``delete`` are single critical sections, so no reader can
    observe a half-applied mutation. When the memtable reaches
    ``max_memtable_entries`` the store compacts: merge into the base, write the
    base atomically to a snapshot file (temp + fsync + rename), truncate the WAL.

Durability / recovery model
---------------------------
Recovery = load snapshot, then replay WAL records with ``seq > snapshot_seq``.
Compaction writes the snapshot *before* truncating the WAL, so a crash between
the two is harmless: the replay skips records already covered by the snapshot.

Values must be JSON-serializable; keys must be ``str``. TTLs are seconds and are
stored as absolute epoch deadlines so they survive restarts. Expiration is lazy
(check on read) plus a periodic sweep via ``expire()``.

Example
-------
    store = KVStore("state.wal", max_memtable_entries=1000)
    store.put("session:42", {"model": "sonnet"}, ttl=300)
    store.get("session:42")          # -> dict, or None after 300 s
    store.delete("session:42")
    store.snapshot()                 # consistent point-in-time view
    store.close()
"""

from __future__ import annotations

import json
import logging
import os
import struct
import tempfile
import threading
import time
import zlib
from dataclasses import dataclass
from typing import Any, Dict, List, Optional, Tuple

__all__ = (
    "Entry",
    "Record",
    "MemTable",
    "WriteAheadLog",
    "KVStore",
    "KVEngineError",
    "WALCorruptionError",
    "SnapshotError",
)

LOG = logging.getLogger("kv_engine")

OP_PUT = 0
OP_DELETE = 1
_OP_NAMES = {OP_PUT: "put", OP_DELETE: "delete"}

_MAGIC = b"CWAL"
_VERSION = 1
# version, op, seq, key_len, value_len, expires_at  (network byte order, no padding)
_HEADER = struct.Struct(">BBIIId")
_CRC = struct.Struct(">I")
_SNAPSHOT_VERSION = 1


class KVEngineError(Exception):
    """Base error for kv_engine."""


class WALCorruptionError(KVEngineError):
    """A WAL record failed length/CRC validation (strict mode)."""


class SnapshotError(KVEngineError):
    """A snapshot file is unreadable or malformed."""


@dataclass(frozen=True)
class Entry:
    """One stored value. ``expires_at == 0.0`` means no TTL; ``tombstone`` marks a delete."""

    key: str
    value: Any
    expires_at: float = 0.0
    tombstone: bool = False

    def is_expired(self, now: float) -> bool:
        return self.expires_at > 0.0 and self.expires_at <= now


@dataclass(frozen=True)
class Record:
    """One decoded WAL record."""

    seq: int
    op: int
    key: str
    value: Any
    expires_at: float


def _encode_value(value: Any) -> bytes:
    try:
        return json.dumps(value, separators=(",", ":"), sort_keys=True).encode("utf-8")
    except (TypeError, ValueError) as exc:
        raise KVEngineError(f"value is not JSON-serializable: {exc}") from exc


def _atomic_write_json(path: str, payload: Dict[str, Any]) -> None:
    """Write JSON durably: temp file in the same directory, fsync, atomic rename."""
    directory = os.path.dirname(os.path.abspath(path)) or "."
    os.makedirs(directory, exist_ok=True)
    fd, tmp = tempfile.mkstemp(prefix=".snapshot-", suffix=".tmp", dir=directory)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as fh:
            json.dump(payload, fh, separators=(",", ":"), sort_keys=True)
            fh.flush()
            os.fsync(fh.fileno())
        os.replace(tmp, path)
    except BaseException:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        raise


class WriteAheadLog:
    """Append-only CRC32-framed record log. Thread-safe; fsync per record."""

    def __init__(self, filepath: str, *, fsync: bool = True, strict: bool = False) -> None:
        if not filepath:
            raise ValueError("filepath must be non-empty")
        self.filepath = os.path.abspath(filepath)
        self.fsync = bool(fsync)
        self.strict = bool(strict)
        self._lock = threading.RLock()
        directory = os.path.dirname(self.filepath)
        if directory:
            os.makedirs(directory, exist_ok=True)
        self._fh = open(self.filepath, "ab")

    # ---------------------------------------------------------------- writes

    def append(self, seq: int, op: int, key: str, value: Any, expires_at: float = 0.0) -> None:
        if op not in _OP_NAMES:
            raise ValueError(f"unknown op: {op!r}")
        if seq < 1:
            raise ValueError(f"seq must be >= 1, got {seq}")
        key_bytes = key.encode("utf-8")
        value_bytes = _encode_value(value)
        body = _HEADER.pack(_VERSION, op, seq, len(key_bytes), len(value_bytes), float(expires_at))
        body += key_bytes + value_bytes
        record = _MAGIC + body + _CRC.pack(zlib.crc32(body))
        with self._lock:
            self._fh.write(record)
            self._fh.flush()
            if self.fsync:
                os.fsync(self._fh.fileno())

    def clear(self) -> None:
        """Truncate the log to zero (only safe after the state it describes is snapshotted)."""
        with self._lock:
            self._fh.seek(0)
            self._fh.truncate(0)
            self._fh.flush()
            if self.fsync:
                os.fsync(self._fh.fileno())

    # ---------------------------------------------------------------- reads

    def recover(self) -> List[Record]:
        """Return every intact record in append order.

        Reading stops at the first invalid record; in non-strict mode the corrupt
        tail is truncated so later appends stay aligned with the log.
        """
        records: List[Record] = []
        with self._lock:
            if not os.path.exists(self.filepath):
                return records
            corrupt_at: Optional[int] = None
            with open(self.filepath, "rb") as fh:
                while True:
                    start = fh.tell()
                    magic = fh.read(len(_MAGIC))
                    if not magic:
                        break
                    if magic != _MAGIC:
                        corrupt_at = start
                        break
                    head = fh.read(_HEADER.size)
                    if len(head) != _HEADER.size:
                        corrupt_at = start
                        break
                    version, op, seq, key_len, value_len, expires_at = _HEADER.unpack(head)
                    if version != _VERSION or op not in _OP_NAMES:
                        corrupt_at = start
                        break
                    payload = fh.read(key_len + value_len)
                    if len(payload) != key_len + value_len:
                        corrupt_at = start
                        break
                    crc_bytes = fh.read(_CRC.size)
                    if len(crc_bytes) != _CRC.size:
                        corrupt_at = start
                        break
                    if zlib.crc32(head + payload) != _CRC.unpack(crc_bytes)[0]:
                        corrupt_at = start
                        break
                    try:
                        key = payload[:key_len].decode("utf-8")
                        value = json.loads(payload[key_len:]) if value_len else None
                    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
                        if self.strict:
                            raise WALCorruptionError(f"record at offset {start}: {exc}") from exc
                        corrupt_at = start
                        break
                    records.append(Record(seq, op, key, value, expires_at))

            if corrupt_at is not None:
                if self.strict:
                    raise WALCorruptionError(
                        f"corrupt WAL record at offset {corrupt_at} in {self.filepath}"
                    )
                LOG.warning(
                    "WAL %s: corrupt/torn record at offset %d, tail truncated",
                    self.filepath,
                    corrupt_at,
                )
                self._truncate_to(corrupt_at)
        return records

    def _truncate_to(self, offset: int) -> None:
        with open(self.filepath, "r+b") as fh:
            fh.truncate(offset)
            fh.flush()
            os.fsync(fh.fileno())

    def close(self) -> None:
        with self._lock:
            if not self._fh.closed:
                self._fh.flush()
                if self.fsync:
                    os.fsync(self._fh.fileno())
                self._fh.close()

    def __enter__(self) -> "WriteAheadLog":
        return self

    def __exit__(self, *_exc: Any) -> None:
        self.close()

    def __repr__(self) -> str:
        return f"WriteAheadLog({self.filepath!r}, fsync={self.fsync}, strict={self.strict})"


class MemTable:
    """Bounded in-memory write buffer: ``key -> Entry`` (tombstones included).

    ``max_entries == 0`` means unbounded. The optional ``lock`` lets the owning
    store share one critical section across WAL append + memtable apply.
    """

    def __init__(self, max_entries: int = 0, lock: Optional[threading.RLock] = None) -> None:
        if max_entries < 0:
            raise ValueError("max_entries must be >= 0")
        self.max_entries = int(max_entries)
        self._lock = lock if lock is not None else threading.RLock()
        self._entries: Dict[str, Entry] = {}

    def put(self, entry: Entry) -> None:
        with self._lock:
            self._entries[entry.key] = entry

    def get(self, key: str) -> Optional[Entry]:
        with self._lock:
            return self._entries.get(key)

    def delete(self, key: str) -> Optional[Entry]:
        with self._lock:
            return self._entries.pop(key, None)

    def clear(self) -> None:
        with self._lock:
            self._entries.clear()

    def entries(self) -> List[Entry]:
        """Copy of the buffer contents, taken atomically."""
        with self._lock:
            return list(self._entries.values())

    def live(self, now: Optional[float] = None) -> List[Entry]:
        now = time.time() if now is None else now
        return [e for e in self.entries() if not e.tombstone and not e.is_expired(now)]

    def is_full(self) -> bool:
        return self.max_entries > 0 and len(self) >= self.max_entries

    def __len__(self) -> int:
        with self._lock:
            return len(self._entries)

    def __contains__(self, key: str) -> bool:
        with self._lock:
            return key in self._entries

    def __repr__(self) -> str:
        return f"MemTable(entries={len(self)}, max_entries={self.max_entries})"


class KVStore:
    """WAL-backed, TTL-aware, thread-safe key-value store."""

    def __init__(
        self,
        wal_path: str,
        max_memtable_entries: int,
        *,
        snapshot_path: Optional[str] = None,
        fsync: bool = True,
        ttl_default: Optional[float] = None,
    ) -> None:
        if max_memtable_entries < 1:
            raise ValueError("max_memtable_entries must be >= 1")
        if ttl_default is not None and ttl_default <= 0:
            raise ValueError("ttl_default must be > 0")

        self._lock = threading.RLock()
        self._max_memtable_entries = int(max_memtable_entries)
        self._ttl_default = ttl_default
        self._wal = WriteAheadLog(wal_path, fsync=fsync)
        self._snapshot_path = snapshot_path or f"{self._wal.filepath}.snapshot"
        self._memtable = MemTable(self._max_memtable_entries, lock=self._lock)
        self._base: Dict[str, Entry] = {}
        self._seq = 0
        self._closed = False
        self._recover()

    # ---------------------------------------------------------------- recovery

    def _recover(self) -> None:
        with self._lock:
            base_seq, entries = self._load_snapshot()
            for entry in entries:
                self._base[entry.key] = entry

            last_seq = base_seq
            for record in self._wal.recover():
                last_seq = max(last_seq, record.seq)
                if record.seq <= base_seq:
                    continue  # already covered by the snapshot
                self._apply(record, self._base)
            self._seq = last_seq

    @staticmethod
    def _apply(record: Record, target: Dict[str, Entry]) -> None:
        if record.op == OP_DELETE:
            target.pop(record.key, None)
        else:
            target[record.key] = Entry(record.key, record.value, record.expires_at, False)

    def _load_snapshot(self) -> Tuple[int, List[Entry]]:
        if not os.path.exists(self._snapshot_path):
            return 0, []
        try:
            with open(self._snapshot_path, "r", encoding="utf-8") as fh:
                doc = json.load(fh)
        except (OSError, json.JSONDecodeError) as exc:
            raise SnapshotError(f"unreadable snapshot {self._snapshot_path}: {exc}") from exc
        if not isinstance(doc, dict) or doc.get("version") != _SNAPSHOT_VERSION:
            raise SnapshotError(f"unexpected snapshot format in {self._snapshot_path}")
        entries: List[Entry] = []
        for item in doc.get("entries", []):
            if not isinstance(item, dict) or not isinstance(item.get("k"), str):
                raise SnapshotError(f"malformed snapshot entry in {self._snapshot_path}")
            entries.append(
                Entry(
                    key=item["k"],
                    value=item.get("v"),
                    expires_at=float(item.get("e", 0.0)),
                    tombstone=bool(item.get("d", False)),
                )
            )
        return int(doc.get("seq", 0)), entries

    # ---------------------------------------------------------------- mutations

    def put(self, key: str, value: Any, ttl: Optional[float] = None) -> None:
        """Durably record then apply ``key = value`` atomically.

        ``ttl`` is seconds; ``None`` falls back to ``ttl_default``; ``ttl <= 0``
        means no expiry.
        """
        if not isinstance(key, str):
            raise TypeError(f"key must be str, got {type(key).__name__}")
        if ttl is None:
            ttl = self._ttl_default
        if ttl is not None and ttl <= 0:
            raise ValueError(f"ttl must be > 0, got {ttl}")
        self._check_open()
        expires_at = time.time() + ttl if ttl else 0.0
        with self._lock:
            seq = self._seq + 1
            self._wal.append(seq, OP_PUT, key, value, expires_at)
            self._seq = seq
            self._memtable.put(Entry(key, value, expires_at, False))
            if self._memtable.is_full():
                self._compact()

    def delete(self, key: str) -> bool:
        """Durably record then apply a delete. Returns True if a live key was removed."""
        if not isinstance(key, str):
            raise TypeError(f"key must be str, got {type(key).__name__}")
        self._check_open()
        with self._lock:
            existed = self._lookup(key) is not None
            seq = self._seq + 1
            self._wal.append(seq, OP_DELETE, key, None, 0.0)
            self._seq = seq
            self._memtable.put(Entry(key, None, 0.0, True))
            if self._memtable.is_full():
                self._compact()
            return existed

    # ---------------------------------------------------------------- reads

    def get(self, key: str, default: Any = None) -> Any:
        """Atomic read: memtable shadows the base, expired keys read as absent."""
        if not isinstance(key, str):
            raise TypeError(f"key must be str, got {type(key).__name__}")
        self._check_open()
        with self._lock:
            entry = self._lookup(key)
            if entry is None:
                return default
            if entry.is_expired(time.time()):
                self._drop_expired(key)
                return default
            return entry.value

    def exists(self, key: str) -> bool:
        self._check_open()
        with self._lock:
            entry = self._lookup(key)
            return entry is not None and not entry.is_expired(time.time())

    def _lookup(self, key: str) -> Optional[Entry]:
        """Caller must hold ``self._lock``."""
        entry = self._memtable.get(key)
        if entry is not None:
            return None if entry.tombstone else entry
        base = self._base.get(key)
        return None if base is None or base.tombstone else base

    def _drop_expired(self, key: str) -> None:
        self._memtable.delete(key)
        self._base.pop(key, None)

    def expire(self) -> int:
        """Drop every expired entry; returns how many were removed."""
        self._check_open()
        now = time.time()
        with self._lock:
            removed = 0
            for entry in self._memtable.entries():
                if entry.is_expired(now):
                    self._memtable.delete(entry.key)
                    removed += 1
            for entry in list(self._base.values()):
                if entry.is_expired(now):
                    self._base.pop(entry.key, None)
                    removed += 1
            return removed

    def keys(self) -> List[str]:
        """Live (non-expired, non-tombstoned) keys, sorted."""
        self._check_open()
        now = time.time()
        with self._lock:
            merged = dict(self._base)
            for entry in self._memtable.entries():
                if entry.tombstone:
                    merged.pop(entry.key, None)
                else:
                    merged[entry.key] = entry
            return sorted(
                key for key, entry in merged.items() if not entry.is_expired(now)
            )

    def snapshot(self) -> Dict[str, Any]:
        """Consistent point-in-time view of live entries (key -> value)."""
        self._check_open()
        now = time.time()
        with self._lock:
            merged = dict(self._base)
            for entry in self._memtable.entries():
                if entry.tombstone:
                    merged.pop(entry.key, None)
                else:
                    merged[entry.key] = entry
            return {
                key: entry.value
                for key, entry in merged.items()
                if not entry.is_expired(now)
            }

    def write_snapshot(self, dest: Optional[str] = None) -> str:
        """Persist the current live view atomically; returns the path written."""
        self._check_open()
        now = time.time()
        path = dest or self._snapshot_path
        with self._lock:
            merged = dict(self._base)
            for entry in self._memtable.entries():
                if entry.tombstone:
                    merged.pop(entry.key, None)
                else:
                    merged[entry.key] = entry
            doc = {
                "version": _SNAPSHOT_VERSION,
                "seq": self._seq,
                "created": now,
                "entries": [
                    {
                        "k": entry.key,
                        "v": entry.value,
                        "e": entry.expires_at,
                        "d": entry.tombstone,
                    }
                    for entry in sorted(merged.values(), key=lambda e: e.key)
                    if not entry.is_expired(now)
                ],
            }
            _atomic_write_json(path, doc)
            return path

    # ---------------------------------------------------------------- compaction

    def flush(self) -> None:
        """Force a compaction: merge memtable into base, snapshot, truncate WAL."""
        self._check_open()
        with self._lock:
            self._compact()

    def _compact(self) -> None:
        """Caller must hold ``self._lock``. Snapshot first, then truncate the WAL."""
        now = time.time()
        merged = dict(self._base)
        for entry in self._memtable.entries():
            if entry.tombstone or entry.is_expired(now):
                merged.pop(entry.key, None)
            else:
                merged[entry.key] = entry

        doc = {
            "version": _SNAPSHOT_VERSION,
            "seq": self._seq,
            "created": now,
            "entries": [
                {"k": e.key, "v": e.value, "e": e.expires_at, "d": e.tombstone}
                for e in sorted(merged.values(), key=lambda e: e.key)
            ],
        }
        _atomic_write_json(self._snapshot_path, doc)

        self._base = merged
        self._wal.clear()
        self._memtable.clear()

    # ---------------------------------------------------------------- misc

    def stats(self) -> Dict[str, Any]:
        with self._lock:
            return {
                "wal_path": self._wal.filepath,
                "snapshot_path": self._snapshot_path,
                "max_memtable_entries": self._max_memtable_entries,
                "memtable_entries": len(self._memtable),
                "base_entries": len(self._base),
                "live_entries": len(self.keys()),
                "last_seq": self._seq,
                "closed": self._closed,
            }

    def __len__(self) -> int:
        self._check_open()
        return len(self.keys())

    def __contains__(self, key: str) -> bool:
        return self.exists(key)

    def close(self) -> None:
        with self._lock:
            if self._closed:
                return
            self._closed = True
            self._wal.close()

    def _check_open(self) -> None:
        if self._closed:
            raise KVEngineError("store is closed")

    def __enter__(self) -> "KVStore":
        return self

    def __exit__(self, *_exc: Any) -> None:
        self.close()

    def __repr__(self) -> str:
        return (
            f"KVStore(wal_path={self._wal.filepath!r}, "
            f"max_memtable_entries={self._max_memtable_entries}, "
            f"memtable={len(self._memtable)}, base={len(self._base)})"
        )
