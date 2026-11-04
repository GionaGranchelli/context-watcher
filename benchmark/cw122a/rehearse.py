"""Offline by construction: real proxy to local mock + predefined synthetic repairs, never an agent."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

from analyze import analyze, require, verify_wire
from oracle import ROOT, check, digest

REPO = ROOT.parent.parent
HISTORICAL = ['docs/CW-115-FOUR-MODE-ISOLATION-REPORT.md', 'docs/CW-115-PERFORMANCE-SAVINGS-QUALIFICATION.md', 'docs/cw115_four_mode_benchmark_data.json']


def verify_freeze():
    frozen = json.loads((ROOT / 'freeze.json').read_text())
    require(all(digest(REPO / name) == value for name, value in frozen['files'].items()), 'source/fixture freeze mismatch')
    return digest(ROOT / 'freeze.json')


def build_dataset(wires, campaign):
    traces = {family: json.loads((ROOT / task['trace']).read_text()) for family, task in campaign['tasks'].items()}
    repairs = json.loads((ROOT / 'synthetic-solutions.json').read_text())['solutions']
    indexed = {w['session_id']: w for w in wires}
    require(len(indexed) == len(wires) == len(campaign['pairs']) * 2, 'duplicate/missing wire attempts')
    runs = []
    for pair in campaign['pairs']:
        family = pair['family']
        for order, mode in enumerate(pair['order']):
            session = pair['pair_id'] + '-' + mode
            wire = indexed[session]
            verify_wire(wire, campaign, traces[family])
            with tempfile.TemporaryDirectory(prefix='cw122a-fixture-') as directory:
                workspace = Path(directory) / 'repo'
                shutil.copytree(ROOT / 'fixtures' / family, workspace)
                initial = {p.name: digest(p) for p in workspace.iterdir() if p.is_file()}
                require(not check(family, workspace, campaign)['passed'], 'broken fixture unexpectedly passed acceptance')
                for name, contents in repairs[family].items():
                    (workspace / name).write_text(contents)
                acceptance = check(family, workspace, campaign)
                acceptance.pop('output')  # No private paths or raw transcripts in shareable aggregate.
            usage = wire['mock_response']['usage']
            request = {'input_tokens': usage.get('input_tokens'), 'cached_input_tokens': usage.get('input_tokens_details', {}).get('cached_tokens'),
                       'output_tokens': usage.get('output_tokens'), 'usage_provenance':'synthetic_mock_usage', 'raw_usage':usage,
                       'first_byte_latency_ms':None, 'first_byte_provenance':'NOT_AVAILABLE'}
            runs.append({'pair_id':pair['pair_id'],'family':family,'task_id':campaign['tasks'][family]['id'],'repetition':pair['repetition'],
                         'mode':mode,'order':order,'session_id':session,'attempt_id':session,'status':'completed','failure_reason':None,
                         'repository_hash':hashlib.sha256(json.dumps(initial,sort_keys=True).encode()).hexdigest(),
                         'initial_fixture_hashes':initial,'configuration_hash':digest(ROOT/'campaign.json'),
                         'runtime':{'provider':'local_mock','model':'cw122-offline-mock','agent_version':'NOT_EXECUTED; predefined fixture repair', 'model_revision':'NOT_AVAILABLE; synthetic mock','server_version':'in-process Axum fixture'},
                         'sampling':campaign['sampling'],'requests':[request],'duration_ms':1000,'duration_provenance':'SYNTHETIC_FROZEN_TEST_VALUE_NOT_A_TIMING_RESULT',
                         'acceptance':acceptance,'wire':wire})
    return {'schema':'cw122a-runs-v1','scope':'SYNTHETIC_REHEARSAL','runs':runs}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--live', action='store_true', help='Unsupported in CW-122A; fresh CW-122B authorization and adapter qualification required')
    args = parser.parse_args(argv)
    require(not args.live, 'LIVE_NOT_IMPLEMENTED_OR_AUTHORIZED; offline readiness only')
    freeze = verify_freeze()
    require(not args.output.exists(), 'evidence directory must be new; never overwrite runs')
    args.output.mkdir(parents=True)
    before = {name:digest(REPO/name) for name in HISTORICAL}
    env = {k:v for k,v in os.environ.items() if not any(word in k.upper() for word in ['API_KEY','TOKEN','SECRET','AUTHORIZATION'])}
    env['CW122_OFFLINE_EVIDENCE'] = str(args.output.resolve())
    subprocess.run(['cargo','test','-p','cw-proxy','--test','cw122_pruning','--','--test-threads=1'], cwd=REPO, env=env, check=True, timeout=300)
    campaign = json.loads((ROOT/'campaign.json').read_text())
    wires = json.loads((args.output/'wire.json').read_text())
    abstentions = json.loads((args.output/'abstentions.json').read_text())
    require(len(abstentions)==5 and all(r['original']==r['outbound'] and not r['applied_mutation_ids'] for r in abstentions), 'abstention evidence mismatch')
    dataset = build_dataset(wires,campaign)
    traces = {family:json.loads((ROOT/task['trace']).read_text()) for family,task in campaign['tasks'].items()}
    result = analyze(dataset,campaign,traces)
    require(result == analyze(dataset,campaign,traces), 'analysis is nondeterministic')
    require(before == {name:digest(REPO/name) for name in HISTORICAL}, 'historical evidence changed')
    require(verify_freeze() == freeze, 'fixture/transcript sources changed')
    for name,value in [('runs.json',dataset),('analysis.json',result),('manifest.json',{'scope':'OFFLINE_REHEARSAL','inference_calls':0,'abstention_cases':[r['session_id'] for r in abstentions],'runner_freeze_sha256':freeze,'historical_hashes':before,'status':'OFFLINE_GATES_PASS','live_admission':'NO_GO; no authorization or qualified live adapter'})]:
        (args.output/name).write_text(json.dumps(value,indent=2,sort_keys=True)+'\n')
    print(json.dumps({'status':'OFFLINE_GATES_PASS','pairs':len(result['pairs']),'product_value':result['product_value_verdict'],'live':'NOT_AUTHORIZED'}))
    return 0

if __name__ == '__main__':
    raise SystemExit(main())
