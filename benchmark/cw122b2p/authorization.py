"""Validate a future authorization envelope; this module cannot dispatch requests."""
from datetime import datetime, timezone
import hashlib
import os
from pathlib import Path
import re
import stat
from urllib.parse import urlsplit


FIELDS = {
    'authorization_id', 'approver_identity', 'approval_reference', 'issued_at', 'expires_at',
    'provider', 'model', 'endpoint', 'campaign_sha256', 'runner_sha256', 'codex_executable_sha256',
    'profile_sha256', 'pricing_sha256', 'exact_run_count', 'max_requests', 'max_requests_per_attempt',
    'max_seconds', 'max_seconds_per_attempt', 'campaign_request_guard', 'spend_ceiling_usd',
    'reservation_overrun_risk_accepted', 'credential_file', 'credential_sha256',
    'private_evidence_directory', 'authorization_claim_directory',
}
HASH = re.compile(r'^[0-9a-f]{64}$')
CAMPAIGN_REQUEST_GUARD = {
    'constructor': 'SpendGuard::new_with_campaign_request_limit',
    'instances': 1,
    'shared_across_attempts': True,
    'limit': 360,
}
REPO = Path(__file__).resolve().parents[2]
SMOKE_FIELDS = {
    'experiment', 'authorization_id', 'approver_identity', 'approval_reference', 'issued_at', 'expires_at',
    'provider', 'model', 'endpoint', 'processing_tier', 'campaign_sha256', 'runner_sha256',
    'codex_executable_path', 'codex_executable_sha256', 'profile_sha256', 'pricing_sha256',
    'pair_id', 'attempt_order', 'exact_attempt_count', 'max_requests', 'max_requests_per_attempt',
    'max_seconds', 'max_seconds_per_attempt', 'campaign_request_guard', 'spend_ceiling_usd',
    'request_reservation_usd', 'reservation_overrun_risk_accepted', 'credential_file',
    'credential_sha256', 'private_evidence_directory', 'authorization_claim_directory',
}
SMOKE_REQUEST_GUARD = {
    'constructor': 'SpendGuard::new_with_campaign_request_limit',
    'instances': 1,
    'shared_across_attempts': True,
    'limit': 60,
}


def require(ok, message):
    if not ok:
        raise ValueError(message)


def sha256_file(path):
    digest = hashlib.sha256()
    with Path(path).open('rb') as source:
        for block in iter(lambda: source.read(1024 * 1024), b''):
            digest.update(block)
    return digest.hexdigest()


def no_symlink_components(path):
    path = Path(path)
    if not path.is_absolute() or '..' in path.parts:
        return False
    current = Path(path.anchor)
    for part in path.parts[1:]:
        current /= part
        if current.is_symlink():
            return False
    return True


def validate(record, selection, now=None):
    require(isinstance(record, dict) and set(record) == FIELDS, 'authorization fields missing or unknown')
    require(record['authorization_id'] and record['approver_identity'] and record['approval_reference'], 'approval identity/reference required')
    try:
        issued, expires = (datetime.fromisoformat(record[k].replace('Z', '+00:00')) for k in ('issued_at', 'expires_at'))
        now = now or datetime.now(timezone.utc)
        valid_window = issued.tzinfo is not None and expires.tzinfo is not None and issued <= now < expires and 0 < (expires - issued).total_seconds() <= 86400
    except (AttributeError, TypeError, ValueError):
        valid_window = False
    require(valid_window, 'authorization expired, future, or validity exceeds 24 hours')
    endpoint = urlsplit(record['endpoint'])
    require(endpoint.scheme in ('http', 'https') and endpoint.hostname and not endpoint.username and not endpoint.password and not endpoint.query and not endpoint.fragment, 'invalid endpoint')
    for key in ('provider', 'model', 'endpoint', 'campaign_sha256', 'runner_sha256', 'codex_executable_sha256',
                'profile_sha256', 'pricing_sha256', 'credential_file', 'credential_sha256',
                'private_evidence_directory', 'authorization_claim_directory'):
        require(record[key] == selection[key], 'authorization selection/hash mismatch: ' + key)
    for key in ('campaign_sha256', 'runner_sha256', 'codex_executable_sha256', 'profile_sha256', 'pricing_sha256'):
        require(isinstance(record[key], str) and HASH.fullmatch(record[key]), 'invalid SHA-256: ' + key)
    require(isinstance(record['credential_sha256'], str) and HASH.fullmatch(record['credential_sha256']),
            'invalid SHA-256: credential_sha256')
    require(type(record['exact_run_count']) is int and record['exact_run_count'] == 12, 'exactly 12 attempts required')
    for key, expected in (('max_requests', 360), ('max_requests_per_attempt', 30),
                          ('max_seconds', 7200), ('max_seconds_per_attempt', 600)):
        require(type(record[key]) is int and record[key] == expected, 'invalid limit: ' + key)
    require(record['campaign_request_guard'] == CAMPAIGN_REQUEST_GUARD,
            'authorization must bind one shared bounded campaign SpendGuard')
    require(type(record['spend_ceiling_usd']) in (int, float) and 0 < record['spend_ceiling_usd'] < 1_000_000, 'finite positive spend ceiling required')
    require(record['reservation_overrun_risk_accepted'] is True, 'reservation overrun risk must be explicitly accepted')
    credential, evidence = Path(record['credential_file']), Path(record['private_evidence_directory'])
    claims = Path(record['authorization_claim_directory'])
    require(no_symlink_components(credential) and credential.is_file() and stat.S_IMODE(credential.stat().st_mode) == 0o600, 'credential must be a regular 0600 file with no symlink components')
    require(sha256_file(credential) == record['credential_sha256'], 'credential file hash mismatch')
    require(no_symlink_components(evidence) and evidence.is_dir() and stat.S_IMODE(evidence.stat().st_mode) == 0o700, 'evidence directory must be 0700 with no symlink components')
    require(not credential.resolve().is_relative_to(REPO) and not evidence.resolve().is_relative_to(REPO), 'credential and evidence must be outside checkout')
    require(not credential.resolve().is_relative_to(evidence.resolve()), 'credential must be outside evidence directory')
    require(no_symlink_components(claims) and claims.is_dir() and stat.S_IMODE(claims.stat().st_mode) == 0o700,
            'authorization claim directory must be 0700 with no symlink components')
    require(not claims.resolve().is_relative_to(REPO)
            and not claims.resolve().is_relative_to(evidence.resolve())
            and not evidence.resolve().is_relative_to(claims.resolve())
            and not credential.resolve().is_relative_to(claims.resolve()),
            'authorization claim directory must be private and disjoint')
    require(not any(claims.resolve().is_relative_to(root) for root in
                    (Path('/tmp'), Path('/var/tmp'), Path('/dev/shm'), Path('/run'))),
            'authorization claim directory must be persistent')
    return True


def claim_once(record):
    root = Path(record['authorization_claim_directory'])
    require(no_symlink_components(root) and root.is_dir() and stat.S_IMODE(root.stat().st_mode) == 0o700,
            'authorization claim directory is unsafe')
    marker = root / (hashlib.sha256(record['authorization_id'].encode()).hexdigest() + '.claimed')
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, 'O_NOFOLLOW', 0)
    fd = os.open(marker, flags, 0o600)
    try:
        os.write(fd, (record['authorization_id'] + '\n').encode())
        os.fsync(fd)
    finally:
        os.close(fd)
    directory_fd = os.open(root, os.O_RDONLY | getattr(os, 'O_DIRECTORY', 0))
    try:
        os.fsync(directory_fd)
    finally:
        os.close(directory_fd)


def validate_smoke(record, selection, now=None):
    require(isinstance(record, dict) and set(record) == SMOKE_FIELDS,
            'smoke authorization fields missing or unknown')
    for key in ('codex_executable_path', 'credential_file', 'private_evidence_directory',
                'authorization_claim_directory'):
        require(isinstance(record[key], str) and Path(record[key]).is_absolute(),
                'smoke authorization path must be absolute: ' + key)
    require(record['experiment'] == 'CW-122B2-L1-SMOKE', 'wrong smoke experiment')
    require(all(isinstance(record[key], str) and record[key].strip() for key in
                ('authorization_id', 'approver_identity', 'approval_reference')),
            'approval identity/reference required')
    require(re.fullmatch(r'[A-Za-z0-9_-]{1,84}', record['authorization_id']) is not None,
            'authorization ID cannot bind a valid CW session')
    try:
        issued, expires = (datetime.fromisoformat(record[k].replace('Z', '+00:00'))
                           for k in ('issued_at', 'expires_at'))
        now = now or datetime.now(timezone.utc)
        valid_window = (issued.tzinfo is not None and expires.tzinfo is not None
                        and issued <= now < expires
                        and 0 < (expires - issued).total_seconds() <= 86400)
    except (AttributeError, TypeError, ValueError):
        valid_window = False
    require(valid_window, 'smoke authorization expired, future, or validity exceeds 24 hours')
    require(record['provider'] == 'OpenAI API' and record['model'] == 'gpt-6-luna'
            and record['endpoint'] == 'https://api.openai.com/v1'
            and record['processing_tier'] == 'standard', 'smoke provider selection mismatch')
    endpoint = urlsplit(record['endpoint'])
    require(endpoint.scheme == 'https' and endpoint.hostname == 'api.openai.com'
            and not endpoint.username and not endpoint.password and not endpoint.query and not endpoint.fragment,
            'invalid smoke endpoint')
    require(record['pair_id'] == 'f1-r1' and record['attempt_order'] == ['M1', 'M2']
            and type(record['exact_attempt_count']) is int and record['exact_attempt_count'] == 2,
            'smoke must be the frozen f1-r1 M1/M2 pair')
    for key, expected in (('max_requests', 60), ('max_requests_per_attempt', 30),
                          ('max_seconds', 1200), ('max_seconds_per_attempt', 600)):
        require(type(record[key]) is int and record[key] == expected, 'invalid smoke limit: ' + key)
    require(record['campaign_request_guard'] == SMOKE_REQUEST_GUARD,
            'smoke must bind one shared 60-request SpendGuard')
    budget, reservation = record['spend_ceiling_usd'], record['request_reservation_usd']
    require(type(budget) in (int, float) and 0 < budget <= 5,
            'smoke spend ceiling must be positive and at most 5 USD')
    require(type(reservation) in (int, float) and 0 < reservation <= budget / 60,
            'per-request reservation must fit the 60-request smoke envelope')
    require(record['reservation_overrun_risk_accepted'] is True,
            'reservation overrun risk must be explicitly accepted')
    for key in ('campaign_sha256', 'runner_sha256', 'codex_executable_sha256', 'profile_sha256', 'pricing_sha256',
                'credential_sha256'):
        require(isinstance(record[key], str) and HASH.fullmatch(record[key]), 'invalid smoke SHA-256: ' + key)
    for key in ('provider', 'model', 'endpoint', 'campaign_sha256', 'runner_sha256',
                'codex_executable_path', 'codex_executable_sha256', 'profile_sha256', 'pricing_sha256',
                'pair_id', 'credential_file', 'credential_sha256', 'private_evidence_directory',
                'authorization_claim_directory'):
        require(record[key] == selection[key], 'smoke authorization mismatch: ' + key)
    credential = Path(record['credential_file'])
    evidence = Path(record['private_evidence_directory'])
    claims = Path(record['authorization_claim_directory'])
    codex = Path(record['codex_executable_path'])
    require(no_symlink_components(credential) and credential.is_file()
            and stat.S_IMODE(credential.stat().st_mode) == 0o600,
            'credential must be a regular 0600 file with no symlink components')
    require(sha256_file(credential) == record['credential_sha256'], 'credential file hash mismatch')
    require(no_symlink_components(evidence) and evidence.is_dir()
            and stat.S_IMODE(evidence.stat().st_mode) == 0o700 and not any(evidence.iterdir()),
            'private evidence directory must be empty, 0700, and contain no symlinks')
    require(no_symlink_components(claims) and claims.is_dir()
            and stat.S_IMODE(claims.stat().st_mode) == 0o700,
            'authorization claim directory must be 0700 with no symlink components')
    require(not any(claims.resolve().is_relative_to(root) for root in
                    (Path('/tmp'), Path('/var/tmp'), Path('/dev/shm'), Path('/run'))),
            'authorization claim directory must be persistent')
    require(not credential.resolve().is_relative_to(REPO) and not evidence.resolve().is_relative_to(REPO)
            and not claims.resolve().is_relative_to(REPO),
            'credential, evidence, and claim paths must be outside checkout')
    require(not credential.resolve().is_relative_to(evidence.resolve())
            and not evidence.resolve().is_relative_to(claims.resolve())
            and not credential.resolve().is_relative_to(claims.resolve()),
            'credential, evidence, and claim paths must be disjoint')
    require(no_symlink_components(codex) and codex.is_file()
            and sha256_file(codex) == record['codex_executable_sha256'],
            'Codex executable path or hash mismatch')
    return True
