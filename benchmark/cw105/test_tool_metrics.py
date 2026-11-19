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
            self.assertEqual(result["observed_tool_count"], 2)
            self.assertEqual(result["observed_mcp_count"], 1)
            self.assertIsNone(result["tool_count"])
            self.assertIsNone(result["mcp_count"])
            self.assertEqual(result["tool_calls"]["a"]["duration_seconds"], 0.05)
            self.assertFalse(result["tool_calls"]["b"]["finished"])
            self.assertIsNone(result["tool_seconds"])
            self.assertIsNone(result["mcp_seconds"])

    def test_complete_zero_tool_log_and_truncated_log_are_distinct(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "agent.log"
            path.write_text('{"type":"agent_start"}\n')
            incomplete = tool_metrics(path)
            self.assertFalse(incomplete["tool_events_complete"])
            self.assertIsNone(incomplete["tool_count"])
            self.assertIsNone(incomplete["tool_seconds"])
            path.write_text('{"type":"agent_start"}\n{"type":"agent_end"}\n')
            complete = tool_metrics(path)
            self.assertTrue(complete["tool_events_complete"])
            self.assertEqual(complete["tool_count"], 0)
            self.assertEqual(complete["tool_seconds"], 0)

    def test_malformed_tool_event_makes_counts_unknown(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "agent.log"
            path.write_text('{"type":"tool_execution_start","toolName":"read"}\n'
                            '{"type":"agent_end"}\n')
            result = tool_metrics(path)
            self.assertFalse(result["tool_events_complete"])
            self.assertIsNone(result["tool_count"])

    def test_end_without_start_does_not_claim_complete_counts(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "agent.log"
            path.write_text('{"type":"tool_execution_end","toolCallId":"a","toolName":"read"}\n'
                            '{"type":"agent_end"}\n')
            result = tool_metrics(path)
            self.assertFalse(result["tool_events_complete"])
            self.assertEqual(result["observed_tool_count"], 1)
            self.assertIsNone(result["tool_count"])

    def test_non_utf8_output_marks_events_incomplete(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "agent.log"
            path.write_bytes(b"\xff\n" + b'{"type":"agent_end"}\n')
            result = tool_metrics(path)
            self.assertFalse(result["tool_events_complete"])
            self.assertIsNone(result["tool_count"])
