import json
import tempfile
import unittest
from pathlib import Path

from agent import offline_agent


class AgentTest(unittest.TestCase):
    def test_distinct_runs_have_isolated_provider_and_session_config(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            for name, port in (("one", 9001), ("two", 9002)):
                state = root / name
                command, env = offline_agent("/bin/omp", state, port, "Say hi")
                provider = json.loads((state / "models.json").read_text())["providers"]["cw105"]
                self.assertEqual(provider["baseUrl"], f"http://127.0.0.1:{port}/v1")
                self.assertEqual(env["PI_CODING_AGENT_DIR"], str(state))
                self.assertEqual(env["HOME"], str(state / "home"))
                self.assertIn("cw105/qwen3.8-27b-ud", command)
                self.assertNotIn("OPENAI_API_KEY", env)
            with self.assertRaises(FileExistsError):
                offline_agent("/bin/omp", root / "one", 9003, "Say hi")
