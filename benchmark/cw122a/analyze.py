"""Matched analysis only. Null telemetry stays unavailable; synthetic data cannot establish value."""
import copy
from decimal import Decimal
import math
import statistics


def require(ok, message):
    if not ok:
        raise ValueError(message)


def verify_wire(record, campaign, original):
    mode = record['mode']
    expected = copy.deepcopy(original)
    if mode == 'M2':
        expected['input'][2]['output'] = campaign['tasks'][record['family']]['stub']
    require(record['original'] == original, 'original trace mismatch')
    require(record['outbound'] == expected, 'unexpected outbound transformation')
    require(record['applied_mutation_ids'] == (['historical-prune'] if mode == 'M2' else []), 'wire/ledger mismatch')
    require(record['request_count'] == 1, 'wire request-count mismatch')
    if mode == 'M2':
        require(record['treatment_verified'] is True and expected != original, 'absent pruning treatment')
        require(len(expected['input'][2]['output']) < len(original['input'][2]['output']), 'non-reducing treatment')
    else:
        require(record['treatment_verified'] is False, 'passthrough incorrectly treated')
    return True


def total(requests, key):
    values = [request.get(key) for request in requests]
    if not values or any(v is None for v in values):
        return None
    require(all(type(v) is int and v >= 0 for v in values), 'invalid usage ' + key)
    return sum(values)


def metrics(run, pricing):
    requests = run['requests']
    require(isinstance(requests, list), 'invalid request records')
    for request in requests:
        require(request.get('usage_provenance') in ['synthetic_mock_usage', 'provider_response_usage'], 'usage provenance missing')
        for key in ['input_tokens', 'cached_input_tokens', 'output_tokens']:
            value = request.get(key)
            require(value is None or (type(value) is int and value >= 0), 'invalid usage ' + key)
        raw = request.get('raw_usage')
        require(isinstance(raw, dict), 'raw usage provenance missing')
        raw_values = {'input_tokens':raw.get('input_tokens',raw.get('prompt_tokens')),
                      'output_tokens':raw.get('output_tokens',raw.get('completion_tokens')),
                      'cached_input_tokens':raw.get('input_tokens_details',raw.get('prompt_tokens_details',{})).get('cached_tokens')}
        require(all(request.get(k) is None or request[k] == raw_values[k] for k in raw_values), 'usage disagrees with raw provider/mock response')
        i, c = request.get('input_tokens'), request.get('cached_input_tokens')
        require(i is None or c is None or c <= i, 'cached exceeds input')
    inp, cached, out = (total(requests, key) for key in ['input_tokens', 'cached_input_tokens', 'output_tokens'])
    uncached = inp - cached if inp is not None and cached is not None else None
    cost = None
    if inp is not None and cached is not None and out is not None:
        cost = float((Decimal(uncached) * Decimal(pricing['uncached_input_per_million']) + Decimal(cached) * Decimal(pricing['cached_input_per_million']) + Decimal(out) * Decimal(pricing['output_per_million'])) / Decimal(1_000_000))
    wire = run.get('wire')
    return {'input_tokens': inp, 'cached_input_tokens': cached, 'uncached_input_tokens': uncached, 'output_tokens': out,
            'calculated_cost_usd': cost, 'provider_requests': len(requests), 'duration_ms': run.get('duration_ms'),
            'missing_usage_records': sum(any(r.get(k) is None for k in ['input_tokens','cached_input_tokens','output_tokens']) for r in requests),
            'mechanical_estimated_reduction_tokens': wire.get('estimated_pruned_tokens') if wire and run['mode'] == 'M2' else (0 if wire else None),
            'estimated_kv_break_tokens': wire.get('estimated_kv_break_tokens') if wire and run['mode'] == 'M2' else (0 if wire else None)}


def delta(before, after):
    if before is None or after is None:
        return {'absolute': None, 'relative_pct': None}
    return {'absolute': after - before, 'relative_pct': 100 * (after - before) / before if before else None}


def summarize(values):
    values = [v for v in values if v is not None]
    if not values:
        return {'n': 0, 'mean': None, 'median': None, 'range': None, 'sample_sd': None, 'mean_se': None}
    sd = statistics.stdev(values) if len(values) > 1 else None
    return {'n': len(values), 'mean': statistics.mean(values), 'median': statistics.median(values), 'range': [min(values), max(values)],
            'sample_sd': sd, 'mean_se': sd / math.sqrt(len(values)) if sd is not None else None}


def analyze(dataset, campaign, traces):
    require(dataset['schema'] == 'cw122a-runs-v1', 'unsupported dataset')
    synthetic = dataset['scope'] == 'SYNTHETIC_REHEARSAL'
    require(synthetic or dataset['scope'] == 'LIVE_PROVIDER', 'unknown measurement scope')
    require(synthetic or campaign['pricing']['provenance'] != 'SYNTHETIC_TEST_ONLY_NOT_A_PROVIDER_PRICE', 'live data requires verified pricing snapshot')
    scheduled = {p['pair_id']: p for p in campaign['pairs']}
    require(len(scheduled) == len(campaign['pairs']), 'duplicate planned pair')
    sessions, attempts, runs, outcomes = set(), set(), {}, []
    for run in dataset['runs']:
        pair, mode = run['pair_id'], run['mode']
        require(pair in scheduled and mode in ['M1','M2'], 'unmatched mode or pair')
        plan = scheduled[pair]
        require(run['session_id'] == pair + '-' + mode and run['session_id'] not in sessions, 'duplicate or mismatched session ID')
        require(run['attempt_id'] not in attempts, 'duplicate attempt ID')
        require(run['family'] == plan['family'] and run['task_id'] == campaign['tasks'][plan['family']]['id'], 'mismatched task family')
        require(run['repetition'] == plan['repetition'] and run['order'] == plan['order'].index(mode), 'mismatched execution order')
        require(type(run['acceptance']['passed']) is bool, 'acceptance must be explicit boolean')
        require(run['acceptance']['passed'] == (run['acceptance']['functional_passed'] is True and not run['acceptance']['constraint_violations']), 'inconsistent acceptance outcome')
        sessions.add(run['session_id']); attempts.add(run['attempt_id'])
        issues = []
        if run['status'] != 'completed':
            issues.append(run.get('failure_reason') or 'non-completed attempt')
        try:
            require(isinstance(run.get('wire'),dict), 'missing_wire_evidence')
            verify_wire(run['wire'], campaign, traces[run['family']])
        except (KeyError, ValueError) as error:
            issues.append(str(error))
        if isinstance(run.get('wire'),dict):
            require(run['wire']['session_id'] == run['session_id'] and run['wire']['mode'] == mode, 'mismatched wire session/mode')
        measurement = metrics(run, campaign['pricing'])
        for request in run['requests']:
            require((request['usage_provenance'] == 'synthetic_mock_usage') == synthetic, 'mixed synthetic/live provenance')
        if measurement['missing_usage_records'] or any(measurement[k] is None for k in ['input_tokens','cached_input_tokens','output_tokens','duration_ms']):
            issues.append('missing_or_partial_usage')
        duration = run.get('duration_ms')
        require(duration is None or (type(duration) in [float,int] and math.isfinite(duration) and duration >= 0), 'invalid duration')
        record = {'session_id':run['session_id'],'mode':mode,'status':run['status'],'acceptance':run['acceptance'],'issues':issues,'metrics':measurement}
        outcomes.append(record); runs[(pair,mode)] = (run,record)
    pairs = []
    for pair, plan in scheduled.items():
        members = [runs.get((pair, mode)) for mode in ['M1','M2']]
        issues = [f'{mode}:missing_attempt' for mode, member in zip(['M1','M2'],members) if member is None]
        if any(member is None for member in members):
            pairs.append({'pair_id':pair,'family':plan['family'],'eligible_for_efficiency':False,'issues':issues,'differences':{}})
            continue
        (a, ar), (b, br) = members
        for key in ['repository_hash','configuration_hash','runtime','sampling']:
            if a[key] != b[key]: issues.append('unmatched_' + key)
        issues += ['M1:' + i for i in ar['issues']] + ['M2:' + i for i in br['issues']]
        diffs = {key:delta(ar['metrics'][key], br['metrics'][key]) for key in ar['metrics'] if key != 'missing_usage_records'}
        warnings = []
        if diffs['calculated_cost_usd']['absolute'] is not None and diffs['calculated_cost_usd']['absolute'] > 0:
            warnings.append('COST_INVERSION_M2_MORE_EXPENSIVE')
            if diffs['cached_input_tokens']['absolute'] is not None and diffs['cached_input_tokens']['absolute'] < 0:
                warnings.append('CACHE_COST_INVERSION; association, not causal attribution')
        pairs.append({'pair_id':pair,'family':plan['family'],'eligible_for_efficiency':not issues,'issues':issues,'warnings':warnings,'differences':diffs,
                      'acceptance_difference':int(b['acceptance']['passed'])-int(a['acceptance']['passed'])})
    def group(selected):
        valid = [p for p in selected if p['eligible_for_efficiency']]
        return {'planned_pairs':len(selected),'complete_valid_pairs':len(valid),'excluded_pairs':[p['pair_id'] for p in selected if not p['eligible_for_efficiency']],
                'changes':{key:{kind:summarize(p['differences'][key][kind] for p in valid) for kind in ['absolute','relative_pct']} for key in ['input_tokens','cached_input_tokens','uncached_input_tokens','output_tokens','calculated_cost_usd','provider_requests','duration_ms']},
                'input_change_pct':summarize(p['differences']['input_tokens']['relative_pct'] for p in valid),
                'cost_change_pct':summarize(p['differences']['calculated_cost_usd']['relative_pct'] for p in valid)}
    groups = {family:group([p for p in pairs if p['family']==family]) for family in campaign['tasks']}
    whole = group(pairs)
    m2_added_failures = sum(p.get('acceptance_difference',0)<0 for p in pairs)
    decision = 'INCONCLUSIVE'
    if m2_added_failures:
        decision = 'NEGATIVE_CORRECTNESS'
    elif whole['complete_valid_pairs'] >= campaign['decision']['minimum_valid_pairs'] and all(g['complete_valid_pairs'] >= campaign['decision']['minimum_pairs_per_family'] for g in groups.values()):
        i, c = whole['input_change_pct']['median'], whole['cost_change_pct']['median']
        if (i <= campaign['decision']['median_input_delta_pct_at_most'] and c <= campaign['decision']['median_cost_delta_pct_at_most']
                and all(g['input_change_pct']['median'] <= 0 and g['cost_change_pct']['median'] <= 0 for g in groups.values())
                and all(p['differences']['calculated_cost_usd']['absolute'] <= 0 for p in pairs)
                and all(r['acceptance']['passed'] and r['status']=='completed' for r in outcomes)):
            decision = 'POSITIVE_PILOT_ONLY'
        elif i > 0 or c > 0:
            decision = 'NEGATIVE_EFFICIENCY'
    return {'scope':dataset['scope'],'pricing_version':campaign['pricing']['version'],'product_value_verdict':'NOT_EVALUATED' if synthetic else decision,'rehearsed_decision':decision,
            'all_attempts':outcomes,'pairs':pairs,'overall':whole,'families':groups,
            'reliability':{mode:{'attempts':sum(r['mode']==mode for r in outcomes),'accepted':sum(r['mode']==mode and r['acceptance']['passed'] and r['status']=='completed' for r in outcomes),'failed_or_aborted':sum(r['mode']==mode and (not r['acceptance']['passed'] or r['status']!='completed') for r in outcomes)} for mode in ['M1','M2']},
            'uncertainty_note':'Range, sample SD and mean SE are descriptive; at three pairs per family no significance, causal or population claim is justified.',
            'correctness_note':'Equal acceptance in both arms does not establish improved reliability.',
            'mechanical_note':'Local request token reductions are estimates on a frozen request. Complete-session provider usage includes all retries and behavioral changes; do not conflate them.'}

if __name__ == '__main__':
    import argparse
    import json
    from pathlib import Path
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('dataset',type=Path)
    parser.add_argument('--campaign',type=Path,default=Path(__file__).with_name('campaign.json'))
    args=parser.parse_args()
    configuration=json.loads(args.campaign.read_text())
    source_traces={family:json.loads((args.campaign.parent/task['trace']).read_text()) for family,task in configuration['tasks'].items()}
    print(json.dumps(analyze(json.loads(args.dataset.read_text()),configuration,source_traces),indent=2,sort_keys=True))
