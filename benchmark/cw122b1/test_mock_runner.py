"""Offline safety tests for the CW-122B1 Codex runner."""
import contextlib
import hashlib
import io
import json
import os
from pathlib import Path
import signal
import stat
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
    from run_fault_matrix import matrix_from_output, matrix_status, validate_success_path_evidence
    from run_mock_campaign import (
        REPO, kill_process_tree, main, prepare_evidence, public_aggregate,
        run_bounded, verified_git_commit, write_private,
    )
finally:
    del sys.path[:2]


class MockRunnerTests(unittest.TestCase):
    def test_unverified_partial_boundary_blocks_matrix_readiness(self):
        matrix = {
            'scenarios': [{'result': 'PASS'}],
            'partial_evidence_boundaries': [{'result': 'NOT_VERIFIED'}],
        }
        self.assertEqual(matrix_status(matrix, 'PASS'), 'BLOCKED_WITH_UNVERIFIED_SCENARIOS')

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

    @unittest.skipUnless(os.name == 'posix', 'POSIX evidence modes')
    def test_nested_private_evidence_directories_are_0700(self):
        with tempfile.TemporaryDirectory(prefix='cw122b1-private-modes-') as directory:
            root = Path(directory)
            artifact = root / 'faults' / 'F1-07' / 'attempt.json'
            write_private(artifact, {'result': 'PASS'})
            self.assertEqual(stat.S_IMODE((root / 'faults').stat().st_mode), 0o700)
            self.assertEqual(stat.S_IMODE(artifact.parent.stat().st_mode), 0o700)
            self.assertEqual(stat.S_IMODE(artifact.stat().st_mode), 0o600)

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

    def test_fault_matrix_requires_private_hashes_and_both_layers(self):
        with tempfile.TemporaryDirectory(prefix='cw122b1-matrix-') as directory:
            root = Path(directory)
            outside = root.parent / f'{root.name}-outside.bin'
            outside.write_bytes(b'private evidence')
            try:
                digest = hashlib.sha256(outside.read_bytes()).hexdigest()
                record = {
                    'scenario_id': 'F1-01', 'qualification_layer': 'A', 'result': 'PASS',
                    'upstream_dispatch_count': 1, 'accepted_admissions': 1,
                    'rejected_admissions': 1, 'process_cleanup': 'NOT_APPLICABLE',
                    'evidence_preservation': {'status': 'PRESERVED', 'directory': str(root),
                        'files': [{'path': outside.name, 'sha256': digest, 'kind': 'request'}]},
                }
                output = ('CW122B1_F1_RESULT:' + json.dumps(record)).encode()
                matrix = matrix_from_output(output, 0, root)
                first = next(item for item in matrix['scenarios'] if item['scenario_id'] == 'F1-01')
                self.assertEqual(first['layer_a_result'], 'PASS')
                self.assertEqual(first['layer_b_result'], 'NOT_VERIFIED')
                self.assertEqual(first['result'], 'NOT_VERIFIED')
                self.assertFalse(first['evidence_preservation']['verified'])
            finally:
                outside.unlink(missing_ok=True)

    def test_partial_boundary_requires_real_marker_and_verified_process_cleanup(self):
        with tempfile.TemporaryDirectory(prefix='cw122b1-boundary-') as directory:
            root = Path(directory)
            artifact = root / 'ingress.bin'
            artifact.write_bytes(b'captured ingress')
            artifact.chmod(0o600)
            record = {
                'boundary_id': 'after_ingress_before_completed_egress',
                'injection_boundary': 'proxy ingress persisted; egress not dispatched',
                'result': 'PASS', 'process_cleanup': 'VERIFIED',
                'evidence_preservation': {'status': 'PRESERVED', 'directory': str(root),
                    'files': [{'path': artifact.name,
                        'sha256': hashlib.sha256(artifact.read_bytes()).hexdigest(),
                        'kind': 'request'}]},
            }
            output = ('CW122B1_F1_BOUNDARY:' + json.dumps(record)).encode()
            matrix = matrix_from_output(output, 0, root)
            actual = next(item for item in matrix['partial_evidence_boundaries']
                          if item['boundary_id'] == record['boundary_id'])
            self.assertEqual(actual['result'], 'PASS')

            record['process_cleanup'] = 'NOT_VERIFIED'
            output = ('CW122B1_F1_BOUNDARY:' + json.dumps(record)).encode()
            matrix = matrix_from_output(output, 0, root)
            actual = next(item for item in matrix['partial_evidence_boundaries']
                          if item['boundary_id'] == record['boundary_id'])
            self.assertEqual(actual['result'], 'NOT_VERIFIED')

    @unittest.skipUnless(os.name == 'posix', 'POSIX evidence modes')
    def test_fault_matrix_rejects_world_readable_evidence_parent(self):
        with tempfile.TemporaryDirectory(prefix='cw122b1-matrix-mode-') as directory:
            root = Path(directory)
            unsafe = root / 'faults'
            unsafe.mkdir(mode=0o755)
            artifact = unsafe / 'request.bin'
            artifact.write_bytes(b'private evidence')
            artifact.chmod(0o600)
            record = {
                'scenario_id': 'F1-01', 'qualification_layer': 'A', 'result': 'PASS',
                'upstream_dispatch_count': 1, 'accepted_admissions': 1,
                'rejected_admissions': 1, 'process_cleanup': 'NOT_APPLICABLE',
                'evidence_preservation': {'status': 'PRESERVED', 'directory': str(unsafe),
                    'files': [{'path': artifact.name,
                        'sha256': hashlib.sha256(artifact.read_bytes()).hexdigest(),
                        'kind': 'request'}]},
            }
            output = ('CW122B1_F1_RESULT:' + json.dumps(record)).encode()
            matrix = matrix_from_output(output, 0, root)
            first = next(item for item in matrix['scenarios'] if item['scenario_id'] == 'F1-01')
            self.assertEqual(first['result'], 'NOT_VERIFIED')
            self.assertFalse(first['evidence_preservation']['verified'])

    def test_success_path_gate_requires_hash_bound_current_commit_and_exact_treatment(self):
        with tempfile.TemporaryDirectory(prefix='cw122b1-success-gate-') as directory:
            root = Path(directory) / 'evidence'
            root.mkdir(mode=0o700)
            (root / 'raw').mkdir(mode=0o700)
            commit = 'a' * 40
            attempts = []
            for family in ('f1', 'f2'):
                for repetition in range(1, 4):
                    for mode in ('M1', 'M2'):
                        rule = 'cw122b1-historical-prune'
                        session_id = f'{family}-r{repetition}-{mode}'
                        raw = root / 'raw' / session_id
                        raw.mkdir(mode=0o700, parents=True)
                        requests = []
                        for index in range(4):
                            ingress = f'{session_id}-ingress-{index}'.encode()
                            mutated = mode == 'M2' and index < 2
                            egress = ingress + b'-pruned' if mutated else ingress
                            response = f'{session_id}-response-{index}'.encode()
                            hashes = []
                            for name, content in (
                                (f'request_{index:02}.json', ingress),
                                (f'egress_{index:02}.json', egress),
                                (f'response_{index:02}.sse', response),
                            ):
                                artifact = raw / name
                                artifact.write_bytes(content)
                                artifact.chmod(0o600)
                                hashes.append(hashlib.sha256(content).hexdigest())
                            requests.append({
                                'index': index, 'applied_rule_ids': [rule] if mutated else [],
                                'ingress_sha256': hashes[0], 'egress_sha256': hashes[1],
                                'response_sha256': hashes[2],
                            })
                        rollout = b'{"type":"rollout"}\n'
                        (raw / 'rollout_00.jsonl').write_bytes(rollout)
                        (raw / 'rollout_00.jsonl').chmod(0o600)
                        fixture_hashes = {
                            name: hashlib.sha256((CWA / 'fixtures' / family / name).read_bytes()).hexdigest()
                            for name in ('policy.txt', 'retry.py' if family == 'f1' else 'service.py')
                        }
                        attempts.append({
                            'pair_id': f'{family}-r{repetition}', 'family': family,
                            'repetition': repetition, 'mode': mode,
                            'session_id': session_id, 'request_count': 4,
                            'acceptance': {'passed': True},
                            'initial_fixture_hashes': fixture_hashes,
                            'initial_prompt_sha256': 'b' * 64,
                            'task_instructions_sha256': 'c' * 64,
                            'model': 'gpt-4o-mini', 'sampling_observed': {'temperature': None, 'top_p': None},
                            'transcript': {'sha256': [hashlib.sha256(rollout).hexdigest()]},
                            'treatment': {
                                'rule_id': rule,
                                'status': 'verified_mock_wire_treatment' if mode == 'M2' else 'not_applied_passthrough',
                                'verified_requests': 2 if mode == 'M2' else 0,
                            },
                            'requests': requests,
                        })
            manifest = {
                'status': 'MOCK_CODEX_CAMPAIGN_PASS', 'runs': 12, 'requests': 48,
                'inference_calls': 0, 'provider_spend': 0,
                'hash_algorithm': 'SHA-256', 'context_watcher_commit': commit,
                'campaign_sha256': hashlib.sha256((CWA / 'campaign.json').read_bytes()).hexdigest(),
                'cw122a_freeze_sha256': hashlib.sha256((CWA / 'freeze.json').read_bytes()).hexdigest(),
                'attempts': attempts,
            }
            launch = {'commit': commit, 'live': False, 'provider_calls': 0, 'provider_spend_usd': 0}
            write_private(root / 'manifest.json', manifest)
            write_private(root / 'launch.json', launch)
            manifest_bytes = (root / 'manifest.json').read_bytes()
            launch_bytes = (root / 'launch.json').read_bytes()
            aggregate = {
                'status': manifest['status'], 'context_watcher_commit': commit,
                'runs': 12, 'requests': 48, 'provider_calls': 0, 'provider_spend_usd': 0,
                'private_manifest_sha256': hashlib.sha256(manifest_bytes).hexdigest(),
                'private_launch_sha256': hashlib.sha256(launch_bytes).hexdigest(),
            }
            write_private(root / 'aggregate.json', aggregate)
            result = validate_success_path_evidence(root, commit)
            self.assertEqual(result['status'], 'PASS')
            self.assertEqual((result['runs'], result['requests']), (12, 48))

            manifest['attempts'][7]['requests'][0]['applied_rule_ids'] = ['unexpected-rule']
            write_private(root / 'manifest.json', manifest)
            manifest_bytes = (root / 'manifest.json').read_bytes()
            aggregate['private_manifest_sha256'] = hashlib.sha256(manifest_bytes).hexdigest()
            write_private(root / 'aggregate.json', aggregate)
            with self.assertRaisesRegex(ValueError, 'M2 treatment mismatch'):
                validate_success_path_evidence(root, commit)

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
            evidence_root = os.environ.get('CW122B1_F1_EVIDENCE_DIR')
            if evidence_root:
                artifact = Path(evidence_root) / 'faults' / 'F1-07' / 'layer-a-process-tree.json'
                write_private(artifact, {
                    'scenario_id': 'F1-07', 'qualification_layer': 'A',
                    'injection_boundary': 'per-run timeout with detached nested descendant',
                    'process_cleanup': 'VERIFIED', 'upstream_dispatch_count': 0,
                    'result': 'PASS',
                })
                print('CW122B1_F1_RESULT:' + json.dumps({
                    'scenario_id': 'F1-07',
                    'injection_boundary': 'per-run timeout with detached nested descendant',
                    'expected_behavior': 'runner kills and reaps nested child process groups',
                    'observed_behavior': 'timed-out command returned non-success and detached child was reaped',
                    'upstream_dispatch_count': 0, 'accepted_admissions': 0,
                    'rejected_admissions': 0, 'settlement': 'NOT_APPLICABLE',
                    'process_cleanup': 'VERIFIED',
                    'evidence_preservation': {'status': 'PRESERVED',
                        'directory': str(artifact.parent),
                        'files': [{'path': artifact.name,
                            'sha256': hashlib.sha256(artifact.read_bytes()).hexdigest(),
                            'kind': 'process_tree_cleanup'}]},
                    'qualification_layer': 'A', 'result': 'PASS',
                }, sort_keys=True))

    @unittest.skipUnless(os.name == 'posix' and Path('/proc').is_dir(), 'process groups require Linux /proc')
    def test_cleanup_kills_group_when_root_exited_before_inventory(self):
        with tempfile.TemporaryDirectory(prefix='cw122b1-orphaned-group-') as directory:
            root = subprocess.Popen(
                [sys.executable, '-c',
                 'import subprocess,sys; p=subprocess.Popen([sys.executable,"-c",'
                 '"import time; time.sleep(60)"]); print(p.pid,flush=True)'],
                cwd=directory, stdout=subprocess.PIPE, text=True, start_new_session=True,
            )
            child_pid = int(root.stdout.readline())
            self.assertEqual(root.wait(timeout=5), 0)
            root.stdout.close()
            self.assertFalse(Path(f'/proc/{root.pid}').exists(), 'root process must be reaped first')

            cleanup = kill_process_tree(root.pid)
            self.assertTrue(cleanup['verified'], cleanup)
            self.assertIn(root.pid, cleanup['signaled_process_groups'])
            self.assertNotIn(root.pid, cleanup['remaining_pids'])
            self.assertNotIn(child_pid, cleanup['remaining_pids'])
            already_gone = kill_process_tree(root.pid)
            self.assertTrue(already_gone['verified'], already_gone)
            self.assertNotIn(root.pid, already_gone['signaled_process_groups'])

    @unittest.skipUnless(os.name == 'posix' and Path('/proc').is_dir(), 'signal cleanup requires Linux /proc')
    def test_external_sigterm_cleans_detached_descendant_and_preserves_partial_evidence(self):
        with tempfile.TemporaryDirectory(prefix='cw122b1-sigterm-') as directory:
            root = Path(directory)
            evidence = root / 'partial-evidence.json'
            child_pid = root / 'child.pid'
            nested = (
                'import os,sys,time; '
                f'open({str(child_pid)!r},"w").write(str(os.getpid())); '
                'time.sleep(60)'
            )
            code = (
                'import os,sys,time; from pathlib import Path; '
                f'sys.path.insert(0,{str(HERE)!r}); '
                'from run_mock_campaign import install_termination_handlers,run_bounded,write_private; '
                'install_termination_handlers(); '
                f'write_private(Path({str(evidence)!r}),{{"status":"admitted-before-interrupt"}}); '
                f'\ntry: run_bounded([sys.executable,"-c",{nested!r}],{str(root)!r},os.environ.copy(),60)\n'
                'except InterruptedError: sys.exit(130)'
            )
            runner = subprocess.Popen([sys.executable, '-c', code], cwd=root)
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline and not child_pid.exists():
                if runner.poll() is not None:
                    self.fail('dedicated runner exited before launching its child')
                time.sleep(0.01)
            self.assertTrue(child_pid.exists(), 'runner did not reach active-attempt boundary')
            runner.send_signal(signal.SIGTERM)
            self.assertNotEqual(runner.wait(timeout=5), 0)
            self.assertEqual(json.loads(evidence.read_text())['status'], 'admitted-before-interrupt')
            pid = int(child_pid.read_text())
            deadline = time.monotonic() + 2
            while time.monotonic() < deadline:
                try:
                    fields = Path(f'/proc/{pid}/stat').read_text().split(') ', 1)[1].split()
                    if fields[0] == 'Z':
                        break
                except FileNotFoundError:
                    break
                time.sleep(0.01)
            else:
                self.fail('SIGTERM cleanup left the detached child running')
            evidence_root = os.environ.get('CW122B1_F1_EVIDENCE_DIR')
            if evidence_root:
                artifact = Path(evidence_root) / 'faults' / 'F1-09' / 'interruption.json'
                write_private(artifact, {'status': 'interrupted_after_attempt_start', 'provider_calls': 0})
                print('CW122B1_F1_RESULT:' + json.dumps({
                    'scenario_id': 'F1-09', 'injection_boundary': 'dedicated runner SIGTERM while child active',
                    'expected_behavior': 'child tree stopped; partial evidence survives; runner exits nonzero',
                    'observed_behavior': 'detached child reaped and partial evidence preserved',
                    'upstream_dispatch_count': 0, 'accepted_admissions': 0, 'rejected_admissions': 0,
                    'settlement': 'NOT_APPLICABLE', 'process_cleanup': 'VERIFIED',
                    'evidence_preservation': {'status': 'PRESERVED', 'directory': str(artifact.parent),
                        'files': [{'path': artifact.name, 'sha256': hashlib.sha256(artifact.read_bytes()).hexdigest(),
                            'kind': 'interrupted_attempt_record'}]},
                    'qualification_layer': 'A', 'result': 'PASS',
                }, sort_keys=True))

    @unittest.skipUnless(os.name == 'posix' and Path('/proc').is_dir(), 'process cleanup requires Linux /proc')
    def test_failed_cancellation_is_unverified_and_blocks_next_command(self):
        next_command_started = False
        with tempfile.TemporaryDirectory(prefix='cw122b1-cancel-failure-') as directory:
            def injected_failure(pid):
                kill_process_tree(pid)
                return {'verified': False, 'remaining_pids': [], 'errors': ['injected_failure']}

            with self.assertRaisesRegex(RuntimeError, 'process cleanup unverified') as failure:
                run_bounded(
                    [sys.executable, '-c', 'import time; time.sleep(60)'],
                    directory, os.environ.copy(), 0.2, cancel=injected_failure,
                )
                next_command_started = True
        self.assertFalse(next_command_started)
        self.assertIn('injected_failure', str(failure.exception))
        evidence_root = os.environ.get('CW122B1_F1_EVIDENCE_DIR')
        if evidence_root:
            artifact = Path(evidence_root) / 'faults' / 'F1-10' / 'cancellation-failure.json'
            write_private(artifact, {'status': 'cleanup_unverified', 'subsequent_execution_blocked': True})
            print('CW122B1_F1_RESULT:' + json.dumps({
                'scenario_id': 'F1-10', 'injection_boundary': 'cancellation function returns unverified',
                'expected_behavior': 'report failure and block subsequent execution',
                'observed_behavior': 'runner raised non-success cleanup error; next command not started',
                'upstream_dispatch_count': 0, 'accepted_admissions': 0, 'rejected_admissions': 0,
                'settlement': 'NOT_APPLICABLE', 'process_cleanup': 'FAILED_INJECTED_AND_REAPED_FOR_TEST_SAFETY',
                'evidence_preservation': {'status': 'PRESERVED', 'directory': str(artifact.parent),
                    'files': [{'path': artifact.name, 'sha256': hashlib.sha256(artifact.read_bytes()).hexdigest(),
                        'kind': 'cancellation_failure_record'}]},
                'qualification_layer': 'A', 'result': 'PASS',
            }, sort_keys=True))


if __name__ == '__main__':
    unittest.main()
