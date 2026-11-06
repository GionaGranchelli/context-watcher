"""Validate a one-use authorization for the bounded L1 smoke pair."""
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import urllib.request
from urllib.parse import urlsplit


SMOKE_FIELDS = {
    'experiment', 'authorization_id', 'approval_reference', 'issued_at', 'expires_at',
    'provider', 'model', 'endpoint', 'processing_tier', 'campaign_sha256', 'runner_sha256',
    'codex_executable_path', 'codex_executable_sha256', 'profile_sha256', 'pricing_sha256',
    'pair_id', 'attempt_order', 'exact_attempt_count', 'max_requests', 'max_requests_per_attempt',
    'max_seconds', 'max_seconds_per_attempt', 'campaign_request_guard', 'spend_ceiling_usd',
    'request_reservation_usd', 'reservation_overrun_risk_accepted', 'credential_file',
    'credential_sha256', 'private_evidence_directory',
}
SMOKE_REQUEST_GUARD = {
    'constructor': 'SpendGuard::new_with_campaign_request_limit',
    'instances': 1,
    'shared_across_attempts': True,
    'limit': 60,
}
HASH = re.compile(r'^[0-9a-f]{64}$')
REPO = Path(__file__).resolve().parents[2]
GITHUB_OWNER = 'GionaGranchelli'
GITHUB_REPO = 'context-watcher'
GITHUB_PR = 46
# shortcut: pin spend approval to the current repository owner; update if ownership transfers.


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


def approval_statement(record):
    bound = {key: value for key, value in record.items() if key != 'approval_reference'}
    digest = hashlib.sha256(json.dumps(bound, sort_keys=True, separators=(',', ':')).encode()).hexdigest()
    return f'CW-122B2-L1-SMOKE-APPROVE {record["authorization_id"]} {digest}'


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def fetch_github_comment(comment_id):
    url = f'https://api.github.com/repos/{GITHUB_OWNER}/{GITHUB_REPO}/issues/comments/{comment_id}'
    request = urllib.request.Request(url, headers={
        'Accept': 'application/vnd.github+json',
        'User-Agent': 'context-watcher-l1-smoke-authorization',
        'X-GitHub-Api-Version': '2022-11-28',
    })
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), _NoRedirect())
    try:
        with opener.open(request, timeout=5) as response:
            if response.status != 200 or response.geturl() != url:
                raise ValueError('unexpected GitHub approval response')
            body = response.read(32769)
    except Exception as error:
        raise ValueError('GitHub approval could not be verified') from error
    if len(body) > 32768:
        raise ValueError('GitHub approval response is too large')
    try:
        comment = json.loads(body)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ValueError('GitHub approval response is invalid') from error
    if not isinstance(comment, dict):
        raise ValueError('GitHub approval response is invalid')
    return comment


def verify_approval(record, now=None):
    reference = record['approval_reference']
    comment = fetch_github_comment(reference)
    expected_url = f'https://api.github.com/repos/{GITHUB_OWNER}/{GITHUB_REPO}/issues/comments/{reference}'
    expected_issue = f'https://api.github.com/repos/{GITHUB_OWNER}/{GITHUB_REPO}/issues/{GITHUB_PR}'
    require(comment.get('id') == int(reference) and comment.get('url') == expected_url
            and comment.get('issue_url') == expected_issue,
            'GitHub approval comment reference mismatch')
    author = comment.get('user')
    require(isinstance(author, dict) and author.get('login') == GITHUB_OWNER
            and comment.get('author_association') == 'OWNER',
            'GitHub approval author is not the authorized repository owner')
    require(comment.get('body') == approval_statement(record)
            and comment.get('created_at') == comment.get('updated_at'),
            'GitHub approval statement is missing, mismatched, or edited')
    try:
        issued = datetime.fromisoformat(record['issued_at'].replace('Z', '+00:00'))
        expires = datetime.fromisoformat(record['expires_at'].replace('Z', '+00:00'))
        created = datetime.fromisoformat(comment['created_at'].replace('Z', '+00:00'))
    except (AttributeError, KeyError, TypeError, ValueError) as error:
        raise ValueError('GitHub approval timestamps are invalid') from error
    now = now or datetime.now(timezone.utc)
    require(issued <= created < expires and created <= now,
            'GitHub approval comment is outside the authorization window')


def claim_registry_root():
    # shortcut: bind claims to this OS account; upgrade if the runner becomes multi-host.
    try:
        import pwd
        home = Path(pwd.getpwuid(os.getuid()).pw_dir)
    except (ImportError, KeyError, OSError) as error:
        raise ValueError('cannot determine persistent OS account home for authorization claims') from error
    return home / '.local/state/context-watcher/authorization-claims'


def validate_and_claim(record, selection, now=None):
    validate_smoke(record, selection, now)
    verify_approval(record, now)
    claim_once(record)
    return True


def claim_once(record):
    root = claim_registry_root()
    require(no_symlink_components(root), 'runner claim registry path contains a symlink')
    root.mkdir(mode=0o700, parents=True, exist_ok=True)
    require(no_symlink_components(root) and root.is_dir() and stat.S_IMODE(root.stat().st_mode) == 0o700
            and root.stat().st_uid == os.getuid(),
            'runner claim registry must be an owned, persistent 0700 directory')
    marker = root / (hashlib.sha256(record['authorization_id'].encode()).hexdigest() + '.claimed')
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, 'O_NOFOLLOW', 0)
    registry_fd = os.open(root, os.O_RDONLY | getattr(os, 'O_DIRECTORY', 0) | getattr(os, 'O_NOFOLLOW', 0))
    try:
        try:
            fd = os.open(marker.name, flags, 0o600, dir_fd=registry_fd)
        except FileExistsError as error:
            raise ValueError('authorization ID already consumed') from error
        try:
            os.write(fd, (record['authorization_id'] + '\n' + approval_statement(record) + '\n').encode())
            os.fsync(fd)
        finally:
            os.close(fd)
        os.fsync(registry_fd)
    finally:
        os.close(registry_fd)


def validate_smoke(record, selection, now=None):
    require(isinstance(record, dict) and set(record) == SMOKE_FIELDS,
            'smoke authorization fields missing or unknown')
    for key in ('codex_executable_path', 'credential_file', 'private_evidence_directory'):
        require(isinstance(record[key], str) and Path(record[key]).is_absolute(),
                'smoke authorization path must be absolute: ' + key)
    require(record['experiment'] == 'CW-122B2-L1-SMOKE', 'wrong smoke experiment')
    require(isinstance(record['authorization_id'], str) and record['authorization_id'].strip(),
            'authorization ID required')
    require(re.fullmatch(r'[A-Za-z0-9_-]{1,84}', record['authorization_id']) is not None,
            'authorization ID cannot bind a valid CW session')
    require(isinstance(record['approval_reference'], str)
            and re.fullmatch(r'[1-9][0-9]{0,19}', record['approval_reference']) is not None,
            'approval reference must be a GitHub issue-comment ID')
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
                'pair_id', 'credential_file', 'credential_sha256', 'private_evidence_directory'):
        require(record[key] == selection[key], 'smoke authorization mismatch: ' + key)
    credential = Path(record['credential_file'])
    evidence = Path(record['private_evidence_directory'])
    codex = Path(record['codex_executable_path'])
    require(no_symlink_components(credential) and credential.is_file()
            and stat.S_IMODE(credential.stat().st_mode) == 0o600,
            'credential must be a regular 0600 file with no symlink components')
    require(sha256_file(credential) == record['credential_sha256'], 'credential file hash mismatch')
    require(no_symlink_components(evidence) and evidence.is_dir()
            and stat.S_IMODE(evidence.stat().st_mode) == 0o700 and not any(evidence.iterdir()),
            'private evidence directory must be empty, 0700, and contain no symlinks')
    require(not credential.resolve().is_relative_to(REPO) and not evidence.resolve().is_relative_to(REPO),
            'credential and evidence paths must be outside checkout')
    require(not credential.resolve().is_relative_to(evidence.resolve()),
            'credential and evidence paths must be disjoint')
    require(no_symlink_components(codex) and codex.is_file()
            and sha256_file(codex) == record['codex_executable_sha256'],
            'Codex executable path or hash mismatch')
    return True
