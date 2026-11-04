"""Run the bounded real-Codex campaign against CW's in-process local mock."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import stat
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
_received_signal = None


class CancellationUnverified(RuntimeError):
    pass
TEST = [
    'cargo', 'test', '--offline', '--locked', '-p', 'cw-proxy', '--test', 'qualification',
    'test_cw122b1_codex_mock_campaign', '--', '--ignored', '--exact', '--nocapture',
]


def write_private(path, value):
    missing = []
    parent = path.parent
    while not parent.exists():
        missing.append(parent)
        parent = parent.parent
    if os.name == 'posix' and stat.S_IMODE(parent.stat().st_mode) != 0o700:
        raise PermissionError(f'private evidence parent has unsafe mode: {parent}')
    for directory in reversed(missing):
        directory.mkdir(mode=0o700)
        directory.chmod(0o700)
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


def run_bounded(command, cwd, env, timeout, cancel=None):
    cancel = cancel or kill_process_tree
    proc = subprocess.Popen(
        command, cwd=cwd, env=env, stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        start_new_session=True,
    )
    deadline = time.monotonic() + timeout
    try:
        while True:
            remaining = deadline - time.monotonic()
            if _received_signal is not None:
                cleanup = cancel(proc.pid)
                if not cleanup['verified']:
                    proc.poll()
                    if proc.stdout:
                        proc.stdout.close()
                    if proc.stderr:
                        proc.stderr.close()
                    raise CancellationUnverified(f"process cleanup unverified: {cleanup}")
                output, _ = proc.communicate()
                error = InterruptedError(f'campaign interrupted by signal {_received_signal}')
                error.output = output
                raise error
            if remaining <= 0:
                cleanup = cancel(proc.pid)
                if not cleanup['verified']:
                    proc.poll()
                    if proc.stdout:
                        proc.stdout.close()
                    if proc.stderr:
                        proc.stderr.close()
                    raise CancellationUnverified(f"process cleanup unverified: {cleanup}")
                output, _ = proc.communicate()
                raise subprocess.TimeoutExpired(command, timeout, output=output)
            try:
                output, _ = proc.communicate(timeout=min(remaining, 0.25))
                if _received_signal is not None:
                    error = InterruptedError(f'campaign interrupted by signal {_received_signal}')
                    error.output = output
                    raise error
                return proc.returncode, output
            except subprocess.TimeoutExpired:
                continue
    except (CancellationUnverified, InterruptedError, subprocess.TimeoutExpired):
        raise
    except BaseException:
        cleanup = cancel(proc.pid)
        if not cleanup['verified']:
            proc.poll()
            if proc.stdout:
                proc.stdout.close()
            if proc.stderr:
                proc.stderr.close()
            raise CancellationUnverified(f"process cleanup unverified: {cleanup}")
        proc.communicate()
        raise


def install_termination_handlers():
    global _received_signal
    _received_signal = None
    previous = {}
    def interrupted(signum, _frame):
        global _received_signal
        _received_signal = signum
    for signum in (signal.SIGINT, signal.SIGTERM):
        previous[signum] = signal.signal(signum, interrupted)
    return previous


def restore_signal_handlers(previous):
    global _received_signal
    _received_signal = None
    for signum, handler in previous.items():
        signal.signal(signum, handler)


def kill_process_tree(root_pid, *, killpg=os.killpg, proc_root=Path('/proc'), timeout=2):
    if not proc_root.is_dir():
        return {'verified': False, 'signaled_process_groups': [],
                'remaining_pids': [root_pid], 'errors': ['process_inventory_unavailable']}
    descendants = {root_pid}
    groups = set()
    if proc_root.is_dir():
        parents = {}
        groups_by_pid = {}
        for entry in proc_root.iterdir():
            if not entry.name.isdigit():
                continue
            try:
                fields = (entry / 'stat').read_text().split(') ', 1)[1].split()
                parents[int(entry.name)] = int(fields[1])
                groups_by_pid[int(entry.name)] = int(fields[2])
            except (OSError, ValueError, IndexError):
                continue
        changed = True
        while changed:
            changed = False
            for pid, parent in parents.items():
                if parent in descendants and pid not in descendants:
                    descendants.add(pid)
                    changed = True
        groups.update(groups_by_pid[pid] for pid in descendants if pid in groups_by_pid)
    if root_pid not in groups_by_pid:
        return {'verified': False, 'signaled_process_groups': [],
                'remaining_pids': [root_pid], 'errors': ['root_absent_at_inventory']}
    errors = []
    for group in groups:
        try:
            killpg(group, signal.SIGKILL)
        except ProcessLookupError:
            pass
        except OSError as error:
            errors.append(f'{type(error).__name__}:{error.errno}')
    targets = set(descendants)
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        alive = []
        for pid in targets:
            try:
                fields = (proc_root / str(pid) / 'stat').read_text().split(') ', 1)[1].split()
                if fields[0] != 'Z':
                    alive.append(pid)
            except (OSError, ValueError, IndexError):
                pass
        if not alive:
            break
        time.sleep(0.01)
    else:
        alive = sorted(pid for pid in targets if (proc_root / str(pid)).exists())
    return {
        'verified': not errors and not alive,
        'signaled_process_groups': sorted(groups),
        'remaining_pids': alive,
        'errors': errors,
    }


def public_aggregate(manifest):
    return {
        'scope': manifest['scope'],
        'status': manifest['status'],
        'hash_algorithm': manifest['hash_algorithm'],
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
    if not Path('/proc').is_dir():
        raise SystemExit('bounded nested-process cleanup requires Linux /proc')
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
    commit = verified_git_commit(REPO, env)
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
    previous_handlers = install_termination_handlers()
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
            'failure_phase': 'campaign_subprocess',
            'failure_reason': f'{type(exc).__name__}: {exc}',
        })
        raise
    finally:
        restore_signal_handlers(previous_handlers)

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


def verified_git_commit(cwd, env):
    try:
        commit = subprocess.run(
            ['git', 'rev-parse', 'HEAD'], cwd=cwd, env=env,
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, check=True,
        ).stdout.strip()
    except (OSError, subprocess.CalledProcessError) as exc:
        raise SystemExit(
            'runner requires a Git checkout; no unverified commit override is accepted'
        ) from exc
    if len(commit) not in (40, 64) or any(char not in '0123456789abcdef' for char in commit):
        raise SystemExit('Git returned an invalid commit identifier')
    return commit


if __name__ == '__main__':
    raise SystemExit(main())
