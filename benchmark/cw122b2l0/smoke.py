"""CW-122B2-L0 runner; live execution needs a valid single-use authorization."""
import hashlib
import argparse
import json
import os
import signal
from pathlib import Path
import re
import stat
import shutil
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from datetime import datetime, timezone

ROOT = Path(__file__).resolve().parents[2]
PRICING = ROOT / 'benchmark/cw122b2p/pricing-2026-11-05.json'
HASH = re.compile(r'^[0-9a-f]{64}$')
SMOKE_SESSION_PREFIX = 'cw122b2l0-'
SESSION_ID_CHARS = re.compile(r'^[A-Za-z0-9_-]+$')
FIELDS = {
    'authorization_id', 'approver_identity', 'approval_reference', 'issued_at', 'expires_at',
    'provider', 'model', 'endpoint', 'processing_tier', 'runner_sha256', 'pricing_sha256',
    'codex_executable_sha256', 'cw_executable_sha256', 'request_cap', 'deadline_seconds',
    'spend_ceiling_usd', 'reservation_overrun_risk_accepted', 'credential_file',
    'credential_sha256',
    'private_evidence_directory', 'isolation_profile', 'codex_executable_path',
    'cw_executable_path', 'sandbox_executable_path', 'sandbox_executable_sha256',
    'authorization_claim_directory',
}


def require(ok, reason):
    if not ok:
        raise ValueError(reason)


def no_symlink_components(path):
    path = Path(path)
    require(path.is_absolute() and all(part not in ('.', '..') for part in path.parts),
            'path must be absolute and normalized')
    current = Path('/')
    for part in path.parts[1:]:
        current /= part
        try:
            if current.is_symlink():
                return False
        except OSError:
            return False
    return True


def sha256(path):
    h = hashlib.sha256()
    with Path(path).open('rb') as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            h.update(block)
    return h.hexdigest()


def validate_credential_not_exposed(record, host_home):
    credential = Path(record['credential_file']).resolve()
    for key in ('cw_executable_path', 'codex_executable_path'):
        executable = Path(record[key])
        path = executable.resolve(strict=True)
        if path.is_relative_to(host_home):
            root = Path(os.path.commonpath((str(executable.parent.resolve()), str(path))))
            require(not credential.is_relative_to(root),
                    'provider credential cannot be under an exposed executable installation root')


def runner_sha256():
    components = (
        Path(__file__),
        ROOT / 'benchmark/cw122b1/run_mock_campaign.py',
        ROOT / 'benchmark/cw122a/offline_env.py',
        ROOT / 'benchmark/cw122a/rehearse.py',
        ROOT / 'benchmark/cw122a/analyze.py',
        ROOT / 'benchmark/cw122a/oracle.py',
    )
    digest = hashlib.sha256()
    for path in components:
        relative = path.resolve().relative_to(ROOT).as_posix().encode()
        digest.update(len(relative).to_bytes(4, 'big'))
        digest.update(relative)
        digest.update(bytes.fromhex(sha256(path)))
    return digest.hexdigest()


def validate(record, selection, now=None):
    require(isinstance(record, dict) and set(record) == FIELDS, 'authorization fields missing or unknown')
    require(all(isinstance(record[k], str) and record[k].strip() for k in
                ('authorization_id', 'approver_identity', 'approval_reference')), 'approval identity/reference required')
    auth_id = record['authorization_id']
    require(SESSION_ID_CHARS.fullmatch(auth_id) is not None
            and len(SMOKE_SESSION_PREFIX) + len(auth_id) <= 128,
            'authorization ID does not fit the authenticated CW session ID contract')
    try:
        issued = datetime.fromisoformat(record['issued_at'].replace('Z', '+00:00'))
        expires = datetime.fromisoformat(record['expires_at'].replace('Z', '+00:00'))
        now = now or datetime.now(timezone.utc)
        require(issued.tzinfo and expires.tzinfo and issued <= now < expires, 'authorization expired or not yet valid')
        require(0 < (expires-issued).total_seconds() <= 86400, 'authorization lifetime exceeds 24 hours')
    except (TypeError, AttributeError, ValueError) as exc:
        raise ValueError('invalid authorization time window') from exc
    expected = {'provider':'openai', 'model':'gpt-6-luna', 'endpoint':'https://api.openai.com/v1',
                'processing_tier':'standard', 'pricing_sha256':sha256(PRICING),
                'isolation_profile':'bubblewrap-read-only-hidden-provider-credential-v1'}
    expected.update(selection)
    for key, value in expected.items():
        require(record.get(key) == value, 'authorization mismatch: ' + key)
    for key in ('runner_sha256','pricing_sha256','codex_executable_sha256','cw_executable_sha256','sandbox_executable_sha256'):
        require(isinstance(record[key],str) and HASH.fullmatch(record[key]), 'invalid hash: '+key)
    require(type(record['request_cap']) is int and record['request_cap'] == 1, 'smoke request cap must be exactly one')
    require(type(record['deadline_seconds']) is int and 1 <= record['deadline_seconds'] <= 900, 'deadline outside 1..900s')
    require(type(record['spend_ceiling_usd']) in (int,float) and 0 < record['spend_ceiling_usd'] <= 5, 'finite smoke spend ceiling required')
    require(record['reservation_overrun_risk_accepted'] is True, 'reservation overrun risk not acknowledged')
    require(isinstance(record['isolation_profile'],str) and record['isolation_profile'], 'isolation profile required')
    credential, evidence = Path(record['credential_file']), Path(record['private_evidence_directory'])
    claims = Path(record['authorization_claim_directory'])
    require(no_symlink_components(credential) and credential.is_file(), 'credential file unavailable or symlinked')
    require(stat.S_IMODE(credential.stat().st_mode) == 0o600, 'credential file must have mode 0600')
    require(isinstance(record['credential_sha256'], str)
            and HASH.fullmatch(record['credential_sha256']) is not None
            and sha256(credential) == record['credential_sha256'],
            'credential contents do not match separately authorized fingerprint')
    require(no_symlink_components(evidence) and evidence.is_dir(), 'private evidence directory unavailable or symlinked')
    require(stat.S_IMODE(evidence.stat().st_mode) == 0o700 and not any(evidence.iterdir()), 'evidence directory must be empty mode 0700')
    require(no_symlink_components(claims) and claims.is_dir()
            and stat.S_IMODE(claims.stat().st_mode) == 0o700,
            'authorization claim directory must exist with mode 0700 and no symlink components')
    require(not claims.resolve().is_relative_to(evidence.resolve())
            and not evidence.resolve().is_relative_to(claims.resolve()),
            'authorization claim and evidence directories must be disjoint')
    volatile_roots = (Path('/tmp'), Path('/var/tmp'), Path('/dev/shm'), Path('/run'))
    require(not any(claims.resolve().is_relative_to(root) for root in volatile_roots),
            'authorization claim directory must be on persistent storage')
    require(not credential.resolve().is_relative_to(ROOT) and not evidence.resolve().is_relative_to(ROOT)
            and not claims.resolve().is_relative_to(ROOT), 'credential/evidence/claim paths must be outside checkout')
    for key in ('codex_executable_path', 'cw_executable_path', 'sandbox_executable_path'):
        require(key in record, 'authorization executable path missing: '+key)
        executable = Path(record[key])
        require(executable.is_absolute() and executable.is_file(), 'selected executable unavailable: '+key)
        expected_hash = {'codex_executable_path':'codex_executable_sha256',
                         'cw_executable_path':'cw_executable_sha256',
                         'sandbox_executable_path':'sandbox_executable_sha256'}[key]
        require(sha256(executable) == record[expected_hash], 'selected executable hash mismatch: '+key)
        require(selection.get(key) == str(executable), 'selected executable path mismatch: '+key)
    validate_credential_not_exposed(record, Path.home().resolve())
    require(runner_sha256() == record['runner_sha256'], 'smoke runner bundle hash mismatch')
    return True


def claim_once(record, claim_root):
    """Durably consume an approval before launch; never remove a claim after failure."""
    root = Path(claim_root)
    require(no_symlink_components(root) and root.is_dir(), 'claim root must exist and contain no symlinks')
    require(stat.S_IMODE(root.stat().st_mode) == 0o700, 'claim root must have mode 0700')
    name = hashlib.sha256(record['authorization_id'].encode()).hexdigest() + '.claimed'
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, 'O_NOFOLLOW', 0)
    fd = os.open(root / name, flags, 0o600)
    try:
        os.write(fd, (record['authorization_id']+'\n').encode())
        os.fsync(fd)
    finally:
        os.close(fd)
    dfd = os.open(root, os.O_RDONLY | getattr(os, 'O_DIRECTORY', 0))
    try:
        os.fsync(dfd)
    finally:
        os.close(dfd)


def authorization_remaining_seconds(record):
    expires = datetime.fromisoformat(record['expires_at'].replace('Z', '+00:00'))
    remaining = (expires - datetime.now(timezone.utc)).total_seconds()
    require(remaining > 0, 'authorization expired before provider execution')
    return remaining


def create_private_evidence(path):
    path = Path(path)
    require(no_symlink_components(path) and not path.exists(), 'evidence path must be new, absolute, and not symlinked')
    path.mkdir(mode=0o700, parents=False)
    require(stat.S_IMODE(path.stat().st_mode) == 0o700, 'unsafe evidence directory mode')
    return path


def write_raw_artifact(evidence, name, data):
    """Exclusive, no-follow, private evidence write; errors must stop dispatch."""
    require(Path(name).name == name and name not in ('.', '..'), 'artifact name must be a basename')
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, 'O_NOFOLLOW', 0)
    fd = os.open(Path(evidence) / name, flags, 0o600)
    digest = hashlib.sha256()
    try:
        view = memoryview(data)
        while view:
            written = os.write(fd, view)
            if written <= 0:
                raise OSError('short evidence write')
            digest.update(view[:written])
            view = view[written:]
        os.fsync(fd)
    finally:
        os.close(fd)
    return digest.hexdigest()


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise urllib.error.HTTPError(req.full_url, code, 'redirect refused', headers, fp)


def mock_exchange(url, payload, evidence, opener=None):
    """Local/mock-only exchange primitive; never follows redirects or sends secrets to logs."""
    from urllib.parse import urlsplit
    parsed = urlsplit(url)
    require(parsed.scheme == 'http' and parsed.hostname in ('127.0.0.1', '::1', 'localhost'),
            'mock exchange only accepts loopback HTTP')
    require('authorization' not in {str(k).lower() for k in payload}, 'provider credentials cannot be captured in payload')
    request_bytes = json.dumps(payload, separators=(',', ':'), ensure_ascii=False).encode()
    request_hash = write_raw_artifact(evidence, 'ingress.json', request_bytes)
    opener = opener or urllib.request.build_opener(NoRedirect)
    request = urllib.request.Request(url, data=request_bytes, headers={'Content-Type':'application/json'}, method='POST')
    try:
        with opener.open(request, timeout=5) as response:
            response_bytes = response.read()
            response_hash = write_raw_artifact(evidence, 'response.bin', response_bytes)
            return response.status, response_bytes, {'ingress_sha256':request_hash,'response_sha256':response_hash}
    except urllib.error.HTTPError as exc:
        exc.close()
        raise
    except BaseException:
        # A failed response remains evidence; failure to persist evidence propagates.
        write_raw_artifact(evidence, 'transport-failure.json', b'{"outcome":"transport_failure"}')
        raise


def build_codex_environment(source, *, control_token, session_id):
    """Minimal Codex-only environment; no host provider credentials are inherited."""
    allowed = ('PATH', 'HOME', 'CARGO_HOME', 'RUSTUP_HOME', 'RUSTUP_TOOLCHAIN', 'TMPDIR', 'TMP', 'TEMP')
    env = {key: source[key] for key in allowed if key in source}
    env.update({
        'CW_PROXY_DUMMY_API_KEY': 'cw-local-proxy-dummy',
        'CW_CONTROL_TOKEN': control_token,
        'CW_SESSION_ID': session_id,
        'LANG': 'C.UTF-8',
        'LC_ALL': 'C.UTF-8',
        'PYTHONDONTWRITEBYTECODE': '1',
    })
    return env


def hidden_paths(home, credential):
    paths = [Path(home).resolve(), Path(credential).resolve().parent]
    require(all(path != Path('/') and path.is_absolute() for path in paths), 'cannot hide a root directory')
    unique = []
    for path in sorted(set(paths), key=lambda item: len(item.parts)):
        if not any(path == parent or path.is_relative_to(parent) for parent in unique):
            unique.append(path)
    return unique


def verify_bubblewrap(sandbox, home, credential):
    paths = hidden_paths(home, credential)
    home = Path(home).resolve()
    credential = Path(credential).resolve()
    home_probe = next(iter(home.iterdir()), None)
    require(home_probe is not None, 'host home must contain an entry to verify isolation')
    argv = [str(sandbox), '--ro-bind', '/', '/', '--dev', '/dev', '--proc', '/proc', '--unshare-pid']
    for path in paths:
        argv.extend(['--tmpfs', str(path)])
    check = 'import os,sys; raise SystemExit(any(os.path.exists(p) for p in sys.argv[1:]))'
    result = subprocess.run(argv + [sys.executable, '-c', check, str(home_probe), str(credential)],
                            env={'PATH':'/usr/bin:/bin','HOME':str(home),
                                 'LANG':'C.UTF-8','LC_ALL':'C.UTF-8'},
                            stdin=subprocess.DEVNULL, capture_output=True, timeout=10)
    require(result.returncode == 0, 'bubblewrap cannot isolate host home and provider credential')
    return paths


def private_runtime_root(credential, host_home):
    hidden = hidden_paths(host_home, credential)
    for parent in (Path('/var/tmp'), Path('/tmp'), Path('/dev/shm')):
        try:
            runtime = Path(tempfile.mkdtemp(prefix='cw122b2l0-', dir=parent))
            if any(runtime == path or runtime.is_relative_to(path) for path in hidden):
                shutil.rmtree(runtime)
                continue
            runtime.chmod(0o700)
            return runtime
        except OSError:
            continue
    raise ValueError('no private executable temporary directory outside the isolation mounts')


def free_loopback_port():
    with socket.socket() as listener:
        listener.bind(('127.0.0.1', 0))
        return listener.getsockname()[1]


def wait_for_loopback(port, process, deadline):
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError('CW daemon exited before its proxy became ready')
        try:
            with socket.create_connection(('127.0.0.1', port), timeout=0.2):
                return
        except OSError:
            time.sleep(0.1)
    raise TimeoutError('CW proxy readiness deadline expired')


def bind_executable(argv, executable, sandbox_name, host_home):
    executable = Path(executable)
    resolved = executable.resolve(strict=True)
    if not resolved.is_relative_to(host_home):
        return str(resolved)
    root = Path(os.path.commonpath((str(executable.parent.resolve()), str(resolved))))
    require(root != host_home and root != Path('/'), 'unsafe executable installation root')
    if '--tmpfs' not in argv or '/opt' not in argv:
        argv.extend(['--tmpfs','/opt'])
    target = Path('/opt') / sandbox_name
    argv.extend(['--dir',str(target),'--ro-bind',str(root),str(target)])
    return str(target / executable.relative_to(root))


def codex_sandbox_command(record, runtime, workspace, session_id, proxy_port):
    host_home = Path.home().resolve()
    validate_credential_not_exposed(record, host_home)
    argv = [record['sandbox_executable_path'], '--ro-bind', '/', '/', '--dev', '/dev',
            '--proc', '/proc', '--unshare-pid']
    cw_command = bind_executable(argv, record['cw_executable_path'], 'cw-dist', host_home)
    codex_command = bind_executable(argv, record['codex_executable_path'], 'codex-dist', host_home)
    for path in hidden_paths(host_home, record['credential_file']):
        argv.extend(['--tmpfs', str(path)])
    home = runtime / 'cw-home'
    codex_home = runtime / 'codex-state'
    temp = runtime / 'codex-tmp'
    for directory in (home, codex_home, temp):
        if not directory.exists():
            directory.mkdir(mode=0o700)
    for directory in (home, codex_home, temp):
        argv.extend(['--bind', str(directory), str(directory)])
    argv.extend(['--ro-bind', str(workspace), str(workspace), '--chdir', str(workspace),
                 '--setenv', 'HOME', str(home), '--setenv', 'CODEX_HOME', str(codex_home),
                 '--setenv', 'TMPDIR', str(temp), '--setenv', 'PATH', '/opt/cw-dist:/opt/cw-dist/bin:/opt/codex-dist/bin:/usr/local/bin:/usr/bin:/bin',
                 '--setenv', 'LANG', 'C.UTF-8', '--setenv', 'LC_ALL', 'C.UTF-8',
                 '--setenv', 'CW_L0_SMOKE_DUMMY_AUTH', '1',
                 '--setenv', 'CW_L0_ALLOWED_SESSION_ID', session_id,
                 '--setenv', 'CW_L0_AUTH_EXPIRES_AT', record['expires_at'],
                 '--setenv', 'CW_PROXY_DUMMY_API_KEY', 'cw-local-proxy-dummy',
                 '--', cw_command, 'wrap', '--proxy-port', str(proxy_port), '--session-id', session_id,
                 '--', codex_command, 'exec', '--ignore-user-config', '--ignore-rules',
                 '--sandbox', 'read-only', '--cd', str(workspace), '--model', record['model'], '--json',
                 'Reply exactly with CW-L0-SMOKE-OK. Do not use tools.'])
    return argv, codex_home


def load_fault_controls():
    path = str(ROOT / 'benchmark/cw122b1')
    sys.path.insert(0, path)
    try:
        from run_mock_campaign import install_termination_handlers, kill_process_tree, restore_signal_handlers, run_bounded
        return install_termination_handlers, kill_process_tree, restore_signal_handlers, run_bounded
    finally:
        sys.path.remove(path)


def write_private_json(evidence, name, value):
    return write_raw_artifact(evidence, name, json.dumps(value, sort_keys=True, indent=2).encode())


def copy_private_file(source, evidence, name):
    return write_raw_artifact(evidence, name, Path(source).read_bytes())


def provider_response_models(response_bytes):
    models = set()
    candidates = [response_bytes]
    if b'data:' in response_bytes:
        candidates = [line[5:].strip() for line in response_bytes.splitlines()
                      if line.startswith(b'data:') and line[5:].strip() != b'[DONE]']
    for candidate in candidates:
        try:
            value = json.loads(candidate)
        except (UnicodeDecodeError, json.JSONDecodeError):
            continue
        response = value.get('response') if isinstance(value, dict) else None
        model = response.get('model') if isinstance(response, dict) else None
        if model is None and isinstance(value, dict):
            model = value.get('model')
        if isinstance(model, str) and model:
            models.add(model)
    return sorted(models)


def provider_response_service_tiers(response_bytes):
    tiers = set()
    candidates = [response_bytes]
    if b'data:' in response_bytes:
        candidates = [line[5:].strip() for line in response_bytes.splitlines()
                      if line.startswith(b'data:') and line[5:].strip() != b'[DONE]']
    for candidate in candidates:
        try:
            value = json.loads(candidate)
        except (UnicodeDecodeError, json.JSONDecodeError):
            continue
        response = value.get('response') if isinstance(value, dict) else None
        tier = response.get('service_tier') if isinstance(response, dict) else None
        if isinstance(tier, str) and tier:
            tiers.add(tier)
    return sorted(tiers)


def collect_live_evidence(database, evidence, codex_home, expected_model=None):
    database = Path(database)
    require(database.is_file(), 'CW smoke database was not created')
    transcript_copy = Path(evidence) / 'codex-home'
    if codex_home is not None and Path(codex_home).exists():
        shutil.copytree(codex_home, transcript_copy, symlinks=False)
        for root, dirs, files in os.walk(transcript_copy):
            os.chmod(root, 0o700)
            for name in files:
                path = Path(root) / name
                require(path.is_file() and not path.is_symlink(), 'unexpected non-regular Codex evidence file')
                os.chmod(path, 0o600)
    conn = sqlite3.connect(database)
    try:
        checkpoint = conn.execute('PRAGMA wal_checkpoint(TRUNCATE)').fetchone()
        require(checkpoint is not None and checkpoint[0] == 0, 'CW database WAL checkpoint incomplete')
        rows = conn.execute('SELECT id,turn_json FROM turns ORDER BY created_at,id').fetchall()
        results = []
        issues = []
        for index, (turn_id, turn_raw) in enumerate(rows):
            turn = json.loads(turn_raw)
            wire = {}
            bodies = {}
            response_models = []
            response_tiers = []
            directory = Path(evidence) / f'request-{index:02d}'
            directory.mkdir(mode=0o700)
            for kind, query in (
                ('ingress', 'SELECT body FROM wire_requests WHERE turn_id=?'),
                ('egress', 'SELECT body FROM wire_egress WHERE turn_id=?'),
                ('response', 'SELECT body,complete,truncated FROM wire_responses WHERE turn_id=?'),
            ):
                found = conn.execute(query, (turn_id,)).fetchone()
                if found is None:
                    wire[kind] = {'available':False,'complete':False if kind == 'response' else None,
                                  'truncated':None}
                    issues.append(f'request {index}: missing {kind} evidence')
                    continue
                body = bytes(found[0])
                bodies[kind] = body
                if kind == 'response':
                    response_models = provider_response_models(body)
                    response_tiers = provider_response_service_tiers(body)
                    if expected_model is not None and response_models != [expected_model]:
                        issues.append(f'request {index}: provider response model missing or mismatched')
                    if expected_model is not None and response_tiers != ['default']:
                        issues.append(f'request {index}: effective provider service tier missing or not standard-priced')
                complete, truncated = (bool(found[1]), bool(found[2])) if kind == 'response' else (None, None)
                wire[kind] = {'sha256':hashlib.sha256(body).hexdigest(),'bytes':len(body),
                              'complete':complete,'truncated':truncated}
                write_raw_artifact(directory, f'{kind}.bin', body)
                if kind == 'response':
                    if not complete or truncated:
                        issues.append(f'request {index}: provider response incomplete or truncated')
            write_private_json(Path(evidence) / f'request-{index:02d}', 'turn.json', turn)
            usage = turn.get('observed_usage')
            if not isinstance(usage, dict):
                issues.append(f'request {index}: provider usage telemetry missing')
                usage_record = {'status':'NOT_AVAILABLE'}
            else:
                details = usage.get('input_tokens_details')
                complete_usage = (isinstance(details, dict)
                    and all(type(details.get(key)) is int for key in (
                        'cached_tokens','cache_write_tokens'))
                    and isinstance(details.get('cache_write_tokens_details'), dict)
                    and all(type(details['cache_write_tokens_details'].get(key)) is int for key in (
                        'ttl_30m_tokens','ttl_12h_tokens')))
                usage_record = usage
                if not complete_usage:
                    issues.append(f'request {index}: provider cache usage telemetry incomplete')
            write_private_json(Path(evidence) / f'request-{index:02d}', 'usage.json', usage_record)
            ingress_json = egress_json = None
            try:
                if 'ingress' in bodies:
                    ingress_json = json.loads(bodies['ingress'])
                if 'egress' in bodies:
                    egress_json = json.loads(bodies['egress'])
            except (UnicodeDecodeError, json.JSONDecodeError):
                issues.append(f'request {index}: wire body is not valid JSON')
            unchanged = ingress_json is not None and egress_json is not None and ingress_json == egress_json
            if not unchanged:
                issues.append(f'request {index}: ingress/egress equality unavailable or false')
            if expected_model is not None and isinstance(egress_json, dict):
                requested_tier = egress_json.get('service_tier')
                if requested_tier not in (None, 'default'):
                    issues.append(f'request {index}: request service tier conflicts with standard pricing authorization')
            results.append({'turn_id':turn_id,'turn':turn,'wire':wire,
                            'ingress_json':ingress_json,'egress_json':egress_json,
                            'request_body_unchanged':unchanged,
                            'provider_response_models':response_models,
                            'provider_response_model_matches':expected_model is not None and response_models == [expected_model],
                            'provider_response_service_tiers':response_tiers,
                            'provider_response_service_tier_matches':expected_model is not None and response_tiers == ['default']})
        ledger = conn.execute(
            'SELECT turn_id,session_id,provider,model,admission_status,reservation_usd,settlement_status,settled_usd,usage_json,error '
            'FROM spend_guard_ledger ORDER BY updated_at,turn_id'
        ).fetchall()
        if len(ledger) != len(rows):
            issues.append('SpendGuard ledger and provider turn count disagree')
        ledger_records = [dict(zip((
            'turn_id','session_id','provider','model','admission_status','reservation_usd',
            'settlement_status','settled_usd','usage_json','error'), row)) for row in ledger]
        write_private_json(evidence, 'spend-guard-ledger.json', ledger_records)
        write_private_json(evidence, 'evidence-issues.json', issues)
    finally:
        conn.close()
        shutil.copy2(database, Path(evidence) / 'cw.db')
        os.chmod(Path(evidence) / 'cw.db', 0o600)
        for suffix in ('-wal', '-shm'):
            sidecar = Path(str(database) + suffix)
            if sidecar.is_file():
                shutil.copy2(sidecar, Path(evidence) / f'cw.db{suffix}')
                os.chmod(Path(evidence) / f'cw.db{suffix}', 0o600)
    return results, ledger_records, issues


def run_authorized_smoke(record, *, evidence, cw_executable, codex_executable):
    evidence = Path(evidence)
    credential = Path(record['credential_file'])
    host_home = Path.home().resolve()
    verify_bubblewrap(record['sandbox_executable_path'], host_home, credential)
    install_handlers, kill_tree, restore_handlers, run_bounded = load_fault_controls()
    authorization_deadline = time.monotonic() + authorization_remaining_seconds(record)
    claim_once(record, record['authorization_claim_directory'])
    write_private_json(evidence, 'authorization-record.json', record)
    started = time.monotonic()
    deadline = min(started + record['deadline_seconds'], authorization_deadline)
    runtime = None
    database = None
    cw_log = None
    server = None
    previous_handlers = None
    result_status = 'INCONCLUSIVE'
    codex_exit = None
    result_code = None
    codex_output = b''
    records = []
    ledger = []
    evidence_issues = ['provider evidence collection not completed']
    codex_home = None
    runner_error = None
    cleanup = {'verified':True,'processes':[]}
    try:
        runtime = private_runtime_root(credential, host_home)
        for name in ('cw-home','cw-tmp','workspace'):
            (runtime / name).mkdir(mode=0o700)
        workspace = runtime / 'workspace'
        (workspace / 'README.txt').write_text('This directory is an isolated one-request smoke workspace.\n')
        os.chmod(workspace / 'README.txt', 0o600)
        database = runtime / 'cw.db'
        port, api_port = free_loopback_port(), free_loopback_port()
        while port == api_port:
            api_port = free_loopback_port()
        cw_env = {
            'PATH':'/usr/bin:/bin', 'HOME':str(runtime / 'cw-home'), 'TMPDIR':str(runtime / 'cw-tmp'),
            'LANG':'C.UTF-8','LC_ALL':'C.UTF-8','RUST_LOG':'warn',
            'CW_OPENAI_API_KEY_FILE':str(credential), 'CW_CAPTURE_WIRE_EGRESS':'1',
            'CW_CAPTURE_WIRE_RESPONSE':'1','CW_L0_ALLOWED_MODEL':record['model'],
            'CW_L0_ALLOWED_SESSION_ID':SMOKE_SESSION_PREFIX + record['authorization_id'],
            'CW_L0_AUTH_EXPIRES_AT':record['expires_at'],
            'CW_L0_SMOKE_SPEND_CEILING_USD':str(record['spend_ceiling_usd']),
            'CW_L0_SMOKE_REQUEST_CAP':str(record['request_cap']),
            'CW_UPSTREAM_TIMEOUT_SECS':str(min(record['deadline_seconds'],300)),
        }
        token = subprocess.run([str(cw_executable), '--db', str(database), 'auth-token'],
                               env=cw_env, stdin=subprocess.DEVNULL, capture_output=True,
                               timeout=min(record['deadline_seconds'],10), check=True).stdout.decode().strip()
        require(re.fullmatch(r'[A-Za-z0-9_-]{32,128}', token) is not None, 'CW control credential format invalid')
        cw_log = runtime / 'cw-server.log'
        previous_handlers = install_handlers()
        with cw_log.open('xb') as log:
            os.chmod(cw_log, 0o600)
            server = subprocess.Popen(
                [str(cw_executable),'--db',str(database),'serve','--port',str(api_port),
                 '--proxy-port',str(port),'--workspace',str(workspace)],
                cwd=workspace, env=cw_env, stdin=subprocess.DEVNULL, stdout=log,
                stderr=subprocess.STDOUT, start_new_session=True,
            )
            wait_for_loopback(port, server, deadline)
            session = SMOKE_SESSION_PREFIX + record['authorization_id']
            argv, codex_home = codex_sandbox_command(
                record, runtime, workspace, session, port)
            try:
                result_code, codex_output = run_bounded(
                    argv, workspace,
                    {'PATH':'/usr/bin:/bin','HOME':str(runtime / 'codex-home')},
                    max(0.1, deadline-time.monotonic()), cancel=kill_tree)
            except subprocess.TimeoutExpired as timeout:
                result_code, codex_output = 124, timeout.output or b''
            codex_exit = result_code
            result_status = 'FAIL' if result_code != 0 else 'INCONCLUSIVE'
    except BaseException as exc:
        result_status = 'INCONCLUSIVE'
        runner_error = {'error':type(exc).__name__,'detail':str(exc)}
        if isinstance(exc, subprocess.TimeoutExpired):
            codex_output = exc.output or b''
            codex_exit = 124
            result_code = 124
    finally:
        if server is not None and server.poll() is None:
            try:
                cleanup_result = kill_tree(server.pid)
                cleanup['processes'].append(cleanup_result)
                cleanup['verified'] = cleanup['verified'] and cleanup_result['verified']
                server.wait(timeout=5)
            except Exception as exc:
                cleanup['verified'] = False
                cleanup['errors'] = [type(exc).__name__]
        if previous_handlers is not None:
            restore_handlers(previous_handlers)
    write_raw_artifact(evidence, 'codex-events.jsonl', codex_output)
    if runner_error is not None:
        write_private_json(evidence, 'runner-error.json', runner_error)
    if cw_log is not None and cw_log.is_file():
        copy_private_file(cw_log, evidence, 'cw-server.log')
    if database is not None and database.is_file():
        try:
            records, ledger, evidence_issues = collect_live_evidence(
                database, evidence, codex_home, expected_model=record['model'])
            if evidence_issues:
                result_status = 'INCONCLUSIVE'
            if any(
                (item['provider_response_models']
                 and item['provider_response_models'] != [record['model']])
                or (item['provider_response_service_tiers']
                    and item['provider_response_service_tiers'] != ['default'])
                for item in records
            ) or any('request service tier conflicts' in issue for issue in evidence_issues):
                result_status = 'FAIL'
        except Exception as exc:
            write_private_json(evidence, 'evidence-error.json', {'error':type(exc).__name__,'detail':str(exc)})
            result_status = 'INCONCLUSIVE'
    if (result_code == 0 and len(records) == 1 and len(ledger) == 1
            and ledger[0]['admission_status'] == 'ADMITTED'
            and ledger[0]['settlement_status'] == 'SETTLED'
            and ledger[0]['provider'] == 'OpenAI'
            and ledger[0]['model'] == record['model']
            and ledger[0]['reservation_usd'] == record['spend_ceiling_usd']
            and ledger[0]['settled_usd'] is not None
            and ledger[0]['settled_usd'] <= record['spend_ceiling_usd']
            and records[0]['turn'].get('model') == record['model']
            and records[0]['request_body_unchanged']
            and records[0]['turn'].get('outcome') == 'completed'
            and b'CW-L0-SMOKE-OK' in codex_output
            and cleanup['verified'] and runner_error is None and not evidence_issues):
        result_status = 'PASS'
    hashes = {}
    for path in evidence.rglob('*'):
        if path.is_file() and not path.is_symlink():
            hashes[str(path.relative_to(evidence))] = sha256(path)
    manifest = {
        'experiment':'CW-122B2-L0-SMOKE','status':result_status,
        'provider_request_records':len(records),'provider_spend_usd':ledger[0].get('settled_usd') if len(ledger)==1 else None,
        'evidence_issues':evidence_issues,
        'model':record['model'],'endpoint':record['endpoint'],'request_cap':record['request_cap'],
        'codex_exit_code':codex_exit,'elapsed_seconds':round(time.monotonic()-started,3),
        'cleanup':cleanup,'transcript_directory':'codex-home','artifact_sha256':hashes,
    }
    write_private_json(evidence, 'run-manifest.json', manifest)
    if runtime is not None:
        shutil.rmtree(runtime, ignore_errors=True)
    return manifest


def run_mock_codex(evidence, *, control_token, session_id, timeout_seconds=300, command=None):
    """Run the prequalified mock-only Codex test; this function has no provider URL."""
    evidence = Path(evidence)
    require(evidence.is_absolute() and evidence.is_dir() and stat.S_IMODE(evidence.stat().st_mode) == 0o700,
            'private evidence directory must be absolute and mode 0700')
    # Fail before any child process is launched if private evidence cannot be written.
    write_raw_artifact(evidence, 'runner-started.json', b'{"mode":"MOCK_ONLY"}')
    codex = shutil.which('codex')
    require(codex is not None, 'Codex CLI unavailable')
    env = build_codex_environment(os.environ, control_token=control_token, session_id=session_id)
    env['CW122B1_EVIDENCE_DIR'] = str(evidence)
    env['CW122B1_COMMIT_SHA'] = subprocess.run(['git','rev-parse','HEAD'], cwd=ROOT, env=env,
        check=True, capture_output=True, text=True, timeout=5).stdout.strip()
    env['CW122B1_CW122A_FREEZE_SHA256'] = hashlib.sha256((ROOT/'benchmark/cw122a/freeze.json').read_bytes()).hexdigest()
    argv = command or ['cargo','test','--offline','--locked','-p','cw-proxy','--test','qualification',
        'test_cw122b1_codex_mock_campaign','--','--ignored','--exact','--nocapture']
    start = time.monotonic()
    proc = subprocess.Popen(argv, cwd=ROOT, env=env, stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, start_new_session=True)
    status, output, exit_code = 'RUNNING', b'', None
    try:
        output, _ = proc.communicate(timeout=timeout_seconds)
        exit_code = proc.returncode
        status = 'PASSED' if exit_code == 0 else 'FAILED'
    except subprocess.TimeoutExpired:
        status = 'TIMEOUT'
        os.killpg(proc.pid, signal.SIGKILL)
        output, _ = proc.communicate()
        exit_code = proc.returncode
    except BaseException as exc:
        status = f'INTERRUPTED:{type(exc).__name__}'
        os.killpg(proc.pid, signal.SIGKILL)
        output, _ = proc.communicate()
        exit_code = proc.returncode
    write_raw_artifact(evidence, 'mock-runner-output.bin', output)
    version = subprocess.run([codex,'--version'],env=env,capture_output=True,text=True,timeout=5).stdout.strip()
    manifest = {'mode':'MOCK_ONLY','provider_calls':0,'provider_spend_usd':0,'status':status,
        'codex_version':version,'exit_code':exit_code,'elapsed_seconds':round(time.monotonic()-start,3),
        'output_sha256':hashlib.sha256(output).hexdigest(),'live_mode':'SEPARATE_AUTHORIZATION_REQUIRED'}
    write_raw_artifact(evidence,'mock-run-manifest.json',json.dumps(manifest,sort_keys=True).encode())
    return manifest


def read_authorization(path):
    path = Path(path)
    require(no_symlink_components(path) and path.is_file(),
            'authorization must be an absolute regular file')
    require(stat.S_IMODE(path.stat().st_mode) == 0o600, 'authorization file must have mode 0600')
    require(not path.resolve().is_relative_to(ROOT), 'authorization must be outside checkout')
    return json.loads(path.read_bytes())


def selection_from_args(args, record):
    require(args.codex_executable is not None and args.cw_executable is not None
            and args.sandbox_executable is not None,
            'Codex, CW and sandbox executable paths must be supplied')
    return {
        'runner_sha256':runner_sha256(),
        'codex_executable_sha256':record.get('codex_executable_sha256'),
        'cw_executable_sha256':record.get('cw_executable_sha256'),
        'codex_executable_path':str(args.codex_executable),
        'cw_executable_path':str(args.cw_executable),
        'sandbox_executable_path':str(args.sandbox_executable),
        'credential_file':record.get('credential_file'),
        'private_evidence_directory':record.get('private_evidence_directory'),
        'authorization_claim_directory':record.get('authorization_claim_directory'),
    }


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--authorization', type=Path)
    parser.add_argument('--codex-executable', type=Path)
    parser.add_argument('--cw-executable', type=Path)
    parser.add_argument('--sandbox-executable', type=Path)
    parser.add_argument('--check', action='store_true', help='validate only; never claim or launch')
    parser.add_argument('--run', action='store_true', help='consume a separate smoke authorization and run once')
    parser.add_argument('--mock-only', action='store_true', help='run the prequalified mock Codex test only')
    parser.add_argument('--evidence-dir', type=Path)
    args = parser.parse_args(argv)
    if args.mock_only:
        require(args.evidence_dir is not None, '--mock-only requires --evidence-dir')
        evidence = create_private_evidence(args.evidence_dir)
        result = run_mock_codex(evidence, control_token='offline-control-token-00000000000000000000000000000000',
                                session_id='cw122b2l0-mock-only')
        print(json.dumps(result, sort_keys=True))
        return result['exit_code']
    if args.check == args.run or args.authorization is None:
        raise SystemExit('NO_GO: choose exactly one of --check or --run and provide separate smoke authorization')
    try:
        record = read_authorization(args.authorization)
        selection = selection_from_args(args, record)
        validate(record, selection)
    except Exception as exc:
        raise SystemExit(f'NO_GO: authorization preflight failed: {type(exc).__name__}: {exc}') from exc
    if args.check:
        print(json.dumps({'preflight':'PASS','launch':'NOT_EXECUTED','provider_calls':0}, sort_keys=True))
        return 0
    result = run_authorized_smoke(record, evidence=record['private_evidence_directory'],
                                  cw_executable=args.cw_executable,
                                  codex_executable=args.codex_executable)
    print(json.dumps(result, sort_keys=True))
    return 0 if result['status'] == 'PASS' else 1


if __name__ == '__main__':
    raise SystemExit(main())
