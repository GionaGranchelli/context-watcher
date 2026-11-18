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
