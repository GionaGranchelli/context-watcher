#!/usr/bin/env python3
"""Offline real-PTY smoke. Builds no model requests; uses only a disposable daemon/control API."""
import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import pty
import secrets
import select
import socket
import struct
import subprocess
import tempfile
import termios
import time
import urllib.request


def require(ok, message):
    if not ok:
        raise RuntimeError(message)


def port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=Path('target/debug/cw'))
    parser.add_argument('--evidence-dir', type=Path, required=True)
    args = parser.parse_args()
    binary = str(args.binary.resolve())
    args.evidence_dir.mkdir(parents=True, exist_ok=False)
    evidence = args.evidence_dir.resolve()
    report = {'scope': 'scripted real PTY; disposable daemon; not human visual qualification',
              'inference': 'NOT_EXECUTED', 'binary_sha256': hashlib.sha256(Path(binary).read_bytes()).hexdigest(),
              'steps': [], 'terminal_restoration': []}
    with tempfile.TemporaryDirectory(prefix='cw121c2-') as temporary:
        root = Path(temporary)
        for name in ['home', 'config', 'data', 'workspace']:
            (root / name).mkdir()
        token = secrets.token_hex(32)
        env = dict(os.environ, HOME=str(root / 'home'), USERPROFILE=str(root / 'home'),
                   XDG_CONFIG_HOME=str(root / 'config'), XDG_DATA_HOME=str(root / 'data'),
                   CW_CONTROL_TOKEN=token, TERM='xterm-256color')
        for key in ['OPENAI_API_KEY', 'ANTHROPIC_API_KEY', 'GEMINI_API_KEY']:
            env.pop(key, None)
        base = f'http://127.0.0.1:{port()}'
        def call(path, body=None, revision=None):
            headers = {'x-cw-control-token': token, 'Content-Type': 'application/json'}
            if revision:
                headers['If-Match'] = revision
            request = urllib.request.Request(base + path, None if body is None else json.dumps(body).encode(), headers)
            with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(request, timeout=3) as response:
                return json.load(response)
        def policy():
            return call('/api/v1/sessions/operator-session/pin-context-policy')
        with (evidence / 'daemon.log').open('wb') as log:
            daemon = subprocess.Popen([binary, '--db', str(root / 'daemon.sqlite'), 'serve', '--port', base.rsplit(':', 1)[1], '--proxy-port', str(port())], cwd=root / 'workspace', env=env, stdout=log, stderr=log)
            try:
                for _ in range(100):
                    try:
                        call('/api/v1/health')
                        break
                    except (OSError, ValueError):
                        require(daemon.poll() is None, 'Disposable daemon exited')
                        time.sleep(.05)
                else:
                    raise RuntimeError('Daemon unavailable')
                call('/api/v1/ingest', {'session_id': 'operator-session', 'provider': 'openai', 'model': 'offline-fixture', 'payload': {'messages': [{'role': 'user', 'content': 'Offline fixture only.'}]}})
                graph = call('/api/v1/sessions/operator-session/graph')
                call('/api/v1/sessions/operator-session/mutations', {'op': {'op': 'pin_context', 'label': 'Fixture', 'content': 'Keep the fixture API.', 'source_node_id': None}, 'status': 'committed'}, graph['revision'])
                for failed_api in [False, True]:
                    master, slave = pty.openpty()
                    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 40, 120, 0, 0))
                    before = termios.tcgetattr(slave)
                    target = f'http://127.0.0.1:{port()}' if failed_api else base
                    process = subprocess.Popen([binary, '--db', str(root / 'separate-optimization.sqlite'), 'tui', '--daemon-url', target], cwd=root / 'workspace', env=env, stdin=slave, stdout=slave, stderr=slave)
                    raw = bytearray()
                    def drain(seconds=.15):
                        deadline = time.monotonic() + seconds
                        while time.monotonic() < deadline:
                            readable, _, _ = select.select([master], [], [], .03)
                            if readable:
                                raw.extend(os.read(master, 65536))
                    def keys(value):
                        os.write(master, value)
                        drain()
                    try:
                        drain(.4)
                        # Crossterm queries cursor position while entering alternate screen.
                        keys(b'\x1b[1;1R')
                        keys(b'c')
                        if failed_api:
                            drain(.5)
                            require(not policy()['enabled'], 'Failed API attempt changed consent')
                            keys(b'q')
                        else:
                            keys(b'e')
                            require(not policy()['enabled'], 'Single hotkey granted consent')
                            for _ in range(30):
                                keys(b'\x1b[6~')
                            keys(b'e')
                            require(b'Type ENABLE' in raw, 'Confirmation prompt missing')
                            keys(b'ENABLE operator-session\r')
                            require(policy()['enabled'], 'Typed confirmation did not grant consent')
                            report['steps'].append('typed confirmation enabled daemon session with separate local TUI database')
                            keys(b'd')
                            current = policy()
                            require(not current['enabled'] and len(current['stored_pins']) == 1, 'Revocation changed stored pin')
                            report['steps'].append('revocation preserved stored pin')
                            keys(b'q')
                            keys(b'q')
                        process.wait(timeout=5)
                        drain()
                        require(process.returncode == 0, 'TUI failed')
                        require(termios.tcgetattr(slave) == before, 'Terminal attributes not restored')
                        report['terminal_restoration'].append({'failed_api': failed_api, 'restored': True})
                        name = 'failed-api.ansi' if failed_api else 'operator.ansi'
                        (evidence / name).write_bytes(raw)
                        report[name + '_sha256'] = hashlib.sha256(raw).hexdigest()
                    finally:
                        (evidence / ('failed-api-attempt.ansi' if failed_api else 'operator-attempt.ansi')).write_bytes(raw)
                        if process.poll() is None:
                            process.terminate()
                            process.wait(timeout=5)
                        os.close(master)
                        os.close(slave)
            finally:
                daemon.terminate()
                daemon.wait(timeout=5)
                report['daemon_stopped'] = True
    report['status'] = 'PASS'
    (evidence / 'result.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report))


if __name__ == '__main__':
    main()
