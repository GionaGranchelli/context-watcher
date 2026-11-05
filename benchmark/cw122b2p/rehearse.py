"""Replay saved Codex mock evidence through the frozen CW-122A schema and analyzer."""
import hashlib
import argparse
import json
from pathlib import Path
import sys

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
sys.path.insert(0, str(REPO / 'benchmark/cw122a'))

from analyze import analyze
from telemetry import NORMALIZATION_LIMITATIONS, cache_fairness, evaluate_pair, normalize_attempt, normalize_campaign, require


def replay(aggregate_path=None):
    aggregate_path = Path(aggregate_path) if aggregate_path else REPO / 'benchmark/cw122b1/mock-aggregate.json'
    require(aggregate_path.is_absolute() and aggregate_path.is_file(), 'aggregate must be an existing absolute file')
    require(aggregate_path == (REPO / 'benchmark/cw122b1/mock-aggregate.json') or
            not aggregate_path.resolve().is_relative_to(REPO.resolve()), 'private aggregate path must be outside checkout')
    campaign_path = REPO / 'benchmark/cw122a/campaign.json'
    schema_path = REPO / 'benchmark/cw122a/telemetry.schema.json'
    aggregate = json.loads(aggregate_path.read_text())
    campaign = json.loads(campaign_path.read_text())
    schema = json.loads(schema_path.read_text())
    from jsonschema import Draft202012Validator
    Draft202012Validator(schema).validate(normalize_campaign(aggregate, campaign))
    require(aggregate['status'] == 'MOCK_CODEX_CAMPAIGN_PASS', 'saved Codex mock campaign did not pass')
    require(aggregate['provider_calls'] == 0 and aggregate['provider_spend_usd'] == 0, 'provider inference/spend present in mock campaign')
    require(len(aggregate['attempts']) == 12 and sum(a['request_count'] for a in aggregate['attempts']) == 48, 'mock campaign count mismatch')
    traces = {family: json.loads((REPO / 'benchmark/cw122a' / task['trace']).read_text())
              for family, task in campaign['tasks'].items()}
    dataset = normalize_campaign(aggregate, campaign)
    analysis = analyze(dataset, campaign, traces)
    by_attempt = {(a['pair_id'], a['mode']): normalize_attempt(a) for a in aggregate['attempts']}
    cache_results = []
    for pair in campaign['pairs']:
        m1, m2 = (by_attempt[(pair['pair_id'], mode)] for mode in ('M1', 'M2'))
        status = cache_fairness(None, None)
        comparison = evaluate_pair(m1, m2, status)
        require(not comparison['eligible_for_economics'], 'unknown cache state unexpectedly qualified')
        cache_results.append({'pair_id': pair['pair_id'], 'cache_status': status,
                              'eligible_for_economics': comparison['eligible_for_economics'],
                              'm1_cost_usd': comparison['m1']['cost_usd'],
                              'm2_cost_usd': comparison['m2']['cost_usd']})
    require(analysis['product_value_verdict'] == 'NOT_EVALUATED', 'synthetic rehearsal claimed product value')
    require(analysis['overall']['complete_valid_pairs'] == 0, 'incomplete wire evidence unexpectedly formed a valid pair')
    return {
        'status': 'OFFLINE_READINESS_REPLAY_PASS', 'source_campaign_status': aggregate['status'],
        'source_campaign_sha256': hashlib.sha256(aggregate_path.read_bytes()).hexdigest(),
        'normalized_scope': dataset['scope'], 'normalized_runs': len(dataset['runs']),
        'mock_requests': sum(a['request_count'] for a in aggregate['attempts']),
        'provider_calls': aggregate['provider_calls'], 'provider_spend_usd': aggregate['provider_spend_usd'],
        'schema_valid': True, 'valid_pairs': analysis['overall']['complete_valid_pairs'],
        'product_value': analysis['product_value_verdict'], 'cache_pairs': cache_results,
        'normalization_limitations': NORMALIZATION_LIMITATIONS,
        'live': 'NO_GO; no authorization or live runner',
    }


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--aggregate', type=Path, help='saved sanitized mock aggregate; no network or inference')
    args = parser.parse_args()
    print(json.dumps(replay(args.aggregate), indent=2, sort_keys=True))
