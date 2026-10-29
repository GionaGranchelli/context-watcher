import importlib.util
import os
import tempfile
import unittest
from pathlib import Path


spec = importlib.util.spec_from_file_location("cw_hermes_plugin", Path(__file__).with_name("__init__.py"))
plugin = importlib.util.module_from_spec(spec)
spec.loader.exec_module(plugin)


class HermesMiddlewareTest(unittest.TestCase):
    def test_adds_authenticated_session_headers_only_to_request_kwargs(self):
        with tempfile.TemporaryDirectory() as directory:
            token_file = Path(directory) / "control-token"
            token_file.write_text("a" * 64, encoding="utf-8")
            token_file.chmod(0o600)
            old_file = os.environ.get("CW_CONTROL_TOKEN_FILE")
            old_session = os.environ.get("CW_SESSION_ID")
            os.environ["CW_CONTROL_TOKEN_FILE"] = str(token_file)
            os.environ.pop("CW_SESSION_ID", None)
            try:
                original = {"model": "deepseek-test", "messages": []}
                result = plugin._request_headers(
                    request=original,
                    session_id="hermes_123",
                    base_url="http://127.0.0.1:4401/deepseek",
                )
            finally:
                if old_file is None:
                    os.environ.pop("CW_CONTROL_TOKEN_FILE", None)
                else:
                    os.environ["CW_CONTROL_TOKEN_FILE"] = old_file
                if old_session is None:
                    os.environ.pop("CW_SESSION_ID", None)
                else:
                    os.environ["CW_SESSION_ID"] = old_session

            self.assertEqual(result["request"]["extra_headers"]["x-cw-session-id"], "hermes_123")
            self.assertEqual(result["request"]["extra_headers"]["x-cw-control-token"], "a" * 64)
            self.assertNotIn("extra_headers", original)

    def test_rejects_invalid_session_ids_and_insecure_token_files(self):
        with tempfile.TemporaryDirectory() as directory:
            token_file = Path(directory) / "control-token"
            token_file.write_text("a" * 64, encoding="utf-8")
            token_file.chmod(0o644)
            old_file = os.environ.get("CW_CONTROL_TOKEN_FILE")
            os.environ["CW_CONTROL_TOKEN_FILE"] = str(token_file)
            try:
                self.assertIsNone(plugin._request_headers(
                    request={}, session_id="bad/id", base_url="http://127.0.0.1:4401/deepseek"
                ))
                self.assertIsNone(plugin._request_headers(
                    request={}, session_id="valid_id", base_url="http://127.0.0.1:4401/deepseek"
                ))
            finally:
                if old_file is None:
                    os.environ.pop("CW_CONTROL_TOKEN_FILE", None)
                else:
                    os.environ["CW_CONTROL_TOKEN_FILE"] = old_file

    def test_never_sends_the_cw_token_to_a_provider_endpoint(self):
        self.assertIsNone(plugin._request_headers(
            request={}, session_id="hermes_123", base_url="https://api.deepseek.com"
        ))
        self.assertIsNone(plugin._request_headers(
            request={}, session_id="hermes_123", base_url="http://127.0.0.1:4401/openai/v1"
        ))


if __name__ == "__main__":
    unittest.main()
