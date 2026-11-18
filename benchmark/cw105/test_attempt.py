import json
import os
import sys
import tempfile
import unittest
from contextlib import contextmanager
from pathlib import Path

from attempt import attempt
from workspace import git


class AttemptTest(unittest.TestCase):
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
