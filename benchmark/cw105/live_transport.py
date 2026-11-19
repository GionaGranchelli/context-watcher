"""Construct the pinned live transport after execution preflight and approval."""
import json
import subprocess
from datetime import datetime, timezone
from http.client import HTTPConnection
from urllib.parse import urlsplit

from authorization import ENDPOINT, claim
from budget import BudgetExceeded
from workspace import git


def verify_merged_pr(revision, *, remaining=None):
    result = subprocess.run(
        ["gh", "pr", "view", "53", "--repo", "GionaGranchelli/context-watcher",
         "--json", "state,baseRefName,mergeCommit"],
        capture_output=True, text=True, check=True,
        timeout=min(15, remaining()) if remaining else 15,
    )
    pr = json.loads(result.stdout)
    if (pr.get("state"), pr.get("baseRefName"), (pr.get("mergeCommit") or {}).get("oid")) != (
        "MERGED", "main", revision,
    ):
        raise ValueError("PR #53 must be merged into main at the approved revision")


def authorize_transport(document, plan, registry, *, checkout, merged_revision, remaining=None):
    revision = plan.get("benchmark_revision")
    if revision is None or revision != merged_revision:
        raise ValueError("Execution plan must match the reviewed merge commit")
    if git(checkout, "rev-parse", "HEAD", remaining=remaining).decode().strip() != revision:
        raise ValueError("Checkout does not match the reviewed merge commit")
    if git(checkout, "status", "--porcelain", "--untracked-files=all", remaining=remaining):
        raise ValueError("Benchmark checkout must be clean")
    verify_merged_pr(revision, remaining=remaining)
    if remaining is not None:
        remaining()
    claim(document, plan, registry)
    expires = datetime.fromisoformat(document["expires_at"])
    endpoint = urlsplit(ENDPOINT)

    def connect(timeout):
        remaining = (expires - datetime.now(timezone.utc)).total_seconds()
        if remaining <= 0:
            raise BudgetExceeded("Live authorization expired")
        return HTTPConnection(endpoint.hostname, endpoint.port, timeout=min(timeout, remaining))

    return connect
