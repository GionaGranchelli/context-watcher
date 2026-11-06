"""Offline adapter and cost/cache gates for the CW-122B1 mock aggregate."""
from decimal import Decimal
import hashlib
import json
from pathlib import Path

PRICING = json.loads((Path(__file__).parent / 'pricing-2026-11-05.json').read_text())
NORMALIZATION_LIMITATIONS = {
    'repository_hash': 'The valid-format hash is a sentinel hash of an absence statement, not a source-tree hash.',
    'initial_fixture_hashes': 'NOT_AVAILABLE_IN_SANITIZED_B1_AGGREGATE',
    'wire': 'Null because raw ingress/egress payloads are absent; every pair is ineligible for efficiency analysis.',
    'cache_write_usage': 'NOT_AVAILABLE; economics are blocked by telemetry.analyze_attempt.',
    'acceptance': 'Boolean summary from the B1 scripted fixture-oracle campaign; raw oracle output is not copied.',
    'sampling': 'B1 observed temperature/top_p were unset; frozen CW-122A planned values occupy schema-required fields and are not B1 observations.',
    'source_attempt_status': 'B1 mock aggregate reports the scripted attempts passed; normalized records are marked invalid because required independent wire payload evidence is absent.',
    'spend_guard': 'Per-request admission and settlement outcomes are NOT_AVAILABLE in the sanitized B1 aggregate.',
    'retries': 'Codex retry count is NOT_AVAILABLE; aggregate records this status explicitly.',
}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def normalize_attempt(attempt):
    """Map saved B1 mock telemetry into a review dataset without inventing cache evidence."""
    requests = []
    for item in attempt['requests']:
        raw = item.get('provider_reported_usage')
        usage = raw if isinstance(raw, dict) else {}
        details = usage.get('input_tokens_details') or {}
        cached = details.get('cached_tokens')
        writes = details.get('cache_write_tokens')
        requests.append({
            'index': item['index'], 'status': item['status'], 'model': item['model'],
            'sampling': item.get('sampling'),
            'input_tokens': usage.get('input_tokens'), 'cached_input_tokens': cached,
            'cache_write_tokens': writes,
            'cache_write_ttl_30m_tokens': (details.get('cache_write_tokens_details') or {}).get('ttl_30m_tokens'),
            'cache_write_ttl_12h_tokens': (details.get('cache_write_tokens_details') or {}).get('ttl_12h_tokens'),
            'output_tokens': usage.get('output_tokens'), 'total_tokens': usage.get('total_tokens'),
            'usage_provenance': item.get('usage_provenance', 'NOT_AVAILABLE'),
            'request_hashes': {'ingress_sha256': item.get('ingress_sha256'), 'egress_sha256': item.get('egress_sha256')},
            'response_sha256': item.get('response_sha256'),
            'applied_rule_ids': item.get('applied_rule_ids'), 'mutation_skips': item.get('mutation_skips'),
            'raw_usage': usage,
        })
    return {
        'pair_id': attempt['pair_id'], 'family': attempt['family'], 'mode': attempt['mode'],
        'acceptance': attempt.get('acceptance_passed'), 'request_count_reported': attempt.get('request_count'),
        'requests': requests, 'coding_wall_ms': attempt.get('coding_wall_ms'),
        'complete_session_wall_ms': attempt.get('complete_session_wall_ms'),
        'bootstrap_wall_ms': attempt.get('bootstrap_wall_ms'),
        'provider_errors': attempt.get('provider_error_count'), 'tool_errors': attempt.get('tool_error_count'),
        'retry_count': attempt.get('retry_count'), 'retry_count_status': attempt.get('retry_count_status'),
        'spend_guard_admission_settlement': 'NOT_AVAILABLE_IN_SANITIZED_B1_AGGREGATE',
        'treatment': attempt.get('treatment'),
        'transformation_verification': 'HASH_AND_LEDGER_ATTESTATION_ONLY; raw egress unavailable in public aggregate',
    }


def normalize_campaign(aggregate, campaign):
    """Convert B1's saved mock attempts to the frozen CW-122A run envelope."""
    pairs = {pair['pair_id']: pair for pair in campaign['pairs']}
    runs = []
    for attempt in aggregate['attempts']:
        pair = pairs[attempt['pair_id']]
        normalized = normalize_attempt(attempt)
        requests = []
        for item in normalized['requests']:
            raw = item['raw_usage']
            details = raw.get('input_tokens_details') or {}
            requests.append({
                'input_tokens': raw.get('input_tokens'),
                'cached_input_tokens': details.get('cached_tokens'),
                'output_tokens': raw.get('output_tokens'),
                'usage_provenance': 'synthetic_mock_usage', 'raw_usage': raw,
                'first_byte_latency_ms': None, 'first_byte_provenance': 'NOT_AVAILABLE_MOCK',
            })
        passed = attempt.get('acceptance_passed') is True
        mode = attempt['mode']
        sample = {'temperature': campaign['sampling']['temperature'], 'top_p': campaign['sampling']['top_p'],
                  'seed': None, 'nondeterminism': 'CW-122A planned fields used for schema; B1 recorded temperature/top_p unset'}
        runs.append({
            'pair_id': attempt['pair_id'], 'family': attempt['family'],
            'task_id': campaign['tasks'][attempt['family']]['id'], 'repetition': pair['repetition'],
            'mode': mode, 'order': pair['order'].index(mode),
            'session_id': attempt['pair_id'] + '-' + mode, 'attempt_id': attempt['pair_id'] + '-' + mode,
            'status': 'invalid', 'failure_reason': 'wire payload absent from sanitized aggregate; normalized observation is not analyzable',
            'repository_hash': hashlib.sha256(b'CW122B2P:repository hash unavailable in sanitized B1 aggregate').hexdigest(),
            'initial_fixture_hashes': {'unavailable': hashlib.sha256(b'CW122B2P:initial fixture hash unavailable in sanitized B1 aggregate').hexdigest()},
            'configuration_hash': aggregate.get('campaign_sha256', 'NOT_AVAILABLE'),
            'runtime': {'provider': 'loopback mock', 'model': 'synthetic:' + (normalized['requests'][0]['model'] if normalized['requests'] else 'NOT_AVAILABLE'),
                        'agent_version': aggregate.get('codex_version', 'NOT_AVAILABLE'),
                        'model_revision': 'NOT_APPLICABLE_SYNTHETIC', 'server_version': aggregate.get('context_watcher_commit', 'NOT_AVAILABLE')},
            'sampling': sample,
            'requests': requests,
            'duration_ms': attempt.get('complete_session_wall_ms'),
            'duration_provenance': 'MOCK_CAMPAIGN_WALL_CLOCK_NOT_PROVIDER_LATENCY',
            'acceptance': {'passed': passed, 'functional_passed': passed, 'constraint_violations': []},
            'wire': None,
        })
    return {'schema': 'cw122a-runs-v1', 'scope': 'SYNTHETIC_REHEARSAL', 'runs': runs}


def analyze_attempt(run, pricing=PRICING):
    """Calculate complete-session economics only when every billing category is observed."""
    requests = run['requests']
    require(len(requests) == run['request_count_reported'], 'request count disagrees with aggregate')
    missing = []
    cost = Decimal(0)
    for req in requests:
        raw = req.get('raw_usage') or {}
        details = raw.get('input_tokens_details')
        if not isinstance(details, dict):
            missing.append((req['index'], 'input_tokens_details'))
            continue
        fields = {
            'input_tokens': raw.get('input_tokens'), 'cached_input_tokens': details.get('cached_tokens'),
            'cache_write_tokens': details.get('cache_write_tokens'),
            'cache_write_ttl_30m_tokens': (details.get('cache_write_tokens_details') or {}).get('ttl_30m_tokens'),
            'cache_write_ttl_12h_tokens': (details.get('cache_write_tokens_details') or {}).get('ttl_12h_tokens'),
            'output_tokens': raw.get('output_tokens'),
        }
        for field, value in fields.items():
            if type(value) is not int or value < 0:
                missing.append((req['index'], field))
        if all(type(v) is int and v >= 0 for v in fields.values()):
            require(fields['cached_input_tokens'] + fields['cache_write_tokens'] <= fields['input_tokens'], 'cached/write tokens exceed input')
            require(fields['cache_write_ttl_30m_tokens'] + fields['cache_write_ttl_12h_tokens'] == fields['cache_write_tokens'], 'cache-write lifetime breakdown disagrees')
            require(raw.get('total_tokens') == fields['input_tokens'] + fields['output_tokens'], 'total_tokens contradiction')
            uncached = fields['input_tokens'] - fields['cached_input_tokens'] - fields['cache_write_tokens']
            tier_name = ('long_context_over_272000_input_tokens'
                         if fields['input_tokens'] > pricing['long_context_threshold_tokens']
                         else 'short_context_up_to_272000_input_tokens')
            rates = pricing['context_pricing_tiers'][tier_name]
            cost += (
                Decimal(uncached) * Decimal(rates['uncached_input'])
                + Decimal(fields['cached_input_tokens']) * Decimal(rates['cached_input'])
                + Decimal(fields['cache_write_ttl_30m_tokens']) * Decimal(rates['cache_write_ttl_30m'])
                + Decimal(fields['cache_write_ttl_12h_tokens']) * Decimal(rates['cache_write_ttl_12h'])
                + Decimal(fields['output_tokens']) * Decimal(rates['output'])
            ) / Decimal(1_000_000)
    if missing:
        return {'complete_usage': False, 'missing_fields': missing, 'cost_usd': None, 'cache_fairness': 'NOT_EVALUATED'}
    return {'complete_usage': True, 'missing_fields': [], 'cost_usd': str(cost), 'cache_fairness': 'NOT_EVALUATED'}


def cache_fairness(arm_a, arm_b):
    """A: qualify only when independently reset/matched; otherwise keep economics confounded."""
    if not arm_a or not arm_b:
        return 'CACHE_CONFUNDED_UNKNOWN_STATE'
    if arm_a.get('telemetry_complete') is not True or arm_b.get('telemetry_complete') is not True:
        return 'CACHE_CONFUNDED_INCOMPLETE_TELEMETRY'
    state_hashes = (arm_a.get('initial_state_hash'), arm_b.get('initial_state_hash'))
    if any(not isinstance(value, str) or len(value) != 64 or
           any(char not in '0123456789abcdef' for char in value) for value in state_hashes):
        return 'CACHE_CONFUNDED_UNKNOWN_STATE'
    if state_hashes[0] != state_hashes[1]:
        return 'CACHE_CONFUNDED_UNMATCHED_INITIAL_STATE'
    if arm_a.get('isolation_verified') is not True or arm_b.get('isolation_verified') is not True:
        return 'CACHE_CONFUNDED_ISOLATION_UNVERIFIED'
    return 'CACHE_MATCHED_TELEMETRY_COMPLETE'


def evaluate_pair(m1, m2, cache_status):
    """Conservative pair eligibility and visible inversion warning."""
    a, b = analyze_attempt(m1), analyze_attempt(m2)
    treatment = m2.get('treatment') or {}
    verified = treatment.get('status') in ('verified_applied', 'verified_mock_wire_treatment') and treatment.get('verified_requests', 0) > 0
    eligible = a['complete_usage'] and b['complete_usage'] and verified and cache_status == 'CACHE_MATCHED_TELEMETRY_COMPLETE'
    delta = None if a['cost_usd'] is None or b['cost_usd'] is None else str(Decimal(b['cost_usd']) - Decimal(a['cost_usd']))
    return {'eligible_for_economics': eligible, 'm1': a, 'm2': b, 'cache_status': cache_status,
            'm2_cost_minus_m1_usd': delta,
            'warnings': ['COST_INVERSION_M2_MORE_EXPENSIVE'] if delta is not None and Decimal(delta) > 0 else []}
