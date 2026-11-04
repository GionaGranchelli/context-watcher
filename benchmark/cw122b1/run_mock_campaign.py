"""Run the bounded real-Codex campaign against CW's in-process local mock."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import time

REPO = Path(__file__).resolve().parents[2]
CWA = REPO / 'benchmark' / 'cw122a'
sys.path.insert(0, str(CWA))
try:
    from offline_env import build_environment
    from rehearse import verify_freeze
finally:
    sys.path.pop(0)

RUN_LIMIT_SECONDS = 600
CAMPAIGN_LIMIT_SECONDS = 7200
TEST = [
    'cargo', 'test', '--offline', '--locked', '-p', 'cw-proxy', '--test', 'qualification',
    'test_cw122b1_codex_mock_campaign', '--', '--ignored', '--exact', '--nocapture',
]


def write_private(path, value):
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + '\n')
    path.chmod(0o600)


def prepare_evidence(path):
    path = Path(path)
    if path.is_symlink() or not path.is_absolute():
        raise ValueError('evidence path must be absolute and not a symlink')
    resolved = path.resolve()
    if resolved.is_relative_to(REPO.resolve()):
        raise ValueError('raw evidence must be stored outside the repository')
    if path.exists():
        if not path.is_dir() or any(path.iterdir()) or (path.stat().st_mode & 0o777) != 0o700:
            raise ValueError('evidence directory must be empty and have mode 0700')
    else:
        path.mkdir(mode=0o700, parents=True)
    path.chmod(0o700)
    return path.resolve()


def run_bounded(command, cwd, env, timeout):
    proc = subprocess.Popen(
        command, cwd=cwd, env=env, stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        start_new_session=True,
    )
    try:
        output, _ = proc.communicate(timeout=timeout)
        return proc.returncode, output
    except subprocess.TimeoutExpired as exc:
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        output, _ = proc.communicate()
        raise subprocess.TimeoutExpired(command, timeout, output=output) from exc
    except BaseException:
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        proc.communicate()
        raise


def public_aggregate(manifest):
    return {
        'scope': manifest['scope'],
        'status': manifest['status'],
        'codex_version': manifest['codex_version'],
        'campaign_sha256': manifest['campaign_sha256'],
        'cw122a_freeze_sha256': manifest['cw122a_freeze_sha256'],
        'context_watcher_commit': manifest['context_watcher_commit'],
        'runs': manifest['runs'],
        'requests': manifest['requests'],
        'elapsed_ms': manifest['elapsed_ms'],
        'provider_calls': manifest['inference_calls'],
        'provider_spend_usd': manifest['provider_spend'],
        'sampling_observed': manifest['sampling_observed'],
        'codex_analytics_enabled': manifest['codex_analytics_enabled'],
        'codex_tool_network_access': manifest['codex_tool_network_access'],
        'cache_fairness': manifest['cache_fairness'],
        'product_value': manifest['product_value'],
        'attempts': [{
            'pair_id': attempt['pair_id'], 'family': attempt['family'],
            'repetition': attempt['repetition'], 'mode': attempt['mode'],
            'request_count': attempt['request_count'],
            'acceptance_passed': attempt['acceptance']['passed'],
            'treatment': attempt['treatment'],
            'tool_error_count': attempt['tool_error_count'],
            'provider_error_count': attempt['provider_error_count'],
            'retry_count': attempt['retry_count'],
            'retry_count_status': attempt['retry_count_status'],
            'bootstrap_wall_ms': attempt['bootstrap_wall_ms'],
            'coding_wall_ms': attempt['coding_wall_ms'],
            'complete_session_wall_ms': attempt['complete_session_wall_ms'],
            'codex_cli_diagnostics': attempt['codex_cli_diagnostics'],
            'requests': [{k: request[k] for k in (
                'index', 'status', 'ingress_sha256', 'egress_sha256', 'response_sha256',
                'model', 'sampling', 'provider_reported_usage', 'usage_provenance',
                'cached_tokens', 'first_byte_latency_ms', 'first_byte_latency_provenance',
                'applied_rule_ids', 'mutation_skips',
            )} for request in attempt['requests']],
            'transcript': {
                'files': attempt['transcript']['files'],
                'sha256': attempt['transcript']['sha256'],
                'stub_absent': attempt['transcript']['stub_absent'],
            },
        } for attempt in manifest['attempts']],
    }


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--evidence-dir', required=True, type=Path)
    args = parser.parse_args(argv)  # There is deliberately no --live mode.
    evidence = prepare_evidence(args.evidence_dir)
    env = build_environment(os.environ)
    codex = shutil.which('codex', path=env.get('PATH'))
    if not codex:
        raise SystemExit('Codex CLI is unavailable on the allowlisted PATH')
    version = subprocess.run(
        [codex, '--version'], cwd=REPO, env=env, stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, timeout=10, check=True,
    ).stdout.strip().splitlines()[0]
    freeze_sha256 = verify_freeze()
    commit = subprocess.run(
        ['git', 'rev-parse', 'HEAD'], cwd=REPO, env=env,
        stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, check=True,
    ).stdout.strip()
    env['CW122B1_EVIDENCE_DIR'] = str(evidence)
    env['CW122B1_COMMIT_SHA'] = commit
    env['CW122B1_CW122A_FREEZE_SHA256'] = freeze_sha256
    env['PYTHONDONTWRITEBYTECODE'] = '1'
    write_private(evidence / 'launch.json', {
        'scope': 'MOCK_ONLY_CODEX_QUALIFICATION', 'live': False,
        'provider_calls': 0, 'provider_spend_usd': 0,
        'codex_version': version, 'cw122a_freeze_sha256': freeze_sha256,
        'commit': commit, 'command': TEST, 'environment_policy': 'offline_env.build_environment allowlist',
    })

    started = time.monotonic()
    status = 'COMPLETED'
    return_code = 1
    output = b''
    try:
        return_code, output = run_bounded(TEST, REPO, env, CAMPAIGN_LIMIT_SECONDS)
        status = 'PASSED' if return_code == 0 else 'FAILED'
    except subprocess.TimeoutExpired as exc:
        output = exc.output or b''
        status = 'CAMPAIGN_TIMEOUT'
        return_code = 124
    except BaseException as exc:
        output = f'{type(exc).__name__}: {exc}'.encode()
        status = 'CANCELLED_OR_RUNNER_ERROR'
        return_code = 130
        write_private(evidence / 'runner-output.log', {'output': output.decode(errors='replace')})
        write_private(evidence / 'runner-result.json', {
            'status': status, 'return_code': return_code,
            'elapsed_seconds': round(time.monotonic() - started, 3),
        })
        raise

    output_path = evidence / 'runner-output.log'
    output_path.write_bytes(output)
    output_path.chmod(0o600)
    if status == 'PASSED':
        manifest_path = evidence / 'manifest.json'
        if manifest_path.is_file():
            manifest_bytes = manifest_path.read_bytes()
            aggregate = public_aggregate(json.loads(manifest_bytes))
            aggregate['private_manifest_sha256'] = hashlib.sha256(manifest_bytes).hexdigest()
            aggregate['private_launch_sha256'] = hashlib.sha256((evidence / 'launch.json').read_bytes()).hexdigest()
            write_private(evidence / 'aggregate.json', aggregate)
    write_private(evidence / 'runner-result.json', {
        'status': status, 'return_code': return_code,
        'elapsed_seconds': round(time.monotonic() - started, 3),
        'output_sha256': hashlib.sha256(output).hexdigest(),
    })
    print(json.dumps({'status': status, 'codex_version': version,
                      'evidence_dir': str(evidence), 'live': False}))
    return return_code


if __name__ == '__main__':
    raise SystemExit(main())
