import concurrent.futures
import importlib.util
import json
import os
import tempfile
import threading
import time
import unittest
from pathlib import Path


spec = importlib.util.spec_from_file_location("cw_hermes_plugin", Path(__file__).with_name("__init__.py"))
plugin = importlib.util.module_from_spec(spec)
spec.loader.exec_module(plugin)


class HermesMiddlewareTest(unittest.TestCase):
    def setUp(self):
        plugin._SESSION_OVERRIDES.clear()

    def tearDown(self):
        plugin._SESSION_OVERRIDES.clear()

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
                # Assert os.environ was NOT mutated (P1 requirement)
                self.assertEqual(os.environ.get("CW_HERMES_OVERRIDE_FILE"), str(override_file))

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

    def test_client_env_takes_precedence_over_global_env(self):
        with tempfile.TemporaryDirectory() as directory:
            token_file = Path(directory) / "control-token"
            token_file.write_text("e" * 64, encoding="utf-8")
            token_file.chmod(0o600)
            os.environ["CW_CONTROL_TOKEN_FILE"] = str(token_file)

            # Global env points to file 1
            file1 = Path(directory) / "hermes-override-global.json"
            file1.write_text(json.dumps({
                "session_id": "global_session",
                "workspace": "/tmp/ws_global",
                "ts": time.time(),
            }), encoding="utf-8")
            file1.chmod(0o600)
            os.environ["CW_HERMES_OVERRIDE_FILE"] = str(file1)

            # Client env points to file 2
            file2 = Path(directory) / "hermes-override-client.json"
            file2.write_text(json.dumps({
                "session_id": "client_session",
                "workspace": "/tmp/ws_client",
                "ts": time.time(),
            }), encoding="utf-8")
            file2.chmod(0o600)

            try:
                res = plugin._request_headers(
                    request={},
                    session_id="native-scoped-1",
                    base_url="http://127.0.0.1:4401/deepseek",
                    client_env={"CW_HERMES_OVERRIDE_FILE": str(file2)},
                )
                self.assertIsNotNone(res)
                # Client-scoped env must win over global env
                self.assertEqual(
                    res["request"]["extra_headers"]["x-cw-session-id"],
                    "client_session",
                )
                self.assertEqual(
                    res["request"]["extra_headers"]["x-cw-workspace"],
                    "/tmp/ws_client",
                )
                # file2 unlinked, file1 untouched
                self.assertFalse(file2.exists())
                self.assertTrue(file1.exists())
            finally:
                os.environ.pop("CW_CONTROL_TOKEN_FILE", None)
                os.environ.pop("CW_HERMES_OVERRIDE_FILE", None)
                if file1.exists():
                    file1.unlink()

    def test_override_fails_safe_without_cross_binding(self):
        with tempfile.TemporaryDirectory() as directory:
            token_file = Path(directory) / "control-token"
            token_file.write_text("c" * 64, encoding="utf-8")
            token_file.chmod(0o600)
            os.environ["CW_CONTROL_TOKEN_FILE"] = str(token_file)

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

                # Scenario E: Symlink attack rejection
                real_file = Path(directory) / "real-target.json"
                real_file.write_text(json.dumps({"session_id": "s_sym", "ts": time.time()}), encoding="utf-8")
                real_file.chmod(0o600)
                sym_file = Path(directory) / "symlink-override.json"
                os.symlink(str(real_file), str(sym_file))
                os.environ["CW_HERMES_OVERRIDE_FILE"] = str(sym_file)
                res_sym = plugin._request_headers(
                    request={}, session_id="local-fail5", base_url="http://127.0.0.1:4401/deepseek"
                )
                self.assertIsNone(res_sym, "Must reject symlinked override files via O_NOFOLLOW")
            finally:
                os.environ.pop("CW_CONTROL_TOKEN_FILE", None)
                os.environ.pop("CW_HERMES_OVERRIDE_FILE", None)

    def test_session_lifecycle_cleanup_hook(self):
        plugin._SESSION_OVERRIDES["sess_to_clean"] = ("target_1", "/tmp", time.time())
        self.assertIn("sess_to_clean", plugin._SESSION_OVERRIDES)
        plugin.on_session_terminated(session_id="sess_to_clean")
        self.assertNotIn("sess_to_clean", plugin._SESSION_OVERRIDES)

    def test_stress_20_distinct_sequential_sessions_no_crossover(self):
        with tempfile.TemporaryDirectory() as directory:
            token_file = Path(directory) / "control-token"
            token_file.write_text("d" * 64, encoding="utf-8")
            token_file.chmod(0o600)
            os.environ["CW_CONTROL_TOKEN_FILE"] = str(token_file)

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

                    # Use client_env to prevent polluting process global env
                    res = plugin._request_headers(
                        request={},
                        session_id=f"native-hermes-{i}",
                        base_url="http://127.0.0.1:4401/deepseek",
                        client_env={"CW_HERMES_OVERRIDE_FILE": str(override_file)},
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
                    self.assertFalse(override_file.exists())

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

    def test_stress_20_distinct_concurrent_sessions_with_barrier(self):
        """P1: Real concurrent workers with synchronization barrier forcing overlap at binding point."""
        worker_count = 20
        barrier = threading.Barrier(worker_count)
        results = [None] * worker_count
        errors = []

        with tempfile.TemporaryDirectory() as directory:
            token_file = Path(directory) / "control-token"
            token_file.write_text("f" * 64, encoding="utf-8")
            token_file.chmod(0o600)

            now_ts = int(time.time())
            expected_sids = [f"conc_session_{i}_{now_ts}" for i in range(worker_count)]
            override_files = []

            for i in range(worker_count):
                ov_path = Path(directory) / f"hermes-override-conc-{i}.json"
                ov_path.write_text(json.dumps({
                    "session_id": expected_sids[i],
                    "workspace": f"/tmp/concurrent_ws_{i}",
                    "ts": time.time(),
                }), encoding="utf-8")
                ov_path.chmod(0o600)
                override_files.append(ov_path)

            def worker_task(idx):
                client_env = {
                    "CW_CONTROL_TOKEN_FILE": str(token_file),
                    "CW_HERMES_OVERRIDE_FILE": str(override_files[idx]),
                }
                # Force all 20 threads to synchronize right before calling the middleware
                barrier.wait(timeout=10.0)
                try:
                    res = plugin._request_headers(
                        request={},
                        session_id=f"native-conc-hermes-{idx}",
                        base_url="http://127.0.0.1:4401/deepseek",
                        client_env=client_env,
                    )
                    results[idx] = res
                except Exception as e:
                    errors.append((idx, e))

            threads = [threading.Thread(target=worker_task, args=(i,)) for i in range(worker_count)]
            for t in threads:
                t.start()
            for t in threads:
                t.join(timeout=10.0)

            self.assertEqual(len(errors), 0, f"Thread errors occurred: {errors}")

            # Verify every worker succeeded with 100% isolation
            for i in range(worker_count):
                res = results[i]
                self.assertIsNotNone(res, f"Worker {i} returned None")
                headers = res["request"]["extra_headers"]
                self.assertEqual(
                    headers["x-cw-session-id"],
                    expected_sids[i],
                    f"Crossover! Worker {i} expected {expected_sids[i]} got {headers['x-cw-session-id']}",
                )
                self.assertEqual(
                    headers["x-cw-workspace"],
                    f"/tmp/concurrent_ws_{i}",
                )
                self.assertEqual(headers["x-cw-control-token"], "f" * 64)
                # Confirm override file was atomically consumed and unlinked
                self.assertFalse(override_files[i].exists(), f"Override file {i} not unlinked")

            # Assert all 20 sessions were recorded in _SESSION_OVERRIDES
            self.assertEqual(len(plugin._SESSION_OVERRIDES), worker_count)

            # Re-query concurrently to verify concurrent cached lookups
            cached_results = [None] * worker_count
            barrier_cached = threading.Barrier(worker_count)

            def cached_worker_task(idx):
                barrier_cached.wait(timeout=10.0)
                res = plugin._request_headers(
                    request={},
                    session_id=f"native-conc-hermes-{idx}",
                    base_url="http://127.0.0.1:4401/deepseek",
                    client_env={"CW_CONTROL_TOKEN_FILE": str(token_file)},
                )
                cached_results[idx] = res

            threads_cached = [threading.Thread(target=cached_worker_task, args=(i,)) for i in range(worker_count)]
            for t in threads_cached:
                t.start()
            for t in threads_cached:
                t.join(timeout=10.0)

            for i in range(worker_count):
                self.assertIsNotNone(cached_results[i])
                self.assertEqual(
                    cached_results[i]["request"]["extra_headers"]["x-cw-session-id"],
                    expected_sids[i],
                )

    def test_concurrent_race_on_single_override_file_fails_safe_and_single_consumer(self):
        """P1: Multiple concurrent threads racing on the exact same override file.
        Exactly one thread must consume it; the rest must fail safe without crashing."""
        worker_count = 10
        barrier = threading.Barrier(worker_count)
        results = [None] * worker_count
        errors = []

        with tempfile.TemporaryDirectory() as directory:
            token_file = Path(directory) / "control-token"
            token_file.write_text("g" * 64, encoding="utf-8")
            token_file.chmod(0o600)

            shared_override = Path(directory) / "hermes-override-shared.json"
            shared_override.write_text(json.dumps({
                "session_id": "single_winner_session",
                "workspace": "/tmp/single_ws",
                "ts": time.time(),
            }), encoding="utf-8")
            shared_override.chmod(0o600)

            def race_worker(idx):
                client_env = {
                    "CW_CONTROL_TOKEN_FILE": str(token_file),
                    "CW_HERMES_OVERRIDE_FILE": str(shared_override),
                }
                barrier.wait(timeout=10.0)
                try:
                    res = plugin._request_headers(
                        request={},
                        session_id=f"native-race-{idx}",
                        base_url="http://127.0.0.1:4401/deepseek",
                        client_env=client_env,
                    )
                    results[idx] = res
                except Exception as e:
                    errors.append((idx, e))

            threads = [threading.Thread(target=race_worker, args=(i,)) for i in range(worker_count)]
            for t in threads:
                t.start()
            for t in threads:
                t.join(timeout=10.0)

            self.assertEqual(len(errors), 0, f"Errors in race: {errors}")
            successful = [r for r in results if r is not None]
            # Exactly one winner
            self.assertEqual(len(successful), 1, "Exactly one thread must successfully consume the file")
            self.assertEqual(
                successful[0]["request"]["extra_headers"]["x-cw-session-id"],
                "single_winner_session",
            )
            # File must be gone
            self.assertFalse(shared_override.exists())


if __name__ == "__main__":
    unittest.main()
