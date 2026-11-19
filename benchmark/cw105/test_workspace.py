import tempfile
import unittest
from pathlib import Path

from workspace import checkout, cleanup, diff, git, snapshot


class WorkspaceTest(unittest.TestCase):
    def test_frozen_checkouts_do_not_share_edits(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            repository = root / "repo"
            repository.mkdir()
            git(repository, "init")
            (repository / "source").write_text("original\n")
            git(repository, "add", "source")
            git(repository, "-c", "user.name=Fixture", "-c",
                "user.email=fixture@example.invalid", "commit", "-m", "fixture")
            revision = git(repository, "rev-parse", "HEAD").decode().strip()
            first, second = root / "first", root / "second"
            checkout(repository, first, revision)
            (first / "source").write_text("edited\n")
            (first / "new").write_text("created\n")
            patch = diff(first, revision)
            self.assertIn(b"+edited", patch)
            self.assertIn(b"+created", patch)
            checkout(repository, second, revision)
            self.assertEqual((second / "source").read_text(), "original\n")
            self.assertFalse((second / "new").exists())
            cleanup(repository, first)
            cleanup(repository, second)

    def test_staged_seed_is_excluded_from_agent_patch(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            repo = root / "repo"
            repo.mkdir()
            git(repo, "init")
            (repo / "source").write_text("original\n")
            git(repo, "add", "source")
            git(repo, "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                "commit", "-m", "fixture")
            revision = git(repo, "rev-parse", "HEAD").decode().strip()
            candidate = root / "candidate"
            checkout(repo, candidate, revision)
            (candidate / "source").write_text("seed\n")
            git(candidate, "add", "source")
            baseline = snapshot(candidate)
            (candidate / "source").write_text("repair\n")
            git(candidate, "add", "source")
            patch = diff(candidate, baseline)
            self.assertIn(b"-seed", patch)
            self.assertIn(b"+repair", patch)
            self.assertNotIn(b"-original", patch)
            cleanup(repo, candidate)
