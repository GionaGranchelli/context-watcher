import unittest
from unittest.mock import patch

from live import binary_identities, run_live
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
        pins = binary_identities(Path(os.environ["CW105_OMP"]), Path(os.environ["CW105_CW"]))
        plan = dict(proposal(), binaries_sha256=pins, status="ready_for_authorization",
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
                with patch("live.check_loaded_model", return_value={}), patch(
                    "live.read_sample", return_value=([], 0)
                ), patch(
                    "live.authorize_transport", return_value=connect
                ) as transport:
                    passed = run_live(
                        plan, {}, checkout=None, merged_revision=None,
                        repository=Path(os.environ["CW105_KANBAN"]), output=output,
                        omp=Path(os.environ["CW105_OMP"]), cw=Path(os.environ["CW105_CW"]),
                    )
                    transport.assert_called_once()
            self.assertFalse(passed)
            smoke = json.loads((output / "provider-smoke/manifest.json").read_text())
            self.assertTrue(smoke["telemetry_complete"])
            self.assertIsNone(smoke["correct"])
            manifests = list(output.glob("health-contract-*/manifest.json"))
            self.assertEqual(len(manifests), 1)
            result = json.loads(manifests[0].read_text())
            self.assertTrue(result["telemetry_complete"], result)
            self.assertEqual(result["agent"]["returncode"], 0)
            self.assertEqual(result["status"], "complete")
            self.assertIs(result["correct"], False)
            self.assertIn("404", (manifests[0].parent / "api-oracle.log").read_text())
            report = json.loads((output / "report.json").read_text())
            self.assertEqual(report["arms"]["M1"]["missing"], 18)

    @patch("live.authorize_transport")
    @patch("live.binary_identities", return_value={"omp": "actual", "cw": "actual"})
    def test_binary_mismatch_stops_before_authorization(self, identities, transport):
        plan = dict(proposal(), status="ready_for_authorization",
                    binaries_sha256={"omp": "expected", "cw": "actual"})
        with self.assertRaisesRegex(ValueError, "approved hashes"):
            run_live(plan, {}, checkout=None, merged_revision=None,
                     repository=None, output=None, omp=None, cw=None)
        transport.assert_not_called()
