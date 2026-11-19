import unittest
from unittest.mock import patch

from live_transport import authorize_transport


class LiveTransportTest(unittest.TestCase):
    @patch("live_transport.claim")
    @patch("live_transport.HTTPConnection")
    def test_no_connection_when_revision_does_not_match(self, connect, claim):
        with self.assertRaisesRegex(ValueError, "reviewed merge commit"):
            authorize_transport({}, {"benchmark_revision": "a" * 40}, None,
                                checkout=None, merged_revision="b" * 40)
        claim.assert_not_called()
        connect.assert_not_called()

    @patch("live_transport.git", side_effect=[b"a" * 40 + b"\n", b" M harness.py\n"])
    @patch("live_transport.claim")
    @patch("live_transport.HTTPConnection")
    def test_dirty_checkout_cannot_consume_authorization(self, connect, claim, git):
        with self.assertRaisesRegex(ValueError, "clean"):
            authorize_transport({}, {"benchmark_revision": "a" * 40}, None,
                                checkout=None, merged_revision="a" * 40)
        claim.assert_not_called()
        connect.assert_not_called()

    @patch("live_transport.git", side_effect=[b"a" * 40 + b"\n", b""])
    @patch("live_transport.subprocess.run")
    @patch("live_transport.claim")
    def test_open_pr_cannot_consume_authorization(self, claim, query, git):
        query.return_value.stdout = '{"state":"OPEN","baseRefName":"main","mergeCommit":null}'
        with self.assertRaisesRegex(ValueError, "must be merged"):
            authorize_transport({}, {"benchmark_revision": "a" * 40}, None,
                                checkout=None, merged_revision="a" * 40)
        claim.assert_not_called()

    @patch("live_transport.git", side_effect=[b"a" * 40 + b"\n", b""])
    @patch("live_transport.verify_merged_pr")
    @patch("live_transport.claim")
    def test_deadline_after_merge_check_does_not_consume_authorization(self, claim, merged, git):
        from budget import BudgetExceeded

        def expired():
            raise BudgetExceeded("Campaign deadline exceeded")

        with self.assertRaisesRegex(BudgetExceeded, "deadline"):
            authorize_transport({}, {"benchmark_revision": "a" * 40}, None,
                                checkout=None, merged_revision="a" * 40, remaining=expired)
        merged.assert_called_once()
        claim.assert_not_called()

    @patch("live_transport.git", side_effect=[b"a" * 40 + b"\n", b""])
    @patch("live_transport.verify_merged_pr")
    @patch("live_transport.HTTPConnection")
    def test_transport_requires_durable_claim_and_pins_destination(self, connect, merged, git):
        from datetime import datetime, timedelta, timezone
        from pathlib import Path
        from tempfile import TemporaryDirectory

        from authorization import ENDPOINT, MODEL, PROVIDER, plan_digest

        now = datetime.now(timezone.utc)
        plan = {
            "status": "ready_for_authorization", "benchmark_revision": "a" * 40,
            "provider": PROVIDER, "endpoint": ENDPOINT, "model": MODEL,
            "max_requests": 1, "max_admission_window_seconds": 60,
            "provider_cost_cap_nanodollars": 0,
            "local_resource_cap": {"gpu_watts": 707, "junction_c": 80,
                                   "sample_max_age_seconds": 5},
        }
        document = {
            "authorization_id": "cw105-synthetic-authorization",
            "approved_by": "test fixture", "approval_reference": "offline only",
            "plan_sha256": plan_digest(plan), "issued_at": now.isoformat(),
            "expires_at": (now + timedelta(minutes=10)).isoformat(),
        }
        with TemporaryDirectory() as directory:
            registry = Path(directory) / "claims"
            factory = authorize_transport(document, plan, registry,
                                          checkout=None, merged_revision="a" * 40)
            merged.assert_called_once_with("a" * 40, remaining=None)
            self.assertTrue((registry / "cw105-synthetic-authorization.json").is_file())
            connect.assert_not_called()
            self.assertIs(factory(2), connect.return_value)
            connect.assert_called_once_with("192.168.1.156", 1234, timeout=2)
