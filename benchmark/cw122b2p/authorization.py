"""Validate a future authorization envelope; this module cannot dispatch requests."""
from datetime import datetime, timezone
from pathlib import Path
import re
import stat
from urllib.parse import urlsplit


FIELDS = {
    'authorization_id', 'approver_identity', 'approval_reference', 'issued_at', 'expires_at',
    'provider', 'model', 'endpoint', 'campaign_sha256', 'runner_sha256', 'codex_executable_sha256',
    'profile_sha256', 'pricing_sha256', 'exact_run_count', 'max_requests', 'max_requests_per_attempt',
    'max_seconds', 'max_seconds_per_attempt', 'campaign_request_guard', 'spend_ceiling_usd',
    'reservation_overrun_risk_accepted', 'credential_file', 'private_evidence_directory',
}
HASH = re.compile(r'^[0-9a-f]{64}$')
CAMPAIGN_REQUEST_GUARD = {
    'constructor': 'SpendGuard::new_with_campaign_request_limit',
    'instances': 1,
    'shared_across_attempts': True,
    'limit': 360,
}
REPO = Path(__file__).resolve().parents[2]


def require(ok, message):
    if not ok:
        raise ValueError(message)


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
                'profile_sha256', 'pricing_sha256', 'credential_file', 'private_evidence_directory'):
        require(record[key] == selection[key], 'authorization selection/hash mismatch: ' + key)
    for key in ('campaign_sha256', 'runner_sha256', 'codex_executable_sha256', 'profile_sha256', 'pricing_sha256'):
        require(isinstance(record[key], str) and HASH.fullmatch(record[key]), 'invalid SHA-256: ' + key)
    require(type(record['exact_run_count']) is int and record['exact_run_count'] == 12, 'exactly 12 attempts required')
    for key, expected in (('max_requests', 360), ('max_requests_per_attempt', 30),
                          ('max_seconds', 7200), ('max_seconds_per_attempt', 600)):
        require(type(record[key]) is int and record[key] == expected, 'invalid limit: ' + key)
    require(record['campaign_request_guard'] == CAMPAIGN_REQUEST_GUARD,
            'authorization must bind one shared bounded campaign SpendGuard')
    require(type(record['spend_ceiling_usd']) in (int, float) and 0 < record['spend_ceiling_usd'] < 1_000_000, 'finite positive spend ceiling required')
    require(record['reservation_overrun_risk_accepted'] is True, 'reservation overrun risk must be explicitly accepted')
    credential, evidence = Path(record['credential_file']), Path(record['private_evidence_directory'])
    require(credential.is_absolute() and not credential.is_symlink() and credential.is_file() and stat.S_IMODE(credential.stat().st_mode) == 0o600, 'credential must be a regular 0600 file')
    require(evidence.is_absolute() and not evidence.is_symlink() and evidence.is_dir() and stat.S_IMODE(evidence.stat().st_mode) == 0o700, 'evidence directory must be 0700')
    require(not credential.resolve().is_relative_to(REPO) and not evidence.resolve().is_relative_to(REPO), 'credential and evidence must be outside checkout')
    require(not credential.resolve().is_relative_to(evidence.resolve()), 'credential must be outside evidence directory')
    return True
