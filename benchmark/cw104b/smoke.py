"""One local-only matched smoke pair, with mock and cache gates before coding."""
import copy
import hashlib
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
import uuid
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import yaml
from acceptance import check

B = Path(__file__).resolve().parent
ROOT = B.parents[1]
VAULT = Path.home() / '.local/share/context-watcher/cw104b-evidence-vault'
ENDPOINT = 'http://192.168.1.156:8095'
MODEL = 'Qwen3.8-Flash-Next-GSQ-RCO-IQ2_XS'


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True).encode()).hexdigest()


def free_port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


def run(mock_only=False):
    started = time.monotonic()
    phase_start = os.environ.get('CW104B_PHASE_START_UTC')
    elapsed_before = (datetime.now(timezone.utc) - datetime.strptime(phase_start, '%Y%m%dT%H%M%SZ').replace(tzinfo=timezone.utc)).total_seconds() if phase_start else 0
    deadline = started + max(0, 3600 - elapsed_before)
    stamp = datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%SZ')
    private = VAULT / ('smoke-' + stamp)
    private.mkdir(mode=0o700)
    public = ROOT / '.audit/cw104b' / ('smoke-' + stamp + '.json')
    result = {'status': 'PREFLIGHT', 'started_utc': stamp, 'arms': [], 'gate_requests': [],
              'requests': [], 'raw_evidence': private.name, 'hard_cap_seconds': 3600, 'original_phase_start_utc': phase_start or stamp, 'elapsed_before_attempt_seconds': elapsed_before}
    lock = threading.Lock()
    state = {'mode': 'mock', 'arm': None, 'mock': [], 'expected': {}, 'fatal': None}
    config = yaml.safe_load(Path('/tmp/cw104a-local-qual/home/.omp/profiles/cw104a-m1/agent/models.yml').read_text())
    provider = config['providers']['machinist-strata']
    key = provider.pop('apiKey')
    protocol = json.loads((B / 'protocol.json').read_text())
    frozen = json.loads((B / 'freeze.json').read_text())
    for name, expected in frozen['files'].items():
        if hashlib.sha256((B / name).read_bytes()).hexdigest() != expected:
            raise RuntimeError('Frozen implementation hash mismatch: ' + name)
    history = (VAULT / 'cw104b-frozen-history.txt').read_text()
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
        request = urllib.request.Request(ENDPOINT + path,
            data=json.dumps(body).encode() if body is not None else None,
            headers={'Authorization': 'Bearer ' + key, 'Content-Type': 'application/json'})
        return urllib.request.urlopen(request, timeout=min(300, remaining()))

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
                        chunks = [dict(id='mock', model=MODEL, object='chat.completion.chunk',
                            choices=[dict(index=0, delta={'role': 'assistant', 'content': 'OK'}, finish_reason=None)]),
                            dict(id='mock', model=MODEL, object='chat.completion.chunk',
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
                    if body.get('model') != MODEL or body.get('seed') != 104002 or body.get('temperature') != 0 or body.get('top_p') != 1 or body.get('strata_prefix') != {'messages': 2}:
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
                    if response_models != {MODEL} or not usage or not finishes:
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
    cw = ROOT / 'target/debug/cw'
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
        args = ['omp', '--model=machinist-strata/' + MODEL, f'--cwd={workspace}',
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
        (cfg / 'models.yml').write_text(yaml.safe_dump({'providers': {'machinist-strata': provider}}))
        (cfg / 'config.yml').write_text('temperature: 0\ntopP: 1\n')
        native = Path('/tmp/cw104a-local-qual/home/.omp/natives')
        if native.exists():
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
            result['status'] = 'MOCK_GATES_PASSED_NO_INFERENCE'
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
            if answer.get('model') != MODEL or not usage or not 16384 <= usage['prompt_tokens'] <= 32768:
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


if __name__ == '__main__':
    import argparse
    parser = argparse.ArgumentParser()
    parser.add_argument('--mock-only', action='store_true')
    args = parser.parse_args()
    raise SystemExit(0 if run(args.mock_only) else 1)
