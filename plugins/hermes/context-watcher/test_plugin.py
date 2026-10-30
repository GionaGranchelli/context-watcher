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
            request={}, session_id="hermes_123", base_url="https://api.openai.com/v1"
        ))
        self.assertIsNone(plugin._request_headers(
            request={}, session_id="hermes_123", base_url="http://127.0.0.1:4401/unrouted_endpoint"
        ))

    def test_override_file_consumption_and_deterministic_binding(self):
        with tempfile.TemporaryDirectory() as directory:
            token_file = Path(directory) / "control-token"
            token_file.write_text("b" * 64, encoding="utf-8")
            token_file.chmod(0o600)
            os.environ["CW_CONTROL_TOKEN_FILE"] = str(token_file)

            override_file = Path(directory) / "hermes-override-12345.json"
            import json, time
            payload = {
                "session_id": "bound_override_session_abc",
                "workspace": "/tmp/test_workspace",
                "ts": time.time(),
            }
            override_file.write_text(json.dumps(payload), encoding="utf-8")
            override_file.chmod(0o600)

            os.environ["CW_HERMES_OVERRIDE_FILE"] = str(override_file)
            try:
                # 1. First request consumes the override file and binds it to native session 'local-1'
                res1 = plugin._request_headers(
                    request={},
                    session_id="local-1",
                    base_url="http://127.0.0.1:4401/deepseek",
                )
                self.assertIsNotNone(res1)
                self.assertEqual(
                    res1["request"]["extra_headers"]["x-cw-session-id"],
                    "bound_override_session_abc",
                )
                # Assert file was deleted immediately on consumption
                self.assertFalse(override_file.exists(), "Override file must be unlinked on read")
                # Assert environment variable was purged
                self.assertNotIn("CW_HERMES_OVERRIDE_FILE", os.environ)

                # 2. Subsequent request for same native session reuses cached binding
                res2 = plugin._request_headers(
                    request={},
                    session_id="local-1",
                    base_url="http://127.0.0.1:4401/deepseek",
                )
                self.assertEqual(
                    res2["request"]["extra_headers"]["x-cw-session-id"],
                    "bound_override_session_abc",
                )
            finally:
                os.environ.pop("CW_CONTROL_TOKEN_FILE", None)
                os.environ.pop("CW_HERMES_OVERRIDE_FILE", None)

    def test_override_fails_safe_without_cross_binding(self):
        with tempfile.TemporaryDirectory() as directory:
            token_file = Path(directory) / "control-token"
            token_file.write_text("c" * 64, encoding="utf-8")
            token_file.chmod(0o600)
            os.environ["CW_CONTROL_TOKEN_FILE"] = str(token_file)
            import json, time

            try:
                # Scenario A: Nonexistent override file
                os.environ["CW_HERMES_OVERRIDE_FILE"] = str(Path(directory) / "nonexistent.json")
                res_missing = plugin._request_headers(
                    request={}, session_id="local-fail1", base_url="http://127.0.0.1:4401/deepseek"
                )
                self.assertIsNone(res_missing, "Must fail closed (None) when override file missing")

                # Scenario B: Insecure permissions (0644)
                bad_perm = Path(directory) / "hermes-override-insecure.json"
                bad_perm.write_text(json.dumps({"session_id": "s_bad", "ts": time.time()}), encoding="utf-8")
                bad_perm.chmod(0o644)
                os.environ["CW_HERMES_OVERRIDE_FILE"] = str(bad_perm)
                res_perm = plugin._request_headers(
                    request={}, session_id="local-fail2", base_url="http://127.0.0.1:4401/deepseek"
                )
                self.assertIsNone(res_perm, "Must fail closed when permissions not 0600")

                # Scenario C: Stale override file (>120s old)
                stale = Path(directory) / "hermes-override-stale.json"
                stale.write_text(json.dumps({"session_id": "s_stale", "ts": time.time() - 300}), encoding="utf-8")
                stale.chmod(0o600)
                os.environ["CW_HERMES_OVERRIDE_FILE"] = str(stale)
                res_stale = plugin._request_headers(
                    request={}, session_id="local-fail3", base_url="http://127.0.0.1:4401/deepseek"
                )
                self.assertIsNone(res_stale, "Must reject stale override file")

                # Scenario D: Control token in override payload
                with_token = Path(directory) / "hermes-override-token.json"
                with_token.write_text(json.dumps({
                    "session_id": "s_token",
                    "control_token": "secret_token_12345",
                    "ts": time.time(),
                }), encoding="utf-8")
                with_token.chmod(0o600)
                os.environ["CW_HERMES_OVERRIDE_FILE"] = str(with_token)
                res_token = plugin._request_headers(
                    request={}, session_id="local-fail4", base_url="http://127.0.0.1:4401/deepseek"
                )
                self.assertIsNone(res_token, "Must reject override file containing control token")
            finally:
                os.environ.pop("CW_CONTROL_TOKEN_FILE", None)
                os.environ.pop("CW_HERMES_OVERRIDE_FILE", None)

    def test_stress_20_distinct_concurrent_sessions_no_crossover(self):
        with tempfile.TemporaryDirectory() as directory:
            token_file = Path(directory) / "control-token"
            token_file.write_text("d" * 64, encoding="utf-8")
            token_file.chmod(0o600)
            os.environ["CW_CONTROL_TOKEN_FILE"] = str(token_file)
            import json, time

            try:
                now_ts = int(time.time())
                sessions = [f"session_{i}_{now_ts}" for i in range(20)]
                for i, sid in enumerate(sessions):
                    override_file = Path(directory) / f"hermes-override-{i}.json"
                    override_file.write_text(json.dumps({
                        "session_id": sid,
                        "workspace": f"/tmp/ws_{i}",
                        "ts": time.time(),
                    }), encoding="utf-8")
                    override_file.chmod(0o600)

                    # Simulate consume per invocation
                    os.environ["CW_HERMES_OVERRIDE_FILE"] = str(override_file)
                    res = plugin._request_headers(
                        request={},
                        session_id=f"native-hermes-{i}",
                        base_url="http://127.0.0.1:4401/deepseek",
                    )
                    self.assertIsNotNone(res)
                    self.assertEqual(
                        res["request"]["extra_headers"]["x-cw-session-id"],
                        sid,
                        f"Session {i} bound to wrong identity!",
                    )
                    self.assertEqual(
                        res["request"]["extra_headers"]["x-cw-workspace"],
                        f"/tmp/ws_{i}",
                    )

                # Re-query all 20 native sessions in random order to assert cache isolation
                import random
                indices = list(range(20))
                random.shuffle(indices)
                for idx in indices:
                    res_cached = plugin._request_headers(
                        request={},
                        session_id=f"native-hermes-{idx}",
                        base_url="http://127.0.0.1:4401/deepseek",
                    )
                    self.assertEqual(
                        res_cached["request"]["extra_headers"]["x-cw-session-id"],
                        sessions[idx],
                        f"Cache cross-binding detected on native-hermes-{idx}",
                    )
            finally:
                os.environ.pop("CW_CONTROL_TOKEN_FILE", None)
                os.environ.pop("CW_HERMES_OVERRIDE_FILE", None)


if __name__ == "__main__":
    unittest.main()
