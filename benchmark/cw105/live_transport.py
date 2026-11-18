"""Construct the pinned live transport after execution preflight and approval."""
from datetime import datetime, timezone
from http.client import HTTPConnection
from urllib.parse import urlsplit

from authorization import ENDPOINT, claim
from budget import BudgetExceeded
from workspace import git


def authorize_transport(document, plan, registry, *, checkout, merged_revision):
    revision = plan.get("benchmark_revision")
    if revision is None or revision != merged_revision:
        raise ValueError("Execution plan must match the reviewed merge commit")
    if git(checkout, "rev-parse", "HEAD").decode().strip() != revision:
        raise ValueError("Checkout does not match the reviewed merge commit")
    if git(checkout, "status", "--porcelain", "--untracked-files=all"):
        raise ValueError("Benchmark checkout must be clean")
    claim(document, plan, registry)
    expires = datetime.fromisoformat(document["expires_at"])
    endpoint = urlsplit(ENDPOINT)

    def connect(timeout):
        remaining = (expires - datetime.now(timezone.utc)).total_seconds()
        if remaining <= 0:
            raise BudgetExceeded("Live authorization expired")
        return HTTPConnection(endpoint.hostname, endpoint.port, timeout=min(timeout, remaining))

    return connect
