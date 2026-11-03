"""Inspect saved CW-104B evidence. No network, credentials, or model execution."""
import argparse
import copy
import hashlib
import json
from pathlib import Path
import sqlite3

ROOT = Path(__file__).resolve().parents[2]


def strings(value):
    if isinstance(value, str):
        yield value
    elif isinstance(value, list):
        for item in value:
            yield from strings(item)
    elif isinstance(value, dict):
        for item in value.values():
            yield from strings(item)


def diagnose(source, private):
    run = json.loads(source.read_text())
    audit = json.loads(source.with_name(source.stem + '-audit.json').read_text())
    protocol = json.loads((ROOT / 'benchmark/cw104b/protocol.json').read_text())
    block = '\n<cw_pinned_context>\n- [CW104B-Constraints]: ' + protocol['families']['f3']['pin'] + '\n</cw_pinned_context>'
    verified = {}

    def read(name):
        data = (private / name).read_bytes()
        actual = hashlib.sha256(data).hexdigest()
        if actual != audit['raw_file_hashes'][name]:
            raise RuntimeError('Saved evidence hash mismatch: ' + name)
        verified[name] = actual
        return data

    rows, requests = [], {}
    prefix = None
    for arm in ['m1', 'm2']:
        requests[arm] = []
        for record in [r for r in run['requests'] if r['arm'] == arm]:
            index = record['index']
            body = json.loads(read(f'{arm}-{index}-request.json'))
            requests[arm].append(body)
            current_prefix = json.dumps(body['messages'][:2], sort_keys=True).encode()
            if prefix is None:
                prefix = current_prefix
            if current_prefix != prefix:
                raise RuntimeError('Protected two-message prefix changed')
            locations = [{'message': i, 'role': message['role']}
                         for i, message in enumerate(body['messages'])
                         if any(block in text for text in strings(message.get('content')))]
            calls = {}
            for line in read(f'{arm}-{index}-response.sse').decode().splitlines():
                if not line.startswith('data: ') or line == 'data: [DONE]':
                    continue
                event = json.loads(line[6:])
                for choice in event.get('choices', []):
                    for call in choice.get('delta', {}).get('tool_calls', []):
                        key = call['index']
                        calls[key] = calls.get(key, '') + call.get('function', {}).get('name', '')
            rows.append({'arm': arm, 'request': index, 'tail_role': body['messages'][-1]['role'],
                         'messages': len(body['messages']), 'pin_locations': locations,
                         'tools_requested': list(calls.values()),
                         'early_brief_present': any(protocol['families']['f3']['opening_brief'] in text
                                                   for text in strings(body['messages'][1]['content'])),
                         'cached_tokens': record['usage']['prompt_tokens_details']['cached_tokens']})
        native = [json.loads(line) for line in read(f'coding-{arm}-seed.jsonl').decode().splitlines()]
        if any(block in text for text in strings(native)):
            raise RuntimeError('Native transcript contains the injected experiment block')

    m2 = requests['m2']
    without_pin = copy.deepcopy(m2[0]['messages'])
    without_pin[-1]['content'].remove({'type': 'text', 'text': block})
    if m2[1]['messages'][:len(without_pin)] != without_pin:
        raise RuntimeError('First follow-up differs by more than pin disappearance and appended history')
    session = next(arm['session_id'] for arm in run['arms'] if arm['arm'] == 'm2')
    read('store.db')
    with sqlite3.connect('file:' + str(private / 'store.db') + '?mode=ro&immutable=1', uri=True) as db:
        rules = [{'id': row[0], 'status': row[1]}
                 for row in db.execute('select id,status from mutations where session_id=?', (session,))]
    if len(rules) != 1 or rules[0]['status'] != 'committed':
        raise RuntimeError('Treatment rule is not still committed')
    m2_rows = [row for row in rows if row['arm'] == 'm2']
    if [row['request'] for row in m2_rows if row['pin_locations']] != [0]:
        raise RuntimeError('Trace no longer supports the one-request exposure diagnosis')
    if any(not row['early_brief_present'] for row in rows):
        raise RuntimeError('Original early constraints disappeared')
    return {'source': source.name, 'status': 'OFFLINE_DIAGNOSIS_VERIFIED', 'requests': rows,
            'm2_explicit_pin_exposure_requests': 1, 'm2_total_requests': len(m2_rows),
            'm2_write_decision_requests': [row['request'] for row in m2_rows if 'write' in row['tools_requested']],
            'm2_pin_present_at_write': any(row['pin_locations'] for row in m2_rows if 'write' in row['tools_requested']),
            'native_transcripts_pin_absent': True, 'early_constraints_present_every_request': True,
            'rule_after_run': rules, 'first_followup_prefix_equals_first_request_with_only_pin_removed': True,
            'protected_prefix_canonical_json_sha256': hashlib.sha256(prefix).hexdigest(),
            'verified_source_hashes': verified, 'new_inference_calls': 0,
            'physical_cache_claim': 'Only the saved provider telemetry was inspected; no new reuse was measured.'}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--run', type=Path, default=ROOT / '.audit/cw104b/smoke-20261103T140929Z.json')
    parser.add_argument('--raw-dir', type=Path)
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    raw = args.raw_dir or Path.home() / '.local/share/context-watcher/cw104b-evidence-vault' / json.loads(args.run.read_text())['raw_evidence']
    report = diagnose(args.run, raw)
    rendered = json.dumps(report, indent=2) + '\n'
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(rendered)
    print(json.dumps({key: report[key] for key in ['status', 'm2_explicit_pin_exposure_requests',
          'm2_total_requests', 'm2_write_decision_requests', 'm2_pin_present_at_write', 'new_inference_calls']}))
