import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from verify_task import documents_api_override, verify_task


class VerifyTaskTest(unittest.TestCase):
    @patch("verify_task.verify_web", return_value=False)
    @patch("verify_task.verify_api", return_value=True)
    def test_board_requires_both_oracles(self, api, web):
        workspace, evidence = Path("candidate"), Path("evidence")
        self.assertFalse(verify_task(workspace, evidence, "board-query"))
        api.assert_called_once_with(workspace, evidence, "board-query")
        web.assert_called_once_with(workspace, evidence, "board-query")

    def test_unknown_task_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "Unknown benchmark task"):
            verify_task(Path("candidate"), Path("evidence"), "unknown")

    def test_requires_readme_to_name_override(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.assertFalse(documents_api_override(root))
            (root / "README.md").write_text("Set NUXT_PUBLIC_API_BASE_URL for the API.")
            self.assertTrue(documents_api_override(root))

    def test_missing_workspace_exits_as_infrastructure_failure(self):
        import subprocess
        import sys
        from pathlib import Path

        script = Path(__file__).with_name("verify_task.py")
        result = subprocess.run(
            [sys.executable, str(script), "health-contract", "/missing", "/missing"],
            capture_output=True, text=True,
        )
        self.assertEqual(result.returncode, 2)
        self.assertIn("Verification infrastructure failed", result.stderr)
