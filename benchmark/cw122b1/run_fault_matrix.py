"""Run the deterministic CW-122B1-F1 proxy fault tests without a model provider."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import time

REPO = Path(__file__).resolve().parents[2]
CWA = REPO / 'benchmark' / 'cw122a'
sys.path.insert(0, str(CWA))
try:
    from offline_env import build_environment
finally:
    sys.path.pop(0)

sys.path.insert(0, str(Path(__file__).resolve().parent))
try:
    from run_mock_campaign import (
        install_termination_handlers, prepare_evidence, restore_signal_handlers,
        run_bounded, verified_git_commit, write_private,
    )
finally:
    sys.path.pop(0)

TEST = [
    'cargo', 'test', '--offline', '--locked', '-p', 'cw-proxy', '--test', 'qualification',
    'f1_', '--', '--nocapture',
]
PYTHON_TEST = ['python3', '-B', 'benchmark/cw122b1/test_mock_runner.py']
CW122A_TEST = ['python3', '-B', '-m', 'unittest', 'discover', '-s', 'benchmark/cw122a', '-p', 'test_campaign.py']
PARTIAL_TEST = [
    'cargo', 'test', '--offline', '--locked', '-p', 'cw-proxy', '--test', 'qualification',
    'test_cw122b1_failed_attempt_preserves_partial_wire_evidence', '--', '--exact', '--nocapture',
]
CODEX_TEST = [
    'cargo', 'test', '--offline', '--locked', '-p', 'cw-proxy', '--test', 'qualification',
    'test_cw122b1_f1_codex_', '--', '--ignored', '--nocapture', '--test-threads=1',
]
RESULT_PREFIX = 'CW122B1_F1_RESULT:'
BOUNDARY_PREFIX = 'CW122B1_F1_BOUNDARY:'
SCENARIOS = {
    'F1-01': 'Per-session fourth request rejected before dispatch',
    'F1-02': 'Campaign-wide fourth request rejected across sessions',
    'F1-03': 'Retryable provider response bounded by per-session and campaign admission',
    'F1-04': 'HTTP 429, 500, 503, 401 and 403 classified and settled',
    'F1-05': 'Invalid, truncated, incomplete and stalled SSE responses remain unsuccessful',
    'F1-06': 'Missing and invalid usage fail closed without measured-zero substitution',
    'F1-07': 'Per-run timeout kills and reaps Codex/tool process groups',
    'F1-08': 'Campaign deadline prevents next attempt and cancels active work',
    'F1-09': 'External interruption terminates dedicated runner and preserves evidence',
    'F1-10': 'Failed cancellation is reported and blocks subsequent execution',
}
LAYER_B_REQUIRED = {'F1-01', 'F1-02', 'F1-03', 'F1-04', 'F1-05', 'F1-07', 'F1-08', 'F1-09'}


def matrix_from_output(output, return_code, evidence_root):
    evidence_root = Path(evidence_root).resolve()

    def artifact_digest(directory, item):
        directory = Path(directory)
        if directory.is_symlink():
            return None
        try:
            root = directory.resolve(strict=True)
            candidate = root / item['path']
            path = candidate.resolve(strict=True)
            if (not root.is_relative_to(evidence_root) or candidate.is_symlink()
                    or not path.is_relative_to(root) or not path.is_file()):
                return None
            if os.name == 'posix':
                if stat.S_IMODE(evidence_root.stat().st_mode) != 0o700:
                    return None
                current = evidence_root
                for part in path.relative_to(evidence_root).parts[:-1]:
                    current = current / part
                    if current.is_symlink() or stat.S_IMODE(current.stat().st_mode) != 0o700:
                        return None
                if stat.S_IMODE(path.stat().st_mode) != 0o600:
                    return None
            return path, hashlib.sha256(path.read_bytes()).hexdigest()
        except (KeyError, OSError, ValueError):
            return None

    records = []
    boundary_records = []
    for line in output.decode(errors='replace').splitlines():
        position = line.find(RESULT_PREFIX)
        if position < 0:
            boundary_position = line.find(BOUNDARY_PREFIX)
            if boundary_position >= 0:
                try:
                    boundary_records.append(json.loads(line[boundary_position + len(BOUNDARY_PREFIX):]))
                except json.JSONDecodeError:
                    continue
            continue
        try:
            records.append(json.loads(line[position + len(RESULT_PREFIX):]))
        except json.JSONDecodeError:
            continue

    matrix = []
    for scenario_id, expected in SCENARIOS.items():
        attempts = [record for record in records if record.get('scenario_id') == scenario_id]
        if not attempts:
            matrix.append({
                'scenario_id': scenario_id, 'injection_boundary': 'NOT_EXERCISED',
                'expected_behavior': expected, 'observed_behavior': 'No structured test result emitted',
                'upstream_dispatch_count': None,
                'admission_settlement_state': 'NOT_AVAILABLE',
                'process_cleanup': 'NOT_VERIFIED', 'evidence_preservation': 'NOT_VERIFIED',
                'result': 'NOT_VERIFIED',
            })
            continue

        artifacts = []
        all_evidence = True
        for attempt in attempts:
            evidence = attempt.get('evidence_preservation', {})
            if evidence.get('status') != 'PRESERVED':
                all_evidence = False
                continue
            evidence_files = evidence.get('files', [])
            if not evidence_files:
                all_evidence = False
                continue
            for item in evidence_files:
                verified = artifact_digest(evidence.get('directory', ''), item)
                if verified is None:
                    all_evidence = False
                    continue
                path, digest = verified
                if digest != item['sha256']:
                    all_evidence = False
                    continue
                artifacts.append({
                    'name': f"{scenario_id}_{len(artifacts):03d}.{item['kind']}",
                    'sha256': digest, 'kind': item['kind'],
                })

        tests_passed = return_code == 0 and all(item.get('result') == 'PASS' for item in attempts)
        layer_a_pass = any(item.get('qualification_layer') == 'A' and item.get('result') == 'PASS' for item in attempts)
        layer_b_pass = any(item.get('qualification_layer') == 'B' and item.get('result') == 'PASS' for item in attempts)
        layers_complete = layer_a_pass and (scenario_id not in LAYER_B_REQUIRED or layer_b_pass)
        result = 'PASS' if tests_passed and all_evidence and layers_complete else (
            'FAIL' if return_code != 0 else 'NOT_VERIFIED'
        )
        matrix.append({
            'scenario_id': scenario_id,
            'injection_boundary': sorted({item.get('injection_boundary') for item in attempts}),
            'expected_behavior': expected,
            'observed_behavior': [item.get('observed_behavior') for item in attempts],
            'upstream_dispatch_count': sum(item.get('upstream_dispatch_count', 0) for item in attempts),
            'admission_settlement_state': [{
                'accepted': item.get('accepted_admissions'),
                'rejected': item.get('rejected_admissions'),
                'settlement': item.get('settlement'),
            } for item in attempts],
            'process_cleanup': sorted({item.get('process_cleanup', 'NOT_VERIFIED') for item in attempts}),
            'layer_a_result': 'PASS' if layer_a_pass else 'NOT_VERIFIED',
            'layer_b_result': 'PASS' if layer_b_pass else 'NOT_VERIFIED',
            'evidence_preservation': {
                'verified': all_evidence,
                'artifacts': artifacts,
            },
            'result': result,
        })
    def evidence_valid(record):
        evidence = record.get('evidence_preservation', {})
        if evidence.get('status') != 'PRESERVED':
            return False
        files = evidence.get('files', [])
        return bool(files) and all(
            (verified := artifact_digest(evidence.get('directory', ''), item)) is not None
            and verified[1] == item['sha256']
            for item in files
        )

    after_task = next((item for item in boundary_records
                       if item.get('boundary_id') == 'after_task_before_manifest'), None)
    boundary_specs = [
        ('before_first_provider_dispatch', 'before_first_provider_dispatch'),
        ('after_admission_before_dispatch', 'after_admission_before_dispatch'),
        ('after_ingress_before_completed_egress', 'after_ingress_before_completed_egress'),
        ('after_response_before_usage_settlement', 'after_response_before_usage_settlement'),
        ('after_task_before_final_manifest', 'after_task_before_manifest'),
    ]
    boundaries = []
    for boundary_id, output_id in boundary_specs:
        record = next((item for item in boundary_records
                       if item.get('boundary_id') == output_id), None)
        if record is None and boundary_id == 'after_task_before_final_manifest':
            record = after_task
        valid = bool(record and record.get('result') == 'PASS' and evidence_valid(record))
        boundaries.append({
            'boundary_id': boundary_id,
            'injection_boundary': record.get('injection_boundary', 'NOT_EXERCISED') if record else 'NOT_EXERCISED',
            'result': 'PASS' if valid and return_code == 0 else (
                'FAIL' if record and record.get('result') == 'FAIL' else 'NOT_VERIFIED'
            ),
            'evidence_source': 'boundary-injection' if record else 'NONE',
        })
    return {'schema': 'cw122b1-f1-fault-matrix-v1', 'scenarios': matrix,
            'partial_evidence_boundaries': boundaries}


def validate_success_path_evidence(path, commit):
    path = Path(path)
    if path.is_symlink() or not path.is_absolute() or path.resolve().is_relative_to(REPO.resolve()):
        raise ValueError('successful-path evidence must be absolute and outside the checkout')
    root = path.resolve(strict=True)
    if os.name == 'posix' and stat.S_IMODE(root.stat().st_mode) != 0o700:
        raise ValueError('successful-path evidence root must have mode 0700')
    for item in root.rglob('*'):
        if item.is_symlink():
            raise ValueError('successful-path evidence contains a symlink')
        if os.name == 'posix':
            mode = stat.S_IMODE(item.stat().st_mode)
            expected = 0o700 if item.is_dir() else 0o600
            if mode != expected:
                raise ValueError(f'unsafe successful-path evidence mode: {item}')
    manifest_bytes = (root / 'manifest.json').read_bytes()
    aggregate_bytes = (root / 'aggregate.json').read_bytes()
    launch_bytes = (root / 'launch.json').read_bytes()
    manifest = json.loads(manifest_bytes)
    aggregate = json.loads(aggregate_bytes)
    launch = json.loads(launch_bytes)
    if (manifest['status'] != 'MOCK_CODEX_CAMPAIGN_PASS' or manifest['runs'] != 12
            or manifest['requests'] != 48 or manifest['inference_calls'] != 0
            or manifest['provider_spend'] != 0 or len(manifest['attempts']) != 12
            or manifest['context_watcher_commit'] != commit
            or launch.get('commit') != commit or launch.get('live') is not False
            or launch.get('provider_calls') != 0 or launch.get('provider_spend_usd') != 0
            or manifest['hash_algorithm'] != 'SHA-256'
            or manifest['campaign_sha256'] != hashlib.sha256((CWA / 'campaign.json').read_bytes()).hexdigest()
            or manifest['cw122a_freeze_sha256'] != hashlib.sha256((CWA / 'freeze.json').read_bytes()).hexdigest()
            or aggregate.get('private_manifest_sha256') != hashlib.sha256(manifest_bytes).hexdigest()
            or aggregate.get('private_launch_sha256') != hashlib.sha256(launch_bytes).hexdigest()
            or aggregate.get('context_watcher_commit') != commit
            or aggregate.get('status') != manifest['status']
            or aggregate.get('runs') != 12 or aggregate.get('requests') != 48
            or aggregate.get('provider_calls') != 0 or aggregate.get('provider_spend_usd') != 0):
        raise ValueError('successful-path evidence disagrees with the frozen run contract or current commit')
    matched_pairs = {}
    for attempt in manifest['attempts']:
        if not attempt['acceptance']['passed']:
            raise ValueError(f"acceptance failed in {attempt['pair_id']}/{attempt['mode']}")
        pair_inputs = (attempt['initial_fixture_hashes'], attempt['initial_prompt_sha256'],
                       attempt['task_instructions_sha256'], attempt['model'],
                       attempt['sampling_observed'])
        prior = matched_pairs.setdefault(attempt['pair_id'], pair_inputs)
        if prior != pair_inputs:
            raise ValueError(f"matched arms differ in {attempt['pair_id']}")
        for name, digest in attempt['initial_fixture_hashes'].items():
            fixture = CWA / 'fixtures' / attempt['family'] / name
            if hashlib.sha256(fixture.read_bytes()).hexdigest() != digest:
                raise ValueError(f"frozen fixture hash mismatch in {attempt['pair_id']}: {name}")
        requests = attempt['requests']
        if len(requests) != 4:
            raise ValueError(f"unexpected request count in {attempt['pair_id']}/{attempt['mode']}")
        session_root = root / 'raw' / attempt['session_id']
        for request in requests:
            index = request['index']
            for key, name in (
                ('ingress_sha256', f'request_{index:02}.json'),
                ('egress_sha256', f'egress_{index:02}.json'),
                ('response_sha256', f'response_{index:02}.sse'),
            ):
                artifact = session_root / name
                if hashlib.sha256(artifact.read_bytes()).hexdigest() != request[key]:
                    raise ValueError(f"{key} mismatch in {attempt['pair_id']}/{attempt['mode']} request {index}")
        for index, digest in enumerate(attempt['transcript']['sha256']):
            rollout = session_root / f'rollout_{index:02}.jsonl'
            if hashlib.sha256(rollout.read_bytes()).hexdigest() != digest:
                raise ValueError(f"transcript hash mismatch in {attempt['pair_id']}/{attempt['mode']}")
        if attempt['mode'] == 'M1':
            if any(request['applied_rule_ids'] or request['ingress_sha256'] != request['egress_sha256']
                   for request in requests):
                raise ValueError(f"M1 mutation observed in {attempt['pair_id']}")
        elif attempt['mode'] == 'M2':
            rule_id = 'cw122b1-historical-prune'
            if (attempt['treatment']['status'] != 'verified_mock_wire_treatment'
                    or attempt['treatment']['verified_requests'] != 2
                    or sum(rule_id in request['applied_rule_ids'] for request in requests) != 2
                    or any(set(request['applied_rule_ids']) - {rule_id} for request in requests)
                    or any((bool(request['applied_rule_ids']) ==
                            (request['ingress_sha256'] == request['egress_sha256']))
                           for request in requests)):
                raise ValueError(f"M2 treatment mismatch in {attempt['pair_id']}")
        else:
            raise ValueError(f"unknown mode in {attempt['pair_id']}: {attempt['mode']}")
    return {
        'status': 'PASS', 'commit': commit, 'runs': manifest['runs'],
        'requests': manifest['requests'], 'provider_calls': 0, 'provider_spend_usd': 0,
        'aggregate_sha256': hashlib.sha256(aggregate_bytes).hexdigest(),
        'manifest_sha256': hashlib.sha256(manifest_bytes).hexdigest(),
    }


def private_subdirs(root, *parts):
    current = Path(root)
    for part in parts:
        current = current / part
        current.mkdir(mode=0o700, exist_ok=True)
        current.chmod(0o700)
    return current


def descendant_commands(root_pid):
    proc_root = Path('/proc')
    parents = {}
    commands = {}
    for entry in proc_root.iterdir():
        if not entry.name.isdigit():
            continue
        try:
            fields = (entry / 'stat').read_text().split(') ', 1)[1].split()
            parents[int(entry.name)] = int(fields[1])
            commands[int(entry.name)] = (entry / 'cmdline').read_bytes().replace(b'\0', b' ').decode(errors='replace')
        except (OSError, ValueError, IndexError):
            continue
    descendants = {root_pid}
    changed = True
    while changed:
        changed = False
        for pid, parent in parents.items():
            if parent in descendants and pid not in descendants:
                descendants.add(pid)
                changed = True
    return {pid: commands.get(pid, '') for pid in descendants if pid != root_pid}


def run_interrupted_codex_campaign(evidence, env, timeout):
    layer_root = private_subdirs(evidence, 'faults', 'F1-09', 'layer-b')
    worker_root = private_subdirs(layer_root, 'codex-campaign')
    command = [
        sys.executable, '-B', 'benchmark/cw122b1/run_mock_campaign.py',
        '--evidence-dir', str(worker_root), '--fault-scenario', 'F1-09',
    ]
    proc = subprocess.Popen(
        command, cwd=REPO, env=env, stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, start_new_session=True,
    )
    ready_path = worker_root / 'faults/F1-09/layer-b/active-attempt.json'
    deadline = time.monotonic() + timeout
    codex_pid = None
    result = 'NOT_VERIFIED'
    observed = 'active Codex request marker was not observed before timeout or exit'
    output = b''
    while time.monotonic() < deadline:
        if ready_path.is_file():
            processes = descendant_commands(proc.pid)
            codex = [(pid, command_line) for pid, command_line in processes.items()
                     if 'codex' in command_line.lower() and 'exec' in command_line.lower()]
            if codex:
                codex_pid = codex[0][0]
                observed = 'Codex exec process was active after the loopback mock received its request'
                proc.send_signal(signal.SIGTERM)
                break
        if proc.poll() is not None:
            break
        time.sleep(0.02)
    else:
        if proc.poll() is None:
            proc.send_signal(signal.SIGTERM)
            observed = 'runner deadline expired before active Codex request was confirmed'
    try:
        output, _ = proc.communicate(timeout=20)
    except subprocess.TimeoutExpired:
        observed = 'dedicated runner did not exit after SIGTERM; cleanup is unverified'
        proc.kill()
        output, _ = proc.communicate()
        result = 'FAIL'

    result_path = worker_root / 'runner-result.json'
    active = json.loads(ready_path.read_text()) if ready_path.is_file() else None
    runner_result = json.loads(result_path.read_text()) if result_path.is_file() else {}
    interruption = runner_result.get('interruption') or {}
    cleanup = interruption.get('process_cleanup') or {}
    if (result != 'FAIL' and active and proc.returncode == 130
            and runner_result.get('status') == 'INTERRUPTED'
            and interruption.get('signal') == 'SIGTERM'
            and interruption.get('verified') is True and cleanup.get('verified') is True
            and active.get('upstream_dispatch_count') == 1
            and active.get('accepted_admissions') == 1):
        result = 'PASS'
        observed = 'SIGTERM stopped the active Codex attempt; the loopback request, admission and unsettled reservation were recorded'
    elif result != 'FAIL':
        result = 'NOT_VERIFIED'

    files = []
    for path in sorted(worker_root.rglob('*')):
        if path.is_file():
            files.append({
                'path': path.relative_to(evidence).as_posix(),
                'sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
                'kind': path.suffix.lstrip('.') or 'artifact',
            })
    record = {
        'scenario_id': 'F1-09',
        'injection_boundary': 'SIGTERM sent to dedicated campaign runner while Codex awaits the stalled loopback mock response',
        'expected_behavior': 'runner terminates Codex process tree, records cleanup, and preserves observed private evidence',
        'observed_behavior': observed,
        'upstream_dispatch_count': int((active or {}).get('upstream_dispatch_count', 0)),
        'accepted_admissions': int((active or {}).get('accepted_admissions', 0)),
        'rejected_admissions': 0,
        'settlement': {
            'reserved_usd': (active or {}).get('reserved_usd'),
            'status': (active or {}).get('settlement', 'NOT_AVAILABLE'),
            'process_cleanup_verified': cleanup.get('verified', False),
        },
        'process_cleanup': 'VERIFIED' if cleanup.get('verified') is True else 'NOT_VERIFIED',
        'evidence_preservation': {
            'status': 'PRESERVED' if active and files else 'NOT_VERIFIED',
            'directory': str(evidence), 'files': files,
        },
        'qualification_layer': 'B', 'result': result,
        'codex_pid_observed': codex_pid is not None,
    }
    return (0 if result == 'PASS' else 0 if result == 'NOT_VERIFIED' else 1), (
        output + b'\n' + RESULT_PREFIX.encode() + json.dumps(record, sort_keys=True).encode() + b'\n'
    )


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--evidence-dir', required=True, type=Path)
    parser.add_argument('--timeout-seconds', type=float, default=120)
    parser.add_argument('--success-path-evidence-dir', type=Path)
    args = parser.parse_args(argv)
    evidence = prepare_evidence(args.evidence_dir)
    env = build_environment(os.environ)
    commit = verified_git_commit(REPO, env)
    env['CW122B1_F1_EVIDENCE_DIR'] = str(evidence)
    env['PYTHONDONTWRITEBYTECODE'] = '1'
    started = time.monotonic()
    outputs = []
    return_code = 0
    commands = [
        ('LAYER_A_RUST', TEST), ('PARTIAL_EVIDENCE', PARTIAL_TEST),
        ('CW122A_REGRESSION', CW122A_TEST), ('RUNNER_TESTS', PYTHON_TEST),
    ]
    codex_path = shutil.which('codex', path=env.get('PATH'))
    codex_version = 'NOT_AVAILABLE'
    if codex_path:
        version = subprocess.run(
            [codex_path, '--version'], cwd=REPO, env=env, stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, timeout=10, check=True,
        )
        codex_version = version.stdout.strip().splitlines()[0]
        commands.append(('LAYER_B_CODEX', CODEX_TEST))
    else:
        outputs.append(b'LAYER_B_CODEX: NOT_VERIFIED; Codex CLI unavailable on allowlisted PATH\n')
    write_private(evidence / 'launch.json', {
        'scope': 'CW122B1_F1_MOCK_ONLY_FAULT_QUALIFICATION',
        'live': False, 'provider_calls': 0, 'provider_spend_usd': 0,
        'commit': commit,
        'codex_version': codex_version,
        'commands': [{'phase': phase, 'argv': command} for phase, command in commands],
        'environment_policy': 'CW-122A minimal offline allowlist',
    })
    previous_handlers = install_termination_handlers()
    try:
        for phase, command in commands:
            try:
                code, output = run_bounded(command, REPO, env, args.timeout_seconds)
            except subprocess.TimeoutExpired as error:
                code, output = 124, error.output or b''
            except InterruptedError as error:
                code, output = 130, getattr(error, 'output', b'') or b''
            except Exception as error:
                code, output = 125, f'{type(error).__name__}: {error}'.encode()
            outputs.extend([f'=== {phase} ===\n'.encode(), output])
            if code != 0:
                return_code = code
                break
        if return_code == 0 and codex_path:
            code, output = run_interrupted_codex_campaign(evidence, env, args.timeout_seconds)
            outputs.extend([b'=== LAYER_B_F1_09_EXTERNAL_INTERRUPT ===\n', output])
            if code != 0:
                return_code = code
    finally:
        restore_signal_handlers(previous_handlers)
    output = b''.join(outputs)
    output_path = evidence / 'runner-output.log'
    output_path.write_bytes(output)
    output_path.chmod(0o600)
    matrix = matrix_from_output(output, return_code, evidence)
    matrix['commit'] = commit
    matrix['codex_version'] = codex_version
    matrix['duration_ms'] = round((time.monotonic() - started) * 1000)
    matrix['provider_calls'] = 0
    matrix['provider_spend_usd'] = 0
    matrix['runner_exit_code'] = return_code
    if args.success_path_evidence_dir:
        try:
            matrix['successful_path_regression'] = validate_success_path_evidence(
                args.success_path_evidence_dir, commit
            )
        except (OSError, ValueError, KeyError, TypeError, json.JSONDecodeError) as error:
            matrix['successful_path_regression'] = {
                'status': 'FAIL', 'reason': f'{type(error).__name__}: {error}',
                'provider_calls': 0, 'provider_spend_usd': 0,
            }
    else:
        matrix['successful_path_regression'] = {
            'status': 'NOT_VERIFIED', 'reason': 'No current-commit success-path evidence supplied.',
            'provider_calls': 0, 'provider_spend_usd': 0,
        }
    matrix['evidence'] = {
        'private_directory_mode': '0700',
        'runner_output_sha256': hashlib.sha256(output).hexdigest(),
    }
    result_counts = {result: sum(item['result'] == result for item in matrix['scenarios'])
                     for result in ('PASS', 'FAIL', 'NOT_VERIFIED')}
    status = (
        'FAILED' if any(item['result'] == 'FAIL' for item in matrix['scenarios']) else
        'FAILED' if matrix['successful_path_regression']['status'] == 'FAIL' else
        'BLOCKED_WITH_UNVERIFIED_SCENARIOS' if (
            any(item['result'] == 'NOT_VERIFIED' for item in matrix['scenarios'])
            or matrix['successful_path_regression']['status'] != 'PASS'
        ) else
        'READY_FOR_REVIEW'
    )
    matrix['status'] = status
    write_private(evidence / 'fault-matrix.json', matrix)
    sanitized = dict(matrix)
    sanitized['evidence_directory'] = 'PRIVATE_EVIDENCE'
    target = REPO / 'benchmark' / 'cw122b1' / 'fault-matrix.json'
    target.write_text(json.dumps(sanitized, indent=2, sort_keys=True) + '\n')
    print(json.dumps({
        'status': status,
        'commit': commit, 'matrix': str(target), 'private_evidence': str(evidence),
        'result_counts': result_counts, 'provider_calls': 0, 'provider_spend_usd': 0,
    }))
    return return_code


if __name__ == '__main__':
    raise SystemExit(main())
