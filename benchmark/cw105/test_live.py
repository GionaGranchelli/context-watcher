import unittest
from unittest.mock import patch

from live import run_live
from plan import proposal


class LiveEntryTest(unittest.TestCase):
    @patch("live.read_sample")
    @patch("live.authorize_transport")
    def test_unready_proposal_cannot_reach_telemetry_or_transport(self, transport, sample):
        with self.assertRaisesRegex(ValueError, "readiness review"):
            run_live(proposal(), {}, checkout=None, merged_revision=None,
                     repository=None, output=None, omp=None, cw=None)
        transport.assert_not_called()
        sample.assert_not_called()
