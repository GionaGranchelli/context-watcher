"""Opt-in scoring integration on a clean Kanban worktree."""
import os
import sys
import tempfile
import unittest
from contextlib import contextmanager
from pathlib import Path

from attempt import attempt
from schedule import schedule
from verify_task import verify_task

FIXTURE = Path(__file__).resolve().parent / "fixtures/HealthController.kt"
DESTINATION = "services/api/src/main/kotlin/com/cwk/api/HealthController.kt"


@unittest.skipUnless(os.environ.get("CW105_KANBAN"), "set CW105_KANBAN to a Kanban checkout")
class ScoredAttemptTest(unittest.TestCase):
    def test_scores_submitted_patch_through_real_spring(self):
        entry = next(schedule())

        @contextmanager
        def launch(worktree, evidence):
            yield [
                sys.executable, "-c",
                "import shutil,sys; shutil.copyfile(sys.argv[1], sys.argv[2])",
                str(FIXTURE), DESTINATION,
            ], {"PATH": os.defpath}

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "evidence"
            result = attempt(
                entry, Path(os.environ["CW105_KANBAN"]), root, launch,
                lambda worktree, evidence: verify_task(worktree, evidence, entry["task"]),
                seconds=10,
            )
            self.assertEqual(result["status"], "complete", result)
            self.assertIs(result["correct"], True)
            self.assertIn(b"HealthController.kt", (root / "final.patch").read_bytes())
            self.assertTrue((root / "build.log").exists())
            self.assertFalse((root / "workspace").exists())

    def test_debug_seed_is_present_and_patch_contains_only_repair(self):
        entry = next(row for row in schedule() if row["task"] == "web-api-base-url")

        @contextmanager
        def launch(worktree, evidence):
            yield [
                sys.executable, "-c",
                "from pathlib import Path; p=Path('apps/web/app/composables/useApiHealth.js'); "
                "p.write_text(p.read_text().replace('/api/health', '/actuator/health'))",
            ], {"PATH": os.defpath}

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "evidence"
            result = attempt(
                entry, Path(os.environ["CW105_KANBAN"]), root, launch,
                lambda *_: True, seconds=10,
            )
            self.assertEqual(result["status"], "complete", result)
            patch = (root / "final.patch").read_text()
            self.assertIn("-  const response = await fetch", patch)
            self.assertIn("+  const response = await fetch", patch)
            self.assertNotIn("new file mode", patch)
            self.assertNotIn("HealthController.kt", patch)
