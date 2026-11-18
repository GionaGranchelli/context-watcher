import tempfile
import unittest
from datetime import datetime, timezone
from pathlib import Path

from authorization import ENDPOINT, MODEL, PROVIDER, claim, plan_digest


class AuthorizationTest(unittest.TestCase):
    now = datetime(2026, 11, 19, 12, tzinfo=timezone.utc)
    plan = {
        "benchmark_revision": "a" * 40, "max_requests": 10, "max_wall_seconds": 60,
        "provider": PROVIDER, "endpoint": ENDPOINT, "model": MODEL, "status": "ready_for_authorization",
        "provider_cost_cap_nanodollars": 0,
        "local_resource_cap": {"gpu_watts": 100, "junction_c": 80,
                               "sample_max_age_seconds": 5},
    }

    def document(self):
        return {
            "authorization_id": "cw105-fixture-0001",
            "approved_by": "operator",
            "approval_reference": "synthetic fixture",
            "plan_sha256": plan_digest(self.plan),
            "issued_at": "2026-11-19T11:00:00Z",
            "expires_at": "2026-11-19T13:00:00Z",
        }

    def test_bound_plan_and_single_use(self):
        with tempfile.TemporaryDirectory() as directory:
            registry = Path(directory) / "claims"
            with self.assertRaisesRegex(ValueError, "match"):
                claim(self.document(), dict(self.plan, max_requests=11), registry, now=self.now)
            self.assertFalse(registry.exists())
            claim(self.document(), self.plan, registry, now=self.now)
            with self.assertRaisesRegex(ValueError, "consumed"):
                claim(self.document(), self.plan, registry, now=self.now)

    def test_missing_and_expired_authorizations_fail_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            registry = Path(directory) / "claims"
            for doc in ({}, dict(self.document(), expires_at="2026-11-19T12:00:00Z"),
                        dict(self.document(), issued_at="2026-11-19T11:00:00")):
                with self.assertRaises(ValueError):
                    claim(doc, self.plan, registry, now=self.now)
            self.assertFalse(registry.exists())

    def test_incomplete_proposal_cannot_consume_authorization(self):
        with tempfile.TemporaryDirectory() as directory:
            registry = Path(directory) / "claims"
            for plan in (dict(self.plan, status="not_ready_for_authorization"),
                         dict(self.plan, provider_cost_cap_nanodollars=None),
                         dict(self.plan, local_resource_cap={}),
                         dict(self.plan, model="different"),
                         dict(self.plan, endpoint="http://127.0.0.1:1234/v1"),
                         dict(self.plan, benchmark_revision="a" * 7)):
                doc = dict(self.document(), plan_sha256=plan_digest(plan))
                with self.assertRaises(ValueError):
                    claim(doc, plan, registry, now=self.now)
            self.assertFalse(registry.exists())

    def test_authorization_must_cover_campaign_wall_time(self):
        with tempfile.TemporaryDirectory() as directory:
            registry = Path(directory) / "claims"
            plan = dict(self.plan, max_wall_seconds=108600)
            doc = dict(self.document(), plan_sha256=plan_digest(plan))
            with self.assertRaisesRegex(ValueError, "entire campaign"):
                claim(doc, plan, registry, now=self.now)
            self.assertFalse(registry.exists())
            doc["expires_at"] = "2026-11-20T19:00:00Z"
            claim(doc, plan, registry, now=self.now)
