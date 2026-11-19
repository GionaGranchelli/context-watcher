import json
import os
import sys
import tempfile
import time
import unittest
from contextlib import contextmanager
from pathlib import Path

from attempt import attempt
from budget import BudgetExceeded
from workspace import git


class AttemptTest(unittest.TestCase):
    def test_verifier_outputs_are_excluded_from_agent_patch(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            repo = root / "repo"
            repo.mkdir()
            git(repo, "init")
            git(repo, "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                "commit", "--allow-empty", "-m", "fixture")
            revision = git(repo, "rev-parse", "HEAD").decode().strip()

            @contextmanager
            def launch(worktree, evidence):
                yield [sys.executable, "-c", "open('answer','w').write('candidate')"], os.environ

            def verify(worktree, evidence):
                (worktree / "verifier-output").write_text("transient")
                return False

            evidence = root / "attempt"
            result = attempt({"revision": revision, "run_id": "fixture"},
                             repo, evidence, launch, verify, seconds=5)
            self.assertEqual(result["status"], "complete")
            self.assertIs(result["correct"], False)
            patch = (evidence / "final.patch").read_bytes()
            self.assertIn(b"+candidate", patch)
            self.assertNotIn(b"verifier-output", patch)

    def test_failed_process_keeps_patch_and_terminal_manifest(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            repo = root / "repo"
            repo.mkdir()
            git(repo, "init")
            (repo / "source").write_text("original")
            git(repo, "add", "source")
            git(repo, "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                "commit", "-m", "fixture")
            revision = git(repo, "rev-parse", "HEAD").decode().strip()

            @contextmanager
            def launch(worktree, evidence):
                yield [sys.executable, "-c", "open('source','w').write('edited'); exit(7)"], os.environ

            def verify(*args):
                self.fail("An unsuccessful agent run must not be accepted")

            evidence = root / "attempt"
            result = attempt({"revision": revision, "run_id": "fixture"},
                             repo, evidence, launch, verify, seconds=5)
            self.assertEqual(result["status"], "failed")
            self.assertEqual(json.loads((evidence / "manifest.json").read_text()), result)
            self.assertIn(b"+edited", (evidence / "final.patch").read_bytes())
            self.assertFalse((evidence / "workspace").exists())

    def test_deadline_during_checkout_preserves_partial_worktree(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            repo = root / "repo"
            repo.mkdir()
            git(repo, "init")
            git(repo, "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                "commit", "--allow-empty", "-m", "fixture")
            revision = git(repo, "rev-parse", "HEAD").decode().strip()
            calls = 0

            def remaining():
                nonlocal calls
                calls += 1
                if calls == 3:
                    raise BudgetExceeded("Campaign deadline exceeded")
                return 10

            def launch(*args):
                self.fail("Agent must not start after the deadline")

            evidence = root / "attempt"
            result = attempt({"revision": revision, "run_id": "fixture"}, repo, evidence,
                             launch, lambda *_: False, seconds=5, remaining=remaining)
            self.assertEqual(result["status"], "incomplete")
            self.assertIn("deadline", result["error"])
            self.assertEqual(json.loads((evidence / "manifest.json").read_text()), result)
            self.assertTrue((evidence / "workspace").is_dir())

    def test_deadline_during_patch_capture_preserves_worktree(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            repo = root / "repo"
            repo.mkdir()
            git(repo, "init")
            git(repo, "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                "commit", "--allow-empty", "-m", "fixture")
            revision = git(repo, "rev-parse", "HEAD").decode().strip()
            calls = 0

            def remaining():
                nonlocal calls
                calls += 1
                if calls >= 6:
                    raise BudgetExceeded("Campaign deadline exceeded")
                return 10

            @contextmanager
            def launch(worktree, evidence):
                yield [sys.executable, "-c", "open('answer','w').write('candidate')"], os.environ

            evidence = root / "attempt"
            result = attempt({"revision": revision, "run_id": "fixture"}, repo, evidence,
                             launch, lambda *_: False, seconds=5, remaining=remaining)
            self.assertEqual(result["status"], "incomplete")
            self.assertIn("deadline", result["cleanup_error"])
            self.assertEqual((evidence / "workspace" / "answer").read_text(), "candidate")
            self.assertEqual(json.loads((evidence / "manifest.json").read_text()), result)

    def test_wall_time_includes_verification_and_keeps_agent_time_separate(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            repo = root / "repo"
            repo.mkdir()
            git(repo, "init")
            git(repo, "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                "commit", "--allow-empty", "-m", "fixture")
            revision = git(repo, "rev-parse", "HEAD").decode().strip()

            @contextmanager
            def launch(worktree, evidence):
                yield [sys.executable, "-c", "pass"], os.environ

            def verify(*_):
                time.sleep(0.15)
                return True

            evidence = root / "attempt"
            result = attempt({"revision": revision, "run_id": "fixture"}, repo, evidence,
                             launch, verify, seconds=5)
            self.assertEqual(result["status"], "complete")
            self.assertGreaterEqual(result["wall_seconds"] - result["agent_seconds"], 0.15)
            self.assertEqual(result["agent_seconds"], result["agent"]["elapsed_seconds"])
            self.assertEqual(json.loads((evidence / "manifest.json").read_text()), result)

    def test_surviving_process_keeps_workspace_and_manifest(self):
        from unittest.mock import patch
        from process import ProcessStillRunning

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            repo = root / "repo"
            repo.mkdir()
            git(repo, "init")
            git(repo, "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                "commit", "--allow-empty", "-m", "fixture")
            revision = git(repo, "rev-parse", "HEAD").decode().strip()

            @contextmanager
            def launch(worktree, evidence):
                yield ["ignored"], os.environ

            def stuck(*args, **kwargs):
                (kwargs["cwd"] / "answer").write_text("partial")
                raise ProcessStillRunning("Process 42 did not exit after SIGKILL")

            evidence = root / "attempt"
            with patch("attempt.run", side_effect=stuck):
                result = attempt({"revision": revision, "run_id": "fixture"}, repo, evidence,
                                 launch, lambda *_: self.fail("must not score"), seconds=5)
            self.assertEqual(result["status"], "incomplete")
            self.assertTrue(result["workspace_retained"])
            self.assertTrue(result["evidence_may_change"])
            self.assertEqual((evidence / "workspace" / "answer").read_text(), "partial")
            self.assertEqual(json.loads((evidence / "manifest.json").read_text()), result)
