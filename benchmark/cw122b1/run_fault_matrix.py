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
                for part in root.relative_to(evidence_root).parts:
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
    boundaries = [
        {'boundary_id': 'before_first_provider_dispatch', 'injection_boundary': 'request admission rejection is tested, but runner interruption evidence is not',
         'result': 'NOT_VERIFIED', 'evidence_source': 'F1-02 admission record; not an interruption'},
        {'boundary_id': 'after_admission_before_dispatch', 'injection_boundary': 'no deterministic fault hook at this boundary',
         'result': 'NOT_VERIFIED', 'evidence_source': 'NONE'},
        {'boundary_id': 'after_ingress_before_completed_egress', 'injection_boundary': 'provider errors preserve wire records; exact interruption point is not injected',
         'result': 'NOT_VERIFIED', 'evidence_source': 'F1-04 error records; not an interruption'},
        {'boundary_id': 'after_response_before_usage_settlement', 'injection_boundary': 'settlement errors are tested; process interruption before settlement is not',
         'result': 'NOT_VERIFIED', 'evidence_source': 'F1-06 settlement records; not an interruption'},
        {'boundary_id': 'after_task_before_final_manifest',
         'injection_boundary': after_task.get('injection_boundary', 'partial attempt guard drop') if after_task else 'NOT_EXERCISED',
         'result': 'PASS' if after_task and after_task.get('result') == 'PASS' and evidence_valid(after_task) else 'NOT_VERIFIED',
         'evidence_source': 'partial-evidence-boundary' if after_task else 'NONE'},
    ]
    return {'schema': 'cw122b1-f1-fault-matrix-v1', 'scenarios': matrix,
            'partial_evidence_boundaries': boundaries}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--evidence-dir', required=True, type=Path)
    parser.add_argument('--timeout-seconds', type=float, default=120)
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
    matrix['successful_path_regression'] = {
        'status': 'NOT_VERIFIED',
        'reason': 'The original 12-attempt runner stopped at its CW-122A source-freeze check after the F1 SpendGuard change; the historical freeze was preserved.',
        'provider_calls': 0,
    }
    matrix['evidence'] = {
        'private_directory_mode': '0700',
        'runner_output_sha256': hashlib.sha256(output).hexdigest(),
    }
    result_counts = {result: sum(item['result'] == result for item in matrix['scenarios'])
                     for result in ('PASS', 'FAIL', 'NOT_VERIFIED')}
    status = (
        'FAILED' if any(item['result'] == 'FAIL' for item in matrix['scenarios']) else
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
