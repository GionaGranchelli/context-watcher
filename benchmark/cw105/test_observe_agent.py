import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from tool_metrics import tool_metrics


class ObserveAgentTest(unittest.TestCase):
    def test_forwards_bytes_and_records_observed_tool_duration(self):
        observer = Path(__file__).with_name("observe_agent.py")
        program = (
            "import sys,time; "
            "print('{\\\"type\\\":\\\"tool_execution_start\\\",\\\"toolCallId\\\":\\\"x\\\",\\\"toolName\\\":\\\"read\\\"}',flush=True); "
            "time.sleep(0.08); "
            "print('{\\\"type\\\":\\\"tool_execution_end\\\",\\\"toolCallId\\\":\\\"x\\\",\\\"toolName\\\":\\\"read\\\"}',flush=True); "
            "print('{\\\"type\\\":\\\"agent_end\\\"}',flush=True)"
        )
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            timing = root / "timing.jsonl"
            result = subprocess.run([sys.executable, str(observer), str(timing), sys.executable,
                                     "-c", program], capture_output=True, check=True)
            lines = result.stdout.splitlines()
            expected = (b'{"type":"tool_execution_start","toolCallId":"x","toolName":"read"}\n'
                        b'{"type":"tool_execution_end","toolCallId":"x","toolName":"read"}\n'
                        b'{"type":"agent_end"}\n')
            self.assertEqual(result.stdout, expected)
            self.assertEqual([json.loads(line)["type"] for line in lines],
                             ["tool_execution_start", "tool_execution_end", "agent_end"])
            agent_log = root / "agent.log"
            agent_log.write_bytes(result.stdout)
            metrics = tool_metrics(agent_log, timing)
            self.assertEqual(metrics["tool_timing_source"], "observer_monotonic")
            self.assertGreaterEqual(metrics["tool_seconds"], 0.06)
            self.assertEqual(timing.stat().st_mode & 0o777, 0o600)

    def test_incomplete_timing_does_not_invent_duration(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            log = root / "agent.log"
            log.write_text('{"type":"tool_execution_start","toolCallId":"x","toolName":"read"}\n'
                           '{"type":"tool_execution_end","toolCallId":"x","toolName":"read"}\n'
                           '{"type":"agent_end"}\n')
            timing = root / "timing.jsonl"
            timing.write_text('{"type":"tool_execution_start","toolCallId":"x","observed_ns":1}\n')
            metrics = tool_metrics(log, timing)
            self.assertIsNone(metrics["tool_timing_source"])
            self.assertIsNone(metrics["tool_seconds"])

    def test_preserves_nonzero_exit_status(self):
        observer = Path(__file__).with_name("observe_agent.py")
        with tempfile.TemporaryDirectory() as directory:
            result = subprocess.run([sys.executable, str(observer), str(Path(directory) / "timing"),
                                     sys.executable, "-c", "print('partial'); exit(7)"],
                                    capture_output=True)
            self.assertEqual(result.returncode, 7)
            self.assertEqual(result.stdout, b"partial\n")

    def test_mcp_call_cap_kills_agent_before_next_call(self):
        observer = Path(__file__).with_name("observe_agent.py")
        program = (
            "import time; "
            "print('{\"type\":\"tool_execution_start\",\"toolCallId\":\"1\",\"toolName\":\"mcp__cw__cw_pin\"}',flush=True); "
            "print('{\"type\":\"tool_execution_start\",\"toolCallId\":\"2\",\"toolName\":\"mcp__cw__cw_mount\"}',flush=True); "
            "time.sleep(30)"
        )
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            env = dict(__import__("os").environ, CW105_MCP_CALL_LIMIT="1")
            result = subprocess.run([sys.executable, str(observer), str(root / "timing"),
                                     sys.executable, "-c", program],
                                    capture_output=True, env=env, timeout=5)
            self.assertEqual(result.returncode, 125)
            self.assertEqual(len((root / "timing").read_text().splitlines()), 2)
