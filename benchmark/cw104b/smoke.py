"""Offline fixture preflight by default; live execution requires fresh authorization."""
import argparse
import copy
import hashlib
import ipaddress
import json
import os
from pathlib import Path
import shutil
import socket
import sqlite3
import subprocess
import threading
import time
import urllib.request
import urllib.parse
import uuid
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from acceptance import check

B = Path(__file__).resolve().parent
ROOT = B.parents[1]
RUNNER_FREEZE = B / 'runner-freeze.json'
OFFLINE_FIXTURE = B / 'fixtures/offline-lifecycle.json'


class NoProviderRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise ValueError('Provider redirects are disabled for local-only qualification')


def local_endpoint(value):
    parsed = urllib.parse.urlsplit(value)
    networks = ('10.0.0.0/8', '172.16.0.0/12', '192.168.0.0/16', '127.0.0.0/8', 'fc00::/7', '::1/128')
    try:
        address = ipaddress.ip_address(parsed.hostname or '')
        local = any(address in ipaddress.ip_network(network) for network in networks)
        parsed.port  # Validate the port without opening a connection.
    except ValueError:
        local = False
    if (not local or parsed.scheme not in ('http', 'https') or parsed.username is not None
            or parsed.password is not None or parsed.path not in ('', '/') or parsed.query or parsed.fragment):
        raise ValueError('Endpoint must be a bare private/loopback IP HTTP(S) origin without credentials')
    return value.rstrip('/')


def verify_runner_freeze():
    frozen = json.loads(RUNNER_FREEZE.read_text())
    for name, expected in frozen['files'].items():
        if hashlib.sha256((ROOT / name).read_bytes()).hexdigest() != expected:
            raise ValueError('Runner/input freeze mismatch: ' + name)
    return hashlib.sha256(RUNNER_FREEZE.read_bytes()).hexdigest()


def live_authorization(options, freeze_sha256, now=None):
    """Validate an operator's attestation; this is not cryptographic identity proof."""
    if not options.live or not options.authorization_file:
        raise ValueError('Live inference requires --live and a fresh --authorization-file')
    endpoint = local_endpoint(options.endpoint)
    try:
        raw_document = options.authorization_file.read_bytes()
        document = json.loads(raw_document)
        allowed = {'authorization_id', 'approved_by', 'approval_reference', 'issued_at', 'expires_at',
                   'scope', 'max_pairs', 'cloud_spend_allowed', 'max_seconds', 'max_provider_requests',
                   'endpoint', 'model', 'provider', 'runner_freeze_sha256', 'evidence_dir'}
        if not isinstance(document, dict) or set(document) != allowed:
            raise ValueError('Unexpected or missing authorization fields')
        issued = datetime.fromisoformat(document['issued_at'].replace('Z', '+00:00'))
        expires = datetime.fromisoformat(document['expires_at'].replace('Z', '+00:00'))
    except (KeyError, ValueError, TypeError, AttributeError):
        raise ValueError('Invalid live authorization document') from None
    now = now or datetime.now(timezone.utc)
    if (issued.tzinfo is None or expires.tzinfo is None or not issued <= now < expires
            or not 0 < (expires - issued).total_seconds() <= 86400):
        raise ValueError('Live authorization must be current and expire within 24 hours of issuance')
    for field in ['authorization_id', 'approved_by', 'approval_reference']:
        if not isinstance(document.get(field), str) or not document[field].strip() or len(document[field]) > 256:
            raise ValueError('Authorization needs a nonempty bounded ' + field)
    if (document.get('scope') != 'one-local-m1-m2-smoke-pair'
            or type(document.get('max_pairs')) is not int or document['max_pairs'] != 1
            or document.get('cloud_spend_allowed') is not False
            or type(document.get('max_seconds')) is not int or not 0 < document['max_seconds'] <= 3600
            or type(document.get('max_provider_requests')) is not int or not 0 < document['max_provider_requests'] <= 30):
        raise ValueError('Authorization must bound one local pair to at most 3600 seconds/30 provider requests with no cloud spend')
    if (document.get('endpoint') != endpoint or document.get('model') != options.model
            or document.get('provider') != options.provider
            or document.get('runner_freeze_sha256') != freeze_sha256
            or document.get('evidence_dir') != str(options.evidence_dir.resolve())):
        raise ValueError('Authorization does not match the endpoint, model, provider, evidence directory, or runner freeze')
    # Do not expose the private evidence path in a shareable manifest.
    record = {key: value for key, value in document.items() if key != 'evidence_dir'}
    record.update(record_valid=True, consumed=False,
                  document_sha256=hashlib.sha256(raw_document).hexdigest())
    return record


def claim_authorization(evidence_dir, authorization):
    used = evidence_dir / 'used-authorizations'
    used.mkdir(mode=0o700, exist_ok=True)
    marker = used / (hashlib.sha256(authorization['authorization_id'].encode()).hexdigest() + '.json')
    try:
        fd = os.open(marker, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    except FileExistsError:
        raise ValueError('Live authorization has already been consumed; obtain fresh approval') from None
    with os.fdopen(fd, 'w') as output:
        json.dump(dict(authorization, consumed=True), output)
        output.flush()
        os.fsync(output.fileno())
    authorization['consumed'] = True


def model_profile(options):
    text = options.model_profile.read_text()
    try:
        if options.model_profile.suffix.lower() == '.json':
            config = json.loads(text)
        else:
            import yaml
            config = yaml.safe_load(text)
    except ImportError:
        raise ValueError('YAML profiles require PyYAML; JSON profiles use the standard library') from None
    except Exception:
        raise ValueError('Model profile is not valid JSON/YAML') from None
    if not isinstance(config, dict):
        raise ValueError('Model profile must be an object')
    providers = config.get('providers')
    source = providers.get(options.provider) if isinstance(providers, dict) else None
    if not isinstance(source, dict):
        raise ValueError('Explicit provider is missing from the model profile')
    models = source.get('models')
    if not isinstance(models, list) or any(not isinstance(model, dict) for model in models):
        raise ValueError('Model profile requires a list of model definitions')
    selected = [model for model in models if model.get('id') == options.model]
    if (source.get('api') != 'openai-completions' or selected != [{'id': options.model}]
            or set(source) - {'api', 'apiKey', 'baseUrl', 'models'}):
        raise ValueError('Only the qualified minimal OpenAI-completions profile/model is supported')
    key = source.get('apiKey')
    if options.live and (not isinstance(key, str) or not key.strip()):
        raise ValueError('The explicit live profile requires an in-process API key')
    return {'api': source['api'], 'models': selected}, key


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True).encode()).hexdigest()


def free_port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


def run_owner(options, result, public):
    mock_only = not options.live
    model = options.model
    endpoint = local_endpoint(options.endpoint) if options.live else None
    started = time.monotonic()
    phase_start = os.environ.get('CW104B_PHASE_START_UTC')
    elapsed_before = (datetime.now(timezone.utc) - datetime.strptime(phase_start, '%Y%m%dT%H%M%SZ').replace(tzinfo=timezone.utc)).total_seconds() if phase_start else 0
    if elapsed_before < 0:
        raise ValueError('Phase start cannot be in the future')
    cap = result['authorization']['max_seconds'] if options.live else 3600
    deadline = started + max(0, cap - elapsed_before)
    if options.live:
        expires = datetime.fromisoformat(result['authorization']['expires_at'].replace('Z', '+00:00'))
        deadline = min(deadline, started + (expires - datetime.now(timezone.utc)).total_seconds())
    stamp = result['started_utc']
    private = options.evidence_dir / result['run_id']
    private.mkdir(mode=0o700)
    result.update({'status': 'PREFLIGHT', 'started_utc': stamp, 'arms': [], 'gate_requests': [],
              'requests': [], 'raw_evidence': private.name, 'hard_cap_seconds': cap, 'original_phase_start_utc': phase_start or stamp, 'elapsed_before_attempt_seconds': elapsed_before, 'provider_requests_admitted': 0})
    lock = threading.Lock()
    state = {'mode': 'mock', 'arm': None, 'mock': [], 'expected': {}, 'fatal': None}
    provider, key = model_profile(options)
    result['model_profile_public_sha256'] = digest(provider)
    protocol = json.loads((B / 'protocol.json').read_text())
    if model != protocol['model']:
        raise ValueError('Model differs from the frozen protocol; review a new protocol before live qualification')
    history = options.history_file.read_text()
    if hashlib.sha256(history.encode()).hexdigest() != protocol['history_sha256']:
        raise RuntimeError('Frozen history mismatch')
    pin = '\n<cw_pinned_context>\n- [CW104B-Constraints]: ' + protocol['families']['f3']['pin'] + '\n</cw_pinned_context>'

    def save():
        public.write_text(json.dumps(result, indent=2) + '\n')

    def raw(name, value):
        path = private / name
        path.write_text(json.dumps(value))
        path.chmod(0o600)

    def remaining():
        value = deadline - time.monotonic()
        if value <= 0:
            raise RuntimeError('60-minute phase deadline reached')
        return value

    def api(path, body=None):
        if not options.live or not result['authorization'].get('consumed'):
            raise RuntimeError('Live endpoint access requires a consumed fresh authorization')
        remaining()
        if path == '/v1/chat/completions':
            if result['provider_requests_admitted'] >= result['authorization']['max_provider_requests']:
                raise RuntimeError('Authorized provider-request budget exhausted')
            result['provider_requests_admitted'] += 1
            save()
        request = urllib.request.Request(endpoint + path,
            data=json.dumps(body).encode() if body is not None else None,
            headers={'Authorization': 'Bearer ' + key, 'Content-Type': 'application/json'})
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoProviderRedirect())
        return opener.open(request, timeout=min(300, remaining()))

    def metrics():
        with api('/metrics') as response:
            return json.load(response)

    def idle(snapshot):
        if snapshot['live']['state'] != 'idle' or snapshot['live'].get('queued'):
            raise RuntimeError('Runtime busy or concurrent inference observed')
        if not snapshot['conversation_cache']['enabled']:
            raise RuntimeError('Cache disabled')

    def after_request(before):
        # The stream may finish just before server accounting becomes idle.
        until = time.monotonic() + 3
        while True:
            after = metrics()
            delta = after['conversation_cache']['requests'] - before['conversation_cache']['requests']
            if delta > 1:
                raise RuntimeError('Concurrent request counter mismatch')
            if delta == 1 and after['live']['state'] == 'idle' and not after['live'].get('queued'):
                return after
            if time.monotonic() >= until:
                raw('runtime-health-failure.json', after)
                raise RuntimeError('Runtime did not return idle with exactly one completed request')
            time.sleep(.1)

    class Relay(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def do_POST(self):
            with lock:
                try:
                    body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
                    if state['fatal']:
                        raise RuntimeError('Earlier request failed; further dispatch forbidden')
                    if state['mode'] == 'mock':
                        state['mock'].append(body)
                        self.send_response(200)
                        self.send_header('Content-Type', 'text/event-stream')
                        self.end_headers()
                        chunks = [dict(id='mock', model=model, object='chat.completion.chunk',
                            choices=[dict(index=0, delta={'role': 'assistant', 'content': 'OK'}, finish_reason=None)]),
                            dict(id='mock', model=model, object='chat.completion.chunk',
                            choices=[dict(index=0, delta={}, finish_reason='stop')])]
                        for chunk in chunks:
                            self.wfile.write(('data: ' + json.dumps(chunk) + '\n\n').encode())
                        self.wfile.write(b'data: [DONE]\n\n')
                        self.wfile.flush()
                        return
                    arm = state['arm']
                    entries = [r for r in result['requests'] if r['arm'] == arm]
                    expected = state['expected'][arm]
                    if len(entries) >= 12:
                        raise RuntimeError('Twelve-request per-arm safety bound reached')
                    if not entries and body != expected:
                        raw(arm + '-unexpected-first-request.json', body)
                        raise RuntimeError('First request differs from frozen mock payload')
                    if body.get('model') != model or body.get('seed') != 104002 or body.get('temperature') != 0 or body.get('top_p') != 1 or body.get('strata_prefix') != {'messages': 2}:
                        raise RuntimeError('Model, sampling, or cache-boundary mismatch')
                    if body['messages'][:2] != expected['messages'][:2]:
                        raise RuntimeError('Frozen early history changed or compacted')
                    if {k: v for k, v in body.items() if k != 'messages'} != {k: v for k, v in expected.items() if k != 'messages'}:
                        raise RuntimeError('Unexpected non-message payload change')
                    locations = []
                    for message in body['messages']:
                        content = message.get('content')
                        texts = [content] if isinstance(content, str) else [b.get('text', '') for b in content if isinstance(b, dict)] if isinstance(content, list) else []
                        locations.extend(message['role'] for text in texts if pin in text)
                    occurrences = len(locations)
                    if any(role != 'user' for role in locations):
                        raise RuntimeError('Pin injected into tool or other ineligible content')
                    if occurrences > (1 if arm == 'm2' else 0):
                        raise RuntimeError('Unexpected pin duplication or baseline treatment')
                    ledger = []
                    if not entries:
                        with sqlite3.connect(private / 'store.db') as db:
                            ledger = [{'id': row[0], 'status': row[1]} for row in db.execute('select id,status from mutations where session_id=?', (state['session'],))]
                        if arm == 'm1' and ledger or arm == 'm2' and (len(ledger) != 1 or ledger[0]['status'] != 'committed'):
                            raise RuntimeError('Committed ledger disagrees with treatment arm')
                        source = copy.deepcopy(body['messages'])
                        if arm == 'm2':
                            source[-1]['content'].remove({'type': 'text', 'text': pin})
                        q = urllib.request.Request(f'http://127.0.0.1:{control}/api/v1/sessions/{state["session"]}/transform', data=json.dumps({'messages': source, 'provider': 'openai'}).encode(), headers={'Content-Type': 'application/json', 'x-cw-control-token': (private / 'control-token').read_text().strip()}, method='POST')
                        with urllib.request.urlopen(q, timeout=10) as transformed_response:
                            transformed = json.load(transformed_response)
                        if transformed.get('messages') != body['messages'] or transformed.get('applied_rules', []) != [r['id'] for r in ledger]:
                            raise RuntimeError('Exact transform replay or applied-rule agreement failed')
                    before = metrics()
                    idle(before)
                    entry = {'arm': arm, 'index': len(entries), 'request_sha256': digest(body),
                             'pin_blocks': occurrences, 'verified_initial_ledger': ledger if not entries else 'NOT_RECHECKED_FOLLOWUP', 'status': 'DISPATCHED'}
                    result['requests'].append(entry)
                    raw(arm + '-' + str(len(entries)) + '-request.json', body)
                    save()
                    begin = time.monotonic()
                    usage = None
                    finishes = []
                    response_models = set()
                    stream = private / (arm + '-' + str(len(entries)) + '-response.sse')
                    with api('/v1/chat/completions', body) as response, stream.open('wb') as output:
                        stream.chmod(0o600)
                        self.send_response(response.status)
                        self.send_header('Content-Type', 'text/event-stream')
                        self.end_headers()
                        for line in response:
                            remaining()
                            output.write(line)
                            self.wfile.write(line)
                            self.wfile.flush()
                            if line.startswith(b'data: ') and line.strip() != b'data: [DONE]':
                                event = json.loads(line[6:])
                                if event.get('model'):
                                    response_models.add(event['model'])
                                if event.get('usage'):
                                    usage = event['usage']
                                finishes.extend(c['finish_reason'] for c in event.get('choices', []) if c.get('finish_reason'))
                    entry.update(elapsed_seconds=time.monotonic() - begin, usage=usage,
                                 response_models=sorted(response_models), finish_reasons=finishes)
                    save()
                    after = after_request(before)
                    entry['server_request'] = after['requests'][0]
                    entry['cache_after'] = after['conversation_cache']
                    if response_models != {model} or not usage or not finishes:
                        raise RuntimeError('Model identity, finish, or usage missing')
                    if not entries:
                        if not 16384 <= usage['prompt_tokens'] <= 32768:
                            raise RuntimeError('Initial context outside frozen band')
                        cached = usage.get('prompt_tokens_details', {}).get('cached_tokens')
                        if cached != result['gate_cached_tokens'] or after['requests'][0]['reused'] != cached:
                            raise RuntimeError('Initial coding cache state differs from qualified prefix')
                    entry['status'] = 'COMPLETED'
                    save()
                except Exception as error:
                    if not state['fatal']:
                        state['fatal'] = type(error).__name__ + ': ' + str(error)
                    result['fatal'] = state['fatal']
                    save()
                    try:
                        self.send_error(502, 'CW104B measurement gate rejected request')
                    except OSError:
                        pass

    home = private / 'home'
    control, proxy = free_port(), free_port()
    server = ThreadingHTTPServer(('127.0.0.1', 0), Relay)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    env = {k: os.environ[k] for k in ['PATH', 'LANG', 'TERM'] if k in os.environ}
    env.update(HOME=str(home), CW_CONTROL_TOKEN_FILE=str(private / 'control-token'),
               CW_OPENAI_UPSTREAM=f'http://127.0.0.1:{server.server_port}', CW_CAPTURE_WIRE_EGRESS='1')
    cw = options.cw_bin
    daemon = None

    def arm_run(arm, mock):
        workspace = private / 'workspace'
        shutil.copytree(B / 'fixtures/f3', workspace)
        phase = 'mock' if mock else 'coding'
        sid = str(uuid.uuid4())
        seed = private / (phase + '-' + arm + '-seed.jsonl')
        now = datetime.now(timezone.utc).isoformat()
        early = protocol['families']['f3']['opening_brief'] + '\n\nHistorical development reference:\n' + history + '\nCW104B_FINAL_REQUEST_BOUNDARY'
        seed.write_text(json.dumps({'type': 'session', 'version': 3, 'id': sid, 'timestamp': now, 'cwd': str(workspace)}) + '\n' + json.dumps({'type': 'message', 'id': 'opening-user-input', 'parentId': None, 'timestamp': now, 'message': {'role': 'user', 'content': [{'type': 'text', 'text': early}], 'timestamp': int(time.time() * 1000)}}) + '\n')
        seed.chmod(0o600)
        agent_env = dict(env, OMP_PROFILE='cw104b', PI_CODING_AGENT_DIR=str(private / 'state' / (phase + arm)),
                         CW_DAEMON_URL=f'http://127.0.0.1:{control}', CW104B_TRACE_FILE=str(private / (phase + arm + '-hooks.jsonl')))
        if arm == 'm2':
            for args in [['import', '--harness', 'omp', '--workspace', str(workspace), str(seed)],
                         ['pin', 'CW104B-Constraints', protocol['families']['f3']['pin'], '--session', 'omp_' + sid]]:
                subprocess.run([str(cw), '--db', str(private / 'store.db')] + args, env=agent_env, capture_output=True, check=True, timeout=15)
        limit = 30 if mock else min(1500, int(remaining()) - 5)
        args = [options.omp_bin, '--model=' + options.provider + '/' + model, f'--cwd={workspace}',
                f'--session-dir={private}/sessions/{phase}-{arm}', f'--resume={seed}',
                '--system-prompt=Use relative file paths to implement the user request.',
                '--tools=read,edit,write,grep,glob', '--no-context-files', '--no-skills', '--no-rules', '--no-extensions',
                f'--extension={ROOT}/plugins/pi-opencode/index.js', f'--extension={B}/request_options.js',
                '--thinking=off', '--no-title', f'--max-time={limit}', '-p', protocol['final_task']]
        state['arm'] = arm
        state['session'] = 'omp_' + sid
        begin = time.monotonic()
        process = subprocess.Popen(args, env=agent_env, stdout=(private / (phase + arm + '-stdout')).open('wb'),
                                   stderr=(private / (phase + arm + '-stderr')).open('wb'))
        try:
            while process.poll() is None:
                remaining()
                if state['fatal'] or time.monotonic() - begin > limit + 5:
                    raise RuntimeError(state['fatal'] or 'Arm execution deadline reached')
                time.sleep(.2)
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
        with lock:
            if state['fatal']:
                raise RuntimeError(state['fatal'])
        elapsed = time.monotonic() - begin
        final = private / (phase + '-' + arm + '-workspace')
        workspace.rename(final)
        if mock:
            if process.returncode != 0:
                raise RuntimeError('Mock OMP failed')
            return
        entry = {'arm': arm, 'exit_code': process.returncode, 'coding_wall_seconds': elapsed,
                 'acceptance': check('f3', final), 'session_id': 'omp_' + sid,
                 'bootstrap': 'Identical frozen user-only history; no model bootstrap',
                 'end_to_end_arm_seconds': elapsed}
        result['arms'].append(entry)
        save()
        if process.returncode != 0 or state['fatal']:
            raise RuntimeError(state['fatal'] or 'OMP exited unsuccessfully')

    try:
        snapshot = {'kind': 'mock-only; no server access'} if mock_only else metrics()
        if not mock_only:
            idle(snapshot)
        result['runtime_before'] = {k: snapshot.get(k) for k in ['time', 'live', 'conversation_cache']}
        save()
        daemon = subprocess.Popen([str(cw), '--db', str(private / 'store.db'), 'serve', '--port', str(control), '--proxy-port', str(proxy)], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        for _ in range(100):
            try:
                with socket.create_connection(('127.0.0.1', control), timeout=.1):
                    break
            except OSError:
                time.sleep(.05)
        else:
            raise RuntimeError('Isolated CW failed to start')
        provider.update(apiKey='local-relay-placeholder', baseUrl=f'http://127.0.0.1:{proxy}/openai/v1')
        cfg = home / '.omp/profiles/cw104b/agent'
        cfg.mkdir(parents=True)
        (cfg / 'models.yml').write_text(json.dumps({'providers': {options.provider: provider}}))
        (cfg / 'config.yml').write_text('temperature: 0\ntopP: 1\n')
        native = options.omp_natives_dir
        if native is not None:
            shutil.copytree(native, home / '.omp/natives', copy_function=shutil.copy2)
        for arm in ['m1', 'm2']:
            arm_run(arm, True)
        if len(state['mock']) != 2:
            raise RuntimeError('Mock request count differs')
        expected = copy.deepcopy(state['mock'][0])
        expected['messages'][-1]['content'].append({'type': 'text', 'text': pin})
        if expected != state['mock'][1]:
            raise RuntimeError('Full matched request equivalence failed')
        state['expected'] = dict(zip(['m1', 'm2'], state['mock']))
        result['mock_full_request_verified'] = True
        raw('mock-payloads.json', state['mock'])
        if mock_only:
            result['status'] = 'OWNER_ONLY_MOCK_GATES_PASSED_NO_INFERENCE'
            return True
        result['status'] = 'CACHE_GATE'
        save()
        def diagnostic(label, arm):
            before = metrics()
            idle(before)
            body = copy.deepcopy(state['expected']['m1' if arm == 'neutral' else arm])
            if arm == 'neutral':
                body['messages'][-1]['content'] = [{'type': 'text', 'text': 'Reply OK.'}]
            body.update(stream=False, max_completion_tokens=16)
            body.pop('stream_options', None)
            begin = time.monotonic()
            with api('/v1/chat/completions', body) as response:
                answer = json.load(response)
            entry = {'label': label, 'elapsed_seconds': time.monotonic() - begin, 'model': answer.get('model'), 'usage': answer.get('usage')}
            result['gate_requests'].append(entry)
            save()
            after = after_request(before)
            entry['server_request'] = after['requests'][0]
            usage = answer.get('usage')
            if answer.get('model') != model or not usage or not 16384 <= usage['prompt_tokens'] <= 32768:
                raise RuntimeError('Cache gate model, usage, or context mismatch')
            save()
        for label, arm in [('neutral-before-gate-m2', 'neutral'), ('matched-m2', 'm2'), ('neutral-before-gate-m1', 'neutral'), ('matched-m1', 'm1')]:
            diagnostic(label, arm)
        cached = [r['usage'].get('prompt_tokens_details', {}).get('cached_tokens') for r in [r for r in result['gate_requests'] if r['label'].startswith('matched-')]]
        reused = [r['server_request']['reused'] for r in [r for r in result['gate_requests'] if r['label'].startswith('matched-')]]
        if cached[0] is None or cached[0] < 16384 or cached[0] != cached[1] or reused != cached:
            raise RuntimeError('Long-prefix equivalent cache state failed')
        result['gate_cached_tokens'] = cached[0]
        result['status'] = 'CODING'
        save()
        state['mode'] = 'live'
        for arm in ['m1', 'm2']:
            diagnostic('neutral-before-coding-' + arm, 'neutral')
            arm_run(arm, False)
        result['status'] = 'SMOKE_COMPLETE_PENDING_EVIDENCE_AUDIT'
    except Exception as error:
        result['status'] = 'BLOCKED_WITH_EVIDENCE'
        result['error'] = type(error).__name__ + ': ' + str(error)
    finally:
        if daemon:
            daemon.terminate()
            try:
                daemon.wait(timeout=5)
            except subprocess.TimeoutExpired:
                daemon.kill()
                daemon.wait()
        server.shutdown()
        server.server_close()
        result['phase_seconds'] = time.monotonic() - started
        result['owned_processes_stopped'] = True
        save()
    print(json.dumps({'status': result['status'], 'evidence': str(public), 'arms': result['arms'], 'error': result.get('error')}))
    return result['status'] == 'SMOKE_COMPLETE_PENDING_EVIDENCE_AUDIT'


def offline_preflight(result):
    fixture = json.loads(OFFLINE_FIXTURE.read_text())
    if fixture['kind'] != 'sanitized-offline-lifecycle':
        raise ValueError('Wrong offline fixture kind')
    result.update(fixture_sha256=hashlib.sha256(OFFLINE_FIXTURE.read_bytes()).hexdigest(),
                  verification_scope='Sanitized in-process API lifecycle; not native OMP or private historical replay')
    command = ['cargo', 'test', '--locked', '-p', 'cw-proxy', '--test', 'pin_lifecycle']
    completed = subprocess.run(command, cwd=ROOT, capture_output=True, text=True, timeout=300)
    result.update(status='OFFLINE_FIXTURE_PASS' if completed.returncode == 0 else 'OFFLINE_FIXTURE_FAILED',
                  verification_command=command, verification_exit=completed.returncode, inference_calls=0)
    if completed.returncode:
        print(completed.stderr)
    return completed.returncode == 0


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    modes = parser.add_mutually_exclusive_group()
    modes.add_argument('--live', action='store_true', help='Request one live pair; fresh authorization is also mandatory')
    modes.add_argument('--mock-only', action='store_true', help='Explicit alias for the default portable offline fixture')
    modes.add_argument('--owner-replay', action='store_true', help='Owner-only native OMP mock with private frozen history; no inference')
    parser.add_argument('--authorization-file', type=Path)
    parser.add_argument('--endpoint', help='Bare private/loopback IP origin, used only with --live')
    parser.add_argument('--model-profile', type=Path, help='Explicit JSON or YAML OMP model profile; owner modes only')
    parser.add_argument('--provider', help='Provider entry in the explicit profile')
    parser.add_argument('--model', help='Exact model ID from the frozen protocol')
    parser.add_argument('--evidence-dir', type=Path, help='Private owner raw-evidence directory')
    parser.add_argument('--history-file', type=Path, help='Owner-only frozen historical reference')
    parser.add_argument('--manifest-dir', type=Path, default=ROOT / '.audit/cw104b')
    parser.add_argument('--cw-bin', type=Path, default=ROOT / 'target/debug/cw')
    parser.add_argument('--omp-bin', default='omp')
    parser.add_argument('--omp-natives-dir', type=Path)
    options = parser.parse_args(argv)
    stamp = datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%SZ')
    result = {'status': 'PREFLIGHT', 'started_utc': stamp, 'run_id': 'smoke-' + stamp + '-' + uuid.uuid4().hex[:8],
              'mode': 'live' if options.live else 'owner-only-mock' if options.owner_replay else 'offline-fixture',
              'authorization': {'record_valid': False, 'consumed': False}, 'inference_calls': 0,
              'provider_requests_admitted': 0}
    if options.live:
        result.pop('inference_calls')  # Actual admissions/usage are recorded by the live path.
    options.manifest_dir.mkdir(parents=True, exist_ok=True)
    public = options.manifest_dir / (result['run_id'] + '.json')
    try:
        freeze_sha256 = verify_runner_freeze()
        result['runner_freeze_sha256'] = freeze_sha256
        if not options.live and (options.authorization_file or options.endpoint):
            raise ValueError('--authorization-file and --endpoint require --live')
        if options.live and not options.authorization_file:
            raise ValueError('--live requires a fresh --authorization-file')
        if options.live or options.owner_replay:
            for field in ['model_profile', 'provider', 'model', 'evidence_dir', 'history_file']:
                if not getattr(options, field):
                    raise ValueError('Owner modes require --' + field.replace('_', '-'))
            if options.live:
                if not options.endpoint:
                    raise ValueError('--live requires an explicit --endpoint')
                result['authorization'] = live_authorization(options, freeze_sha256)
            options.evidence_dir.mkdir(parents=True, mode=0o700, exist_ok=True)
            if os.name != 'nt' and options.evidence_dir.stat().st_mode & 0o077:
                raise ValueError('Owner evidence directory must be private (0700)')
            if options.live:
                claim_authorization(options.evidence_dir, result['authorization'])
            passed = run_owner(options, result, public)
        else:
            passed = offline_preflight(result)
        return 0 if passed else 1
    except Exception as error:
        result.update(status='PREFLIGHT_BLOCKED', error_type=type(error).__name__,
                      error=str(error) if isinstance(error, (ValueError, RuntimeError)) else 'Required local input or execution unavailable')
        return 2
    finally:
        public.write_text(json.dumps(result, indent=2) + '\n')
        print(json.dumps({'status': result['status'], 'mode': result['mode'], 'manifest': str(public)}))


if __name__ == '__main__':
    raise SystemExit(main())
