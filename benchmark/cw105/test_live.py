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

    def test_guarded_scored_path_stops_on_unchanged_checkout(self):
        import json
        import os
        import tempfile
        from http.client import HTTPConnection
        from http.server import HTTPServer
        from pathlib import Path

        from campaign import serving
        from offline import Mock

        if not all(os.environ.get(key) for key in ("CW105_KANBAN", "CW105_OMP", "CW105_CW")):
            self.skipTest("set CW105_KANBAN, CW105_OMP and CW105_CW")
        plan = dict(proposal(), status="ready_for_authorization",
                    provider_cost_cap_nanodollars=0,
                    pricing={"evidence": "synthetic loopback",
                             "input_nanodollars_per_token": 0,
                             "output_nanodollars_per_token": 0},
                    local_resource_cap={"gpu_watts": 100, "junction_c": 80,
                                        "sample_max_age_seconds": 5})
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "evidence"
            with serving(HTTPServer(("127.0.0.1", 0), Mock)) as port:
                connect = lambda timeout: HTTPConnection("127.0.0.1", port, timeout=timeout)
                with patch("live.read_sample", return_value=([], 0)), patch(
                    "live.authorize_transport", return_value=connect
                ) as transport:
                    passed = run_live(
                        plan, {}, checkout=None, merged_revision=None,
                        repository=Path(os.environ["CW105_KANBAN"]), output=output,
                        omp=Path(os.environ["CW105_OMP"]), cw=Path(os.environ["CW105_CW"]),
                    )
                    transport.assert_called_once()
            self.assertFalse(passed)
            manifests = list(output.glob("*/manifest.json"))
            self.assertEqual(len(manifests), 1)
            result = json.loads(manifests[0].read_text())
            self.assertTrue(result["telemetry_complete"], result)
            self.assertEqual(result["agent"]["returncode"], 0)
            self.assertNotEqual(result["correct"], True)
            self.assertIn("404", result["error"])
            report = json.loads((output / "report.json").read_text())
            self.assertEqual(report["arms"]["M1"]["missing"], 18)
