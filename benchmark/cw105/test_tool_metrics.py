import tempfile
import unittest
from pathlib import Path

from tool_metrics import tool_metrics


class ToolMetricsTest(unittest.TestCase):
    def test_unfinished_call_preserves_count_without_inventing_duration(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "agent.log"
            path.write_text(
                'Working...\n'
                '{"type":"tool_execution_start","toolCallId":"a","toolName":"read"}\n'
                '{"type":"tool_execution_end","toolCallId":"a","toolName":"read","durationMs":50}\n'
                '{"type":"tool_execution_start","toolCallId":"b","toolName":"mcp__cw__cw_pin"}\n'
            )
            result = tool_metrics(path)
            self.assertEqual(result["tool_count"], 2)
            self.assertEqual(result["mcp_count"], 1)
            self.assertEqual(result["tool_calls"]["a"]["duration_seconds"], 0.05)
            self.assertFalse(result["tool_calls"]["b"]["finished"])
            self.assertIsNone(result["tool_seconds"])
            self.assertIsNone(result["mcp_seconds"])
