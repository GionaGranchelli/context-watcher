"""Default-off runner for the separately authorized CW-122B2-L1 smoke pair."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import stat
import sys

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / 'benchmark/cw122b2l1'))
from smoke_authorization import claim_once, no_symlink_components, validate_smoke  # noqa: E402

sys.path.insert(0, str(REPO / 'benchmark/cw122b1'))
from run_mock_campaign import (  # noqa: E402
    CancellationUnverified, build_environment, install_termination_handlers,
    restore_signal_handlers, run_bounded,
)

BUNDLE = (
    'benchmark/cw122b2l1/run_smoke.py',
    'benchmark/cw122b2l1/smoke_authorization.py',
    'benchmark/cw122b2l1/test_smoke_authorization.py',
    'benchmark/cw122b1/run_mock_campaign.py',
    'benchmark/cw122a/offline_env.py',
    'benchmark/cw122a/rehearse.py',
    'benchmark/cw122a/analyze.py',
    'benchmark/cw122b2p/profile.json',
    'benchmark/cw122b2p/pricing-2026-11-05.json',
    'benchmark/cw122a/campaign.json',
    'benchmark/cw122a/oracle.py',
    'benchmark/cw122a/oracles/f1.py',
    'benchmark/cw122a/fixtures/f1',
    'Cargo.toml',
    'Cargo.lock',
    'benchmark/cw122b2l1/smoke-contract.json',
    'crates/cw-core/Cargo.toml',
    'crates/cw-core/src',
    'crates/cw-store/Cargo.toml',
    'crates/cw-store/src',
    'crates/cw-proxy/Cargo.toml',
    'crates/cw-proxy/src',
    'crates/cw-harnesses/Cargo.toml',
    'crates/cw-harnesses/src',
    'crates/cw-proxy/tests/qualification.rs',
)
TEST = [
    'cargo', 'test', '--offline', '--locked', '-p', 'cw-proxy', '--test', 'qualification',
    'test_cw122b2l1_live_matched_smoke_pair', '--', '--ignored', '--exact', '--nocapture',
]


def sha256(path):
    digest = hashlib.sha256()
    with Path(path).open('rb') as source:
        for block in iter(lambda: source.read(1024 * 1024), b''):
            digest.update(block)
    return digest.hexdigest()


def runner_sha256():
    digest = hashlib.sha256()
    for relative in sorted(BUNDLE):
        path = REPO / relative
        if path.is_symlink():
            raise ValueError(f'runner bundle path is a symlink: {relative}')
        files = sorted(path.rglob('*')) if path.is_dir() else [path]
        for file in files:
            if file.is_symlink():
                raise ValueError(f'runner bundle file is a symlink: {file.relative_to(REPO)}')
            if not file.is_file():
                continue
            if '__pycache__' in file.parts or file.suffix == '.pyc':
                continue
            name = file.relative_to(REPO).as_posix()
            digest.update(name.encode() + b'\0')
            digest.update(bytes.fromhex(sha256(file)))
    return digest.hexdigest()


def selection(record):
    codex_path = shutil.which('codex')
    if not codex_path:
        raise ValueError('Codex CLI is unavailable')
    codex = Path(codex_path).resolve(strict=True)
    return {
        'provider': 'OpenAI API', 'model': 'gpt-6-luna',
        'endpoint': 'https://api.openai.com/v1',
        'campaign_sha256': sha256(REPO / 'benchmark/cw122a/campaign.json'),
        'runner_sha256': runner_sha256(),
        'codex_executable_path': str(codex),
        'codex_executable_sha256': sha256(codex),
        'profile_sha256': sha256(REPO / 'benchmark/cw122b2p/profile.json'),
        'pricing_sha256': sha256(REPO / 'benchmark/cw122b2p/pricing-2026-11-05.json'),
        'pair_id': 'f1-r1',
        'credential_file': record.get('credential_file'),
        'credential_sha256': record.get('credential_sha256'),
        'private_evidence_directory': record.get('private_evidence_directory'),
        'authorization_claim_directory': record.get('authorization_claim_directory'),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--run', metavar='AUTHORIZATION_JSON', help='run only with a fresh exact smoke authorization')
    args = parser.parse_args()
    if not args.run:
        print('NO_GO: offline/default mode; no authorization loaded and no provider call made')
        return 2

    auth_path = Path(args.run)
    if (not auth_path.is_absolute() or not no_symlink_components(auth_path)
            or not auth_path.is_file() or auth_path.resolve().is_relative_to(REPO.resolve())):
        raise ValueError('authorization must be a private absolute file outside checkout, with no symlink components')
    if stat.S_IMODE(auth_path.stat().st_mode) != 0o600:
        raise ValueError('authorization file must have mode 0600')
    record = json.loads(auth_path.read_text())
    if not isinstance(record, dict):
        raise ValueError('smoke authorization must be a JSON object')
    validate_smoke(record, selection(record))
    claim_once(record)
    evidence = Path(record['private_evidence_directory'])
    command = TEST + []
    env = build_environment()
    env['CW122B2L1_SMOKE_AUTHORIZATION'] = str(auth_path.resolve())
    previous = install_termination_handlers()
    try:
        try:
            code, output = run_bounded(command, REPO, env, 1200)
        except (CancellationUnverified, InterruptedError, TimeoutError, subprocess.TimeoutExpired) as error:
            output = getattr(error, 'output', b'') or b''
            (evidence / 'runner-output.log').write_bytes(output)
            (evidence / 'runner-output.log').chmod(0o600)
            raise
        log = evidence / 'runner-output.log'
        log.write_bytes(output)
        log.chmod(0o600)
        print(json.dumps({
            'status': 'PASS' if code == 0 else 'FAIL',
            'exit_code': code,
            'output_sha256': sha256(log),
            'output_bytes': log.stat().st_size,
            'evidence_directory': str(evidence),
        }, sort_keys=True))
        return code
    finally:
        restore_signal_handlers(previous)


if __name__ == '__main__':
    try:
        raise SystemExit(main())
    except (ValueError, OSError, json.JSONDecodeError, subprocess.TimeoutExpired) as error:
        print(f'NO_GO: {type(error).__name__}: {error}', file=sys.stderr)
        raise SystemExit(2)
