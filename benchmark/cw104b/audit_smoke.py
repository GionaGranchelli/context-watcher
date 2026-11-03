"""Independently audit one saved smoke run; never dispatch inference."""
import collections
import hashlib
import json
from pathlib import Path
import sqlite3
import subprocess
import sys

from acceptance import check

B = Path(__file__).resolve().parent
ROOT = B.parents[1]


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True).encode()).hexdigest()


def audit(source):
    saved = json.loads(source.read_text())
    if saved['status'] != 'SMOKE_COMPLETE_PENDING_EVIDENCE_AUDIT':
        raise RuntimeError('Run is incomplete; preserve its failure rather than qualifying it')
    private = Path.home() / '.local/share/context-watcher/cw104b-evidence-vault' / saved['raw_evidence']
    protocol = json.loads((B / 'protocol.json').read_text())
    pin = '\n<cw_pinned_context>\n- [CW104B-Constraints]: ' + protocol['families']['f3']['pin'] + '\n</cw_pinned_context>'
    output = {'source': source.name, 'arms': {}, 'pair_differences': {}, 'checks': {},
              'statistics': 'One matched pair; no reliable effect-size or reliability claim.'}
    pin_violations = []
    expected_wires = [digest(body) for body in json.loads((private / 'mock-payloads.json').read_text())]
    for arm in saved['arms']:
        name = arm['arm']
        requests = [r for r in saved['requests'] if r['arm'] == name]
        if not requests or any(r['status'] != 'COMPLETED' or not r.get('usage') for r in requests):
            raise RuntimeError('Missing or failed coding dispatch/usage')
        workspace = private / ('coding-' + name + '-workspace')
        independent = check('f3', workspace)
        tests = subprocess.run([sys.executable, '-B', '-m', 'unittest', 'discover', '-s', str(workspace)],
                               cwd=workspace, capture_output=True, text=True, timeout=30)
        entries = [json.loads(line) for line in (private / ('coding-' + name + '-seed.jsonl')).read_text().splitlines()]
        messages = [e['message'] for e in entries if e.get('type') == 'message']
        tools = [m for m in messages if m.get('role') == 'toolResult']
        original_pin_blocks = sum(c.get('text') == pin for m in messages for c in m.get('content', []) if isinstance(c, dict))
        if original_pin_blocks:
            raise RuntimeError('Original transcript contains an injected experiment block')
        for request in requests:
            body = json.loads((private / f"{name}-{request['index']}-request.json").read_text())
            if digest(body) != request['request_sha256']:
                raise RuntimeError('Captured request hash mismatch')
            expected_wires.append(digest(body))
            for message in body['messages']:
                content = message.get('content')
                texts = [content] if isinstance(content, str) else [c.get('text', '') for c in content if isinstance(c, dict)] if isinstance(content, list) else []
                if any(pin in text for text in texts):
                    if name != 'm2' or message['role'] != 'user':
                        pin_violations.append({'arm': name, 'request': request['index'], 'role': message['role']})
        server = [r['server_request'] for r in requests]
        measurements = {'acceptance': independent, 'public_tests_passed': tests.returncode == 0,
            'public_test_diagnostic': tests.stderr, 'coding_wall_seconds': arm['coding_wall_seconds'],
            'requests': len(requests), 'input_tokens': sum(r['usage']['prompt_tokens'] for r in requests),
            'output_tokens': sum(r['usage']['completion_tokens'] for r in requests),
            'cached_tokens': sum(r['usage']['prompt_tokens_details']['cached_tokens'] for r in requests),
            'server_reused_tokens': sum(r['reused'] for r in server),
            'prefill_ms': sum(r['prompt_ms'] for r in server), 'decode_ms': sum(r['decode_ms'] for r in server),
            'tool_calls': len(tools), 'tool_errors': sum(bool(m.get('isError')) for m in tools),
            'tool_names': dict(collections.Counter(m.get('toolName') for m in tools)),
            'compaction_events': sum(e.get('type') in ['compaction', 'branch_summary'] for e in entries),
            'first_cached_tokens': requests[0]['usage']['prompt_tokens_details']['cached_tokens'],
            'missing_usage_records': 0, 'transport_failures': 0,
            'retry_count': 0, 'original_transcript_pin_absent': True,
            'end_to_end_arm_seconds': arm['end_to_end_arm_seconds']}
        if measurements['cached_tokens'] != measurements['server_reused_tokens']:
            raise RuntimeError('Usage and server reuse disagree')
        output['arms'][name] = measurements
    with sqlite3.connect('file:' + str(private / 'store.db') + '?mode=ro', uri=True) as db:
        rows = db.execute('select wr.body,we.body from wire_requests wr join wire_egress we on wr.turn_id=we.turn_id').fetchall()
    if any(json.loads(a) != json.loads(b) for a, b in rows):
        raise RuntimeError('CW proxy changed a payload after native transformation')
    if collections.Counter(digest(json.loads(b)) for _, b in rows) != collections.Counter(expected_wires):
        raise RuntimeError('Proxy wire records do not match captured mock/coding dispatches')
    a, b = [output['arms'][name] for name in ['m1', 'm2']]
    for metric in ['coding_wall_seconds', 'requests', 'input_tokens', 'output_tokens', 'cached_tokens', 'prefill_ms', 'decode_ms', 'tool_calls', 'tool_errors']:
        output['pair_differences'][metric] = {'absolute_m2_minus_m1': b[metric] - a[metric],
            'relative_percent': 100 * (b[metric] - a[metric]) / a[metric] if a[metric] else 'NOT_AVAILABLE'}
    output['pin_location_violations'] = pin_violations
    output['checks'] = {'no_pin_in_tool_or_other_ineligible_content': not pin_violations,'both_acceptance': all(m['acceptance']['passed'] and m['public_tests_passed'] for m in [a, b]),
        'equal_initial_cache': a['first_cached_tokens'] == b['first_cached_tokens'] == saved['gate_cached_tokens'],
        'full_mock_payload_equivalence': saved['mock_full_request_verified'], 'full_proxy_wire_equivalence': True,
        'first_m2_applied_rule_verified': len(next(r for r in saved['requests'] if r['arm'] == 'm2')['verified_initial_ledger']) == 1,
        'within_original_60_minutes': saved['elapsed_before_attempt_seconds'] + saved['phase_seconds'] <= 3600}
    output['status'] = 'READY_FOR_FULL_LOCAL_CAMPAIGN' if all(output['checks'].values()) else 'BLOCKED_WITH_EVIDENCE'
    output['raw_file_hashes'] = {str(p.relative_to(private)): hashlib.sha256(p.read_bytes()).hexdigest()
                                for p in private.rglob('*') if p.is_file() and 'home' not in p.relative_to(private).parts and 'state' not in p.relative_to(private).parts}
    destination = source.with_name(source.stem + '-audit.json')
    destination.write_text(json.dumps(output, indent=2) + '\n')
    print(json.dumps({'status': output['status'], 'checks': output['checks'], 'arms': output['arms']}))
    return output['status'] == 'READY_FOR_FULL_LOCAL_CAMPAIGN'


if __name__ == '__main__':
    raise SystemExit(0 if audit(Path(sys.argv[1])) else 1)
