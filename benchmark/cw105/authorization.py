"""Validate and consume an operator authorization for a frozen campaign."""
import hashlib
import json
import os
import re
from datetime import datetime, timedelta, timezone


def plan_digest(plan):
    encoded = json.dumps(plan, sort_keys=True, separators=(",", ":"), allow_nan=False)
    return hashlib.sha256(encoded.encode()).hexdigest()


def claim(document, plan, registry, *, now=None):
    """Consume only after all preflight checks, before opening provider access."""
    now = datetime.now(timezone.utc) if now is None else now
    if not isinstance(document, dict) or set(document) != {
        "authorization_id", "approved_by", "approval_reference",
        "plan_sha256", "issued_at", "expires_at",
    }:
        raise ValueError("Invalid authorization document")
    identifier = document["authorization_id"]
    if not isinstance(identifier, str) or not re.fullmatch(r"[A-Za-z0-9_-]{12,128}", identifier):
        raise ValueError("Invalid authorization ID")
    for field in ("approved_by", "approval_reference"):
        value = document[field]
        if not isinstance(value, str) or not value.strip() or len(value) > 512:
            raise ValueError(f"Invalid {field}")
    if document["plan_sha256"] != plan_digest(plan):
        raise ValueError("Authorization does not match the frozen execution plan")
    try:
        issued = datetime.fromisoformat(document["issued_at"])
        expires = datetime.fromisoformat(document["expires_at"])
        valid = issued <= now < expires <= issued + timedelta(hours=24)
    except (TypeError, ValueError, OverflowError):
        valid = False
    if not valid:
        raise ValueError("Authorization requires a current time window of at most 24 hours")

    registry.mkdir(mode=0o700, exist_ok=True)
    info = registry.lstat()
    if registry.is_symlink() or not registry.is_dir() or info.st_uid != os.getuid() or info.st_mode & 0o077:
        raise ValueError("Authorization registry must be a private directory owned by the runner")
    marker = registry / f"{identifier}.json"
    try:
        fd = os.open(marker, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    except FileExistsError:
        raise ValueError("Authorization already consumed") from None
    with os.fdopen(fd, "w", encoding="utf-8") as output:
        json.dump(document, output)
        output.flush()
        os.fsync(output.fileno())
    directory = os.open(registry, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)
