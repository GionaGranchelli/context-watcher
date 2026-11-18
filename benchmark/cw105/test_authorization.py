import tempfile
import unittest
from datetime import datetime, timezone
from pathlib import Path

from authorization import claim, plan_digest


class AuthorizationTest(unittest.TestCase):
    now = datetime(2026, 11, 19, 12, tzinfo=timezone.utc)
    plan = {"revision": "fixture", "requests": 10, "endpoint": "loopback"}

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
                claim(self.document(), dict(self.plan, requests=11), registry, now=self.now)
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
