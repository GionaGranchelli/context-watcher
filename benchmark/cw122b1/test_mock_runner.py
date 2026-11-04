"""Offline safety tests for the CW-122B1 Codex runner."""
import contextlib
import io
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest

HERE = Path(__file__).resolve().parent
CWA = HERE.parent / 'cw122a'
sys.path[:0] = [str(HERE), str(CWA)]
try:
    from offline_env import build_environment
    from run_mock_campaign import (
        REPO, main, prepare_evidence, public_aggregate, run_bounded, verified_git_commit,
    )
finally:
    del sys.path[:2]


class MockRunnerTests(unittest.TestCase):
    def test_child_environment_is_an_allowlist(self):
        source = {
            'PATH': '/usr/bin', 'HOME': '/tmp/home', 'CARGO_HOME': '/tmp/cargo',
            'OPENAI_API_KEY': 'do-not-copy', 'PASSWORD': 'do-not-copy',
            'PRIVATE_KEY': 'do-not-copy', 'SERVICE_KEY': 'do-not-copy',
            'HTTP_PROXY': 'http://127.0.0.1:1', 'HTTPS_PROXY': 'http://127.0.0.1:1',
            'CW_CONTROL_TOKEN': 'do-not-copy', 'OTHER': 'do-not-copy',
        }
        result = build_environment(source)
        self.assertEqual(result['PATH'], '/usr/bin')
        self.assertEqual(result['HOME'], '/tmp/home')
        self.assertFalse((set(source) - {'PATH', 'HOME', 'CARGO_HOME'}) & set(result))
        for key in ('OPENAI_API_KEY', 'PASSWORD', 'PRIVATE_KEY', 'SERVICE_KEY',
                    'HTTP_PROXY', 'HTTPS_PROXY', 'CW_CONTROL_TOKEN', 'OTHER'):
            self.assertNotIn(key, result)

    def test_live_flag_is_rejected_before_any_runner_work(self):
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr), self.assertRaises(SystemExit) as error:
            main(['--live', '--evidence-dir', '/tmp/unused-cw122b1'])
        self.assertEqual(error.exception.code, 2)
        self.assertIn('unrecognized arguments: --live', stderr.getvalue())

    def test_raw_evidence_must_be_private_empty_and_outside_checkout(self):
        with self.assertRaisesRegex(ValueError, 'outside the repository'):
            prepare_evidence(REPO / '.audit' / 'forbidden')
        with tempfile.TemporaryDirectory(prefix='cw122b1-evidence-test-') as directory:
            path = Path(directory) / 'evidence'
            path.mkdir(mode=0o700)
            (path / 'existing').touch()
            with self.assertRaisesRegex(ValueError, 'empty'):
                prepare_evidence(path)

    def test_source_archive_without_git_metadata_fails_explicitly(self):
        with tempfile.TemporaryDirectory(prefix='cw122b1-no-git-') as directory:
            with self.assertRaisesRegex(SystemExit, 'requires a Git checkout'):
                verified_git_commit(directory, {'PATH': os.environ.get('PATH', '')})

    def test_public_aggregate_omits_session_and_host_paths(self):
        request = {
            'index': 0, 'status': 200, 'ingress_sha256': 'a', 'egress_sha256': 'b',
            'response_sha256': 'c', 'model': 'mock', 'sampling': {},
            'provider_reported_usage': {}, 'usage_provenance': 'synthetic_local_mock_response',
            'cached_tokens': None, 'first_byte_latency_ms': None,
            'first_byte_latency_provenance': 'NOT_AVAILABLE', 'applied_rule_ids': [],
            'mutation_skips': [],
        }
        attempt = {
            'pair_id': 'f1-r1', 'family': 'f1', 'repetition': 1, 'mode': 'M1',
            'request_count': 1, 'acceptance': {'passed': True},
            'treatment': {'status': 'not_applied_passthrough'}, 'tool_error_count': 0,
            'provider_error_count': 0, 'retry_count': None, 'retry_count_status': 'NOT_AVAILABLE',
            'bootstrap_wall_ms': 1, 'coding_wall_ms': 1, 'complete_session_wall_ms': 2,
            'codex_cli_diagnostics': {}, 'requests': [request],
            'transcript': {'files': 1, 'sha256': ['d'], 'stub_absent': True},
            'session_id': 'private-session-id', 'codex_path': '/private/codex',
        }
        manifest = {
            'scope': 'mock', 'status': 'pass', 'codex_version': 'test',
            'hash_algorithm': 'SHA-256',
            'campaign_sha256': 'e', 'cw122a_freeze_sha256': 'f',
            'context_watcher_commit': 'g', 'runs': 1,
            'requests': 1, 'elapsed_ms': 2, 'inference_calls': 0,
            'provider_spend': 0, 'sampling_observed': {}, 'cache_fairness': 'NOT_EVALUATED',
            'codex_analytics_enabled': False, 'codex_tool_network_access': 'DISABLED',
            'product_value': 'NOT_EVALUATED', 'attempts': [attempt],
        }
        aggregate = public_aggregate(manifest)
        self.assertEqual(aggregate['hash_algorithm'], 'SHA-256')
        self.assertNotIn('private-session-id', repr(aggregate))
        self.assertNotIn('/private/codex', repr(aggregate))

    @unittest.skipUnless(os.name == 'posix' and Path('/proc').is_dir(), 'nested process groups require Linux /proc')
    def test_timeout_kills_codex_nested_process_group(self):
        with tempfile.TemporaryDirectory(prefix='cw122b1-cancel-') as directory:
            pid_file = Path(directory) / 'child.pid'
            script = (
                'import subprocess,sys,time; '
                f'p=subprocess.Popen([sys.executable,"-c","import time; time.sleep(60)"],start_new_session=True); '
                f'open({str(pid_file)!r},"w").write(str(p.pid)); time.sleep(60)'
            )
            with self.assertRaises(subprocess.TimeoutExpired):
                run_bounded([sys.executable, '-c', script], directory, os.environ.copy(), 0.2)
            child_pid = int(pid_file.read_text())
            deadline = time.monotonic() + 2
            while time.monotonic() < deadline:
                try:
                    os.kill(child_pid, 0)
                    with open(f'/proc/{child_pid}/stat') as stat_file:
                        if stat_file.read().split()[2] == 'Z':
                            break
                    time.sleep(0.02)
                except ProcessLookupError:
                    break
            else:
                self.fail('timed-out command left a live descendant')


if __name__ == '__main__':
    unittest.main()
