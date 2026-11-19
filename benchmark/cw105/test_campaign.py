import unittest

from budget import Budget, BudgetExceeded
from campaign import execute_plan, serving
from report import summarize
from plan import proposal
from schedule import schedule


class CampaignTest(unittest.TestCase):
    @staticmethod
    def report_plan(rows):
        plan = proposal()
        plan["runs"] = list(rows)
        return plan

    def test_incorrect_submission_does_not_stop_matched_arms(self):
        plan = list(schedule())[:3]
        executable = [dict(row, request_cap=2, budget_seconds=10) for row in plan]
        budget = Budget(campaign_limit=6, seconds=30, clock=lambda: 0)
        started = []

        def execute(row):
            started.append(row["run_id"])
            budget.admit(row["run_id"])
            return dict(row, status="complete", correct=row["mode"] != "M0")

        execution = execute_plan(executable, budget, execute)
        self.assertEqual(started, [row["run_id"] for row in plan])
        self.assertIsNone(execution.stop_reason)
        self.assertEqual(execution.unstarted_runs, 0)
        report = summarize(execution.results, self.report_plan(plan))
        self.assertEqual(report["arms"]["M0"]["correct"], 0)
        self.assertEqual(report["arms"]["M1"]["missing"], 0)
        self.assertEqual(report["arms"]["M2"]["missing"], 0)

    def test_safety_stop_records_unstarted_comparisons(self):
        plan = list(schedule())[:3]
        executable = [dict(row, request_cap=2, budget_seconds=10) for row in plan]
        budget = Budget(campaign_limit=6, seconds=30, clock=lambda: 0)
        started = []

        def execute(row):
            started.append(row["run_id"])
            return dict(row, status="incomplete", correct=None,
                        campaign_stop_reason="incomplete_provider_telemetry")

        execution = execute_plan(executable, budget, execute)
        self.assertEqual(started, [plan[0]["run_id"]])
        self.assertEqual(execution.stop_reason, "incomplete_provider_telemetry")
        self.assertEqual(execution.stopped_run_id, plan[0]["run_id"])
        self.assertEqual(execution.unstarted_runs, 2)
        with self.assertRaisesRegex(BudgetExceeded, "incomplete_provider_telemetry"):
            budget.admit(plan[0]["run_id"])

    def test_unhandled_attempt_exception_stops_and_counts_current_run_unstarted(self):
        plan = list(schedule())[:3]
        executable = [dict(row, request_cap=1, budget_seconds=10) for row in plan]
        budget = Budget(campaign_limit=3, seconds=30, clock=lambda: 0)
        started = []

        def execute(row):
            started.append(row["run_id"])
            raise OSError("evidence write failed")

        execution = execute_plan(executable, budget, execute)
        self.assertEqual(started, [plan[0]["run_id"]])
        self.assertEqual(execution.stop_reason, "attempt_execution_exception:OSError")
        self.assertEqual(execution.stopped_run_id, plan[0]["run_id"])
        self.assertEqual(execution.unstarted_runs, 2)

    def test_completed_agent_exit_failure_does_not_stop_later_runs(self):
        plan = list(schedule())[:3]
        executable = [dict(row, request_cap=1, budget_seconds=10) for row in plan]
        budget = Budget(campaign_limit=3, seconds=30, clock=lambda: 0)
        started = []

        def execute(row):
            started.append(row["run_id"])
            return dict(row, status="failed" if row["mode"] == "M0" else "complete",
                        correct=None if row["mode"] == "M0" else True,
                        campaign_stop_reason=None)

        execution = execute_plan(executable, budget, execute)
        self.assertEqual(started, [row["run_id"] for row in plan])
        self.assertIsNone(execution.stop_reason)
        self.assertEqual(summarize(execution.results, self.report_plan(plan))
                         ["arms"]["M0"]["agent_failures"], 1)

    def test_expired_campaign_launches_nothing(self):
        clock = [0]
        budget = Budget(campaign_limit=1, seconds=10, clock=lambda: clock[0])
        clock[0] = 10
        execution = execute_plan(
            [{"run_id": "expired", "request_cap": 1, "budget_seconds": 5}],
            budget, lambda _: self.fail("expired attempt launched"),
        )
        self.assertEqual(execution.results, ())
        self.assertIn("deadline", execution.stop_reason)
        self.assertEqual(execution.unstarted_runs, 1)

    def test_stalled_relay_shutdown_is_reported_and_bounded(self):
        import threading
        from process import ProcessStillRunning

        release = threading.Event()

        class StalledServer:
            server_port = 1234

            def __enter__(self):
                return self

            def __exit__(self, *_):
                release.set()

            def serve_forever(self, _interval):
                release.wait(2)

            def shutdown(self):
                release.wait(2)

        closed = []
        with self.assertRaisesRegex(ProcessStillRunning, "Relay shutdown"):
            with serving(StalledServer(), on_exit=lambda: closed.append(True), shutdown_timeout=0.05):
                pass
        self.assertEqual(closed, [True])
