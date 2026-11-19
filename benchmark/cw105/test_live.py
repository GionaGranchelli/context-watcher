import unittest
from unittest.mock import patch

from live import binary_identities, run_live, score
from plan import proposal


class LiveEntryTest(unittest.TestCase):
    def test_resource_guard_cancels_verification_and_latches_fault(self):
        import tempfile
        from pathlib import Path
        from budget import Budget, BudgetExceeded

        budget = Budget(campaign_limit=1, seconds=10)

        def reject():
            raise BudgetExceeded("GPU temperature limit")

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with self.assertRaisesRegex(RuntimeError, "GPU temperature limit"):
                score(root, root, "health-contract", budget, reject)
            self.assertTrue((root / "verifier.log").exists())
        with self.assertRaisesRegex(BudgetExceeded, "GPU temperature limit"):
            budget.campaign_remaining()

    def test_expired_preflight_does_not_consume_authorization(self):
        import tempfile
        from pathlib import Path
        from budget import Budget, BudgetExceeded

        now = [0]
        plan = dict(proposal(), status="ready_for_authorization",
                    binaries_sha256={"omp": "fixture", "cw": "fixture"})
        budget = Budget(campaign_limit=plan["max_requests"],
                        seconds=plan["max_admission_window_seconds"], clock=lambda: now[0])

        def inventory(*_, **_kwargs):
            now[0] = plan["max_admission_window_seconds"]
            return {}

        with tempfile.TemporaryDirectory() as directory, \
             patch("live.Budget", return_value=budget), \
             patch("live.binary_identities", return_value=plan["binaries_sha256"]), \
             patch("live.check_loaded_model", side_effect=inventory), \
             patch("live.read_sample") as sample, \
             patch("live.authorize_transport") as transport:
            with self.assertRaisesRegex(BudgetExceeded, "deadline"):
                run_live(plan, {}, checkout=None, merged_revision=None, repository=None,
                         output=Path(directory) / "evidence", omp=None, cw=None)
            sample.assert_not_called()
            transport.assert_not_called()

    @patch("live.read_sample")
    @patch("live.authorize_transport")
    def test_unready_proposal_cannot_reach_telemetry_or_transport(self, transport, sample):
        with self.assertRaisesRegex(ValueError, "readiness review"):
            run_live(proposal(), {}, checkout=None, merged_revision=None,
                     repository=None, output=None, omp=None, cw=None)
        transport.assert_not_called()
        sample.assert_not_called()

    def test_pilot_cannot_be_expanded_after_plan_creation(self):
        plan = dict(proposal("pilot"), status="ready_for_authorization")
        plan["runs"] = proposal()["runs"]
        with self.assertRaisesRegex(ValueError, "frozen runs"):
            run_live(plan, {}, checkout=None, merged_revision=None,
                     repository=None, output=None, omp=None, cw=None)

    def test_guarded_scored_path_continues_after_incorrect_answer(self):
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
        plan = dict(proposal("pilot"), binaries_sha256=pins, status="ready_for_authorization")
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "evidence"
            with serving(HTTPServer(("127.0.0.1", 0), Mock)) as port:
                connect = lambda timeout: HTTPConnection("127.0.0.1", port, timeout=timeout)
                with patch("live.score", return_value=False) as score, patch(
                    "live.check_loaded_model", return_value={}), patch(
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
            readings = (output / "gpu-samples.jsonl").read_text().splitlines()
            self.assertGreaterEqual(len(readings), 3)
            self.assertEqual(json.loads(readings[0])["watts"], 0)
            self.assertIsNone(smoke["correct"])
            manifests = list(output.glob("board-query-1-*/manifest.json"))
            self.assertEqual(len(manifests), 3)
            self.assertEqual(score.call_count, 3)
            result = json.loads(manifests[0].read_text())
            self.assertTrue(result["telemetry_complete"], result)
            self.assertEqual(result["agent"]["returncode"], 0)
            self.assertEqual(result["status"], "complete")
            self.assertIs(result["correct"], False)
            report = json.loads((output / "report.json").read_text())
            self.assertEqual(report["campaign_execution"]["unstarted_runs"], 0)
            self.assertEqual(report["arms"]["M0"]["missing"], 0)
            self.assertEqual(report["arms"]["M1"]["missing"], 0)
            self.assertEqual(report["arms"]["M2"]["missing"], 0)
            self.assertEqual(report["recommendation"]["label"], "ITERATE")

    @patch("live.authorize_transport")
    @patch("live.binary_identities", return_value={"omp": "actual", "cw": "actual"})
    def test_binary_mismatch_stops_before_authorization(self, identities, transport):
        plan = dict(proposal(), status="ready_for_authorization",
                    binaries_sha256={"omp": "expected", "cw": "actual"})
        with self.assertRaisesRegex(ValueError, "approved hashes"):
            run_live(plan, {}, checkout=None, merged_revision=None,
                     repository=None, output=None, omp=None, cw=None)
        transport.assert_not_called()
