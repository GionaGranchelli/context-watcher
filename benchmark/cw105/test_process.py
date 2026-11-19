import os
from pathlib import Path
import sys
import tempfile
import unittest

from process import ProcessStillRunning, run, stop_group
from budget import BudgetExceeded


class ProcessTest(unittest.TestCase):
    def test_timeout_preserves_partial_evidence(self):
        with tempfile.TemporaryDirectory() as root:
            log = Path(root) / "agent.log"
            result = run(
                [sys.executable, "-u", "-c", "print('partial'); import time; time.sleep(30)"],
                cwd=root, env=os.environ, log=log, timeout=0.5,
            )
            self.assertTrue(result.timed_out)
            self.assertLess(result.returncode, 0)
            self.assertEqual(log.read_text(), "partial\n")
            self.assertEqual(log.stat().st_mode & 0o777, 0o600)

    def test_preserves_failure_and_refuses_to_overwrite_evidence(self):
        with tempfile.TemporaryDirectory() as root:
            kwargs = dict(cwd=root, env=os.environ, log=Path(root) / "oracle.log", timeout=3)
            command = [sys.executable, "-c", "raise SystemExit(7)"]
            result = run(command, **kwargs)
            self.assertEqual(result.returncode, 7)
            self.assertFalse(result.timed_out)
            with self.assertRaises(FileExistsError):
                run(command, **kwargs)

    def test_guard_cancels_running_process_and_keeps_partial_log(self):
        polls = 0

        def guard():
            nonlocal polls
            polls += 1
            if polls == 3:
                raise BudgetExceeded("GPU temperature limit")

        with tempfile.TemporaryDirectory() as root:
            log = Path(root) / "agent.log"
            result = run(
                [sys.executable, "-u", "-c", "print('partial'); import time; time.sleep(30)"],
                cwd=root, env=os.environ, log=log, timeout=10, guard=guard,
            )
            self.assertEqual(result.cancelled_reason, "GPU temperature limit")
            self.assertFalse(result.timed_out)
            self.assertLess(result.returncode, 0)
            self.assertEqual(log.read_text(), "partial\n")

    def test_expired_guard_does_not_start_process(self):
        def guard():
            raise BudgetExceeded("Campaign deadline exceeded")

        with tempfile.TemporaryDirectory() as root:
            marker = Path(root) / "started"
            result = run(
                [sys.executable, "-c", "from pathlib import Path; Path('started').touch()"],
                cwd=root, env=os.environ, log=Path(root) / "agent.log", timeout=5, guard=guard,
            )
            self.assertEqual(result.cancelled_reason, "Campaign deadline exceeded")
            self.assertFalse(marker.exists())
            self.assertTrue((Path(root) / "agent.log").is_file())

    def test_unresponsive_killed_process_has_bounded_wait(self):
        import subprocess
        from unittest.mock import Mock, patch

        process = Mock(pid=12345)
        process.wait.side_effect = subprocess.TimeoutExpired("fake", 2)
        with patch("process.os.killpg") as kill:
            with self.assertRaisesRegex(ProcessStillRunning, "did not exit"):
                stop_group(process)
        kill.assert_called_once()
        process.wait.assert_called_once_with(timeout=2)
