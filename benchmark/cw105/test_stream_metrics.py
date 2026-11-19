import unittest

from stream_metrics import StreamMetrics
from usage import Usage


class StreamMetricsTest(unittest.TestCase):
    def test_role_chunk_does_not_count_as_first_output(self):
        stream = StreamMetrics(10)
        stream.observe('{"choices":[{"delta":{"role":"assistant"}}]}', 11)
        self.assertIsNone(stream.ttft_seconds)
        stream.observe('{"choices":[{"delta":{"content":"hi"}}]}', 12)
        stream.observe('{"usage":{"prompt_tokens":5,"completion_tokens":1}}', 13)
        self.assertFalse(stream.complete)
        self.assertEqual(stream.usage, Usage(5, 1, None))
        stream.observe("[DONE]", 14)
        self.assertTrue(stream.complete)
        self.assertEqual(stream.ttft_seconds, 2)

    def test_terminator_without_usage_is_incomplete(self):
        stream = StreamMetrics(0)
        stream.observe("[DONE]", 1)
        self.assertFalse(stream.complete)

    def test_duplicate_usage_is_rejected(self):
        stream = StreamMetrics(0)
        data = '{"usage":{"prompt_tokens":5,"completion_tokens":1}}'
        stream.observe(data, 1)
        with self.assertRaisesRegex(ValueError, "Multiple usage"):
            stream.observe(data, 2)

    def test_malformed_choices_are_rejected_even_after_first_output(self):
        stream = StreamMetrics(0)
        stream.observe('{"choices":[{"delta":{"content":"hi"}}]}', 1)
        for data in ('{"choices":null}', '{"choices":[null]}',
                     '{"choices":[{"delta":null}]}'):
            with self.subTest(data=data), self.assertRaisesRegex(ValueError, "Invalid completion"):
                stream.observe(data, 2)
