"""Deterministic cache and usage gates; fixture data only."""
import copy
import json
from pathlib import Path
import tempfile
import unittest

from telemetry import NORMALIZATION_LIMITATIONS, analyze_attempt, cache_fairness, evaluate_pair, normalize_attempt

ROOT = Path(__file__).resolve().parents[2]


def load_rehearsal_module():
    import importlib.util
    spec = importlib.util.spec_from_file_location('cw122b2p_rehearse', ROOT / 'benchmark/cw122b2p/rehearse.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class TelemetryTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        aggregate = json.loads((ROOT / 'benchmark/cw122b1/mock-aggregate.json').read_text())
        cls.attempts = {(a['pair_id'], a['mode']): normalize_attempt(a) for a in aggregate['attempts']}

    def test_b1_usage_maps_without_claiming_cache_telemetry(self):
        run = self.attempts[('f1-r1', 'M1')]
        self.assertEqual(len(run['requests']), 4)
        self.assertEqual(run['requests'][1]['cached_input_tokens'], 32)
        self.assertEqual(run['requests'][1]['applied_rule_ids'], [])
        self.assertIsNotNone(run['requests'][1]['response_sha256'])
        self.assertEqual(run['spend_guard_admission_settlement'], 'NOT_AVAILABLE_IN_SANITIZED_B1_AGGREGATE')
        self.assertIsNone(run['requests'][1]['cache_write_tokens'])
        result = analyze_attempt(run)
        self.assertFalse(result['complete_usage'])
        self.assertIsNone(result['cost_usd'])
        m1 = self.attempts[('f1-r1', 'M1')]
        m2 = self.attempts[('f1-r1', 'M2')]
        self.assertFalse(evaluate_pair(m1, m2, 'CACHE_CONFUNDED_UNKNOWN_STATE')['eligible_for_economics'])

    def test_campaign_adapter_uses_frozen_schema_but_wire_gaps_block_pairs(self):
        import sys
        from jsonschema import Draft202012Validator
        sys.path.insert(0, str(ROOT / 'benchmark/cw122a'))
        from analyze import analyze
        campaign = json.loads((ROOT / 'benchmark/cw122a/campaign.json').read_text())
        schema = json.loads((ROOT / 'benchmark/cw122a/telemetry.schema.json').read_text())
        aggregate = json.loads((ROOT / 'benchmark/cw122b1/mock-aggregate.json').read_text())
        from telemetry import normalize_campaign
        dataset = normalize_campaign(aggregate, campaign)
        Draft202012Validator(schema).validate(dataset)
        traces = {family: json.loads((ROOT / 'benchmark/cw122a' / task['trace']).read_text())
                  for family, task in campaign['tasks'].items()}
        result = analyze(dataset, campaign, traces)
        self.assertEqual(len(dataset['runs']), 12)
        self.assertEqual(result['product_value_verdict'], 'NOT_EVALUATED')
        self.assertEqual(result['overall']['complete_valid_pairs'], 0)
        self.assertTrue(all(any('missing_wire_evidence' in issue for issue in pair['issues']) for pair in result['pairs']))
        self.assertIn('sentinel hash', NORMALIZATION_LIMITATIONS['repository_hash'])
        self.assertIn('unset', NORMALIZATION_LIMITATIONS['sampling'])

    def test_selected_provider_identity_is_not_the_historical_mock_label(self):
        profile = json.loads((ROOT / 'benchmark/cw122b2p/profile.json').read_text())
        self.assertEqual(profile['provider'], 'OpenAI API')
        self.assertEqual(profile['model'], 'gpt-6-luna')
        self.assertEqual(profile['protocol'], 'Responses API')
        self.assertNotEqual(profile['model'], 'gpt-4o-mini')
        self.assertFalse(profile['constraints']['live_inference_authorized'])
        self.assertIn('DOC_VERIFIED', profile['qualification']['codex_cli_model_compatibility'])
        self.assertIn('LIVE_NOT_VERIFIED', profile['qualification']['codex_cli_model_compatibility'])

    def test_offline_rehearsal_replays_saved_campaign_and_refuses_economic_qualification(self):
        module = load_rehearsal_module()
        result = module.replay()
        self.assertEqual(result['status'], 'OFFLINE_READINESS_REPLAY_PASS')
        self.assertEqual(result['normalized_runs'], 12)
        self.assertEqual(result['mock_requests'], 48)
        self.assertEqual(result['valid_pairs'], 0)
        self.assertEqual(result['product_value'], 'NOT_EVALUATED')
        self.assertTrue(all(not pair['eligible_for_economics'] for pair in result['cache_pairs']))

    def test_offline_rehearsal_rejects_symlinked_evidence_paths(self):
        module = load_rehearsal_module()
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            target_dir = root / 'actual'
            target_dir.mkdir()
            target_file = target_dir / 'aggregate.json'
            target_file.write_text('{}')
            file_link = root / 'file-link.json'
            file_link.symlink_to(target_file)
            dir_link = root / 'dir-link'
            dir_link.symlink_to(target_dir, target_is_directory=True)
            for path in (file_link, dir_link / 'aggregate.json'):
                with self.subTest(path=path), self.assertRaisesRegex(ValueError, 'symlinks'):
                    module.replay(path)

    def test_cache_cases_a_through_e_fail_closed(self):
        matched = {'telemetry_complete': True, 'initial_state_hash': 'a' * 64, 'isolation_verified': True}
        self.assertEqual(cache_fairness(matched, matched), 'CACHE_MATCHED_TELEMETRY_COMPLETE')
        self.assertEqual(cache_fairness(None, matched), 'CACHE_CONFUNDED_UNKNOWN_STATE')
        missing = {**matched, 'initial_state_hash': None}
        self.assertEqual(cache_fairness(missing, missing), 'CACHE_CONFUNDED_UNKNOWN_STATE')
        blank = {**matched, 'initial_state_hash': ''}
        self.assertEqual(cache_fairness(blank, blank), 'CACHE_CONFUNDED_UNKNOWN_STATE')
        invalid = {**matched, 'initial_state_hash': 'not-a-sha256'}
        self.assertEqual(cache_fairness(invalid, invalid), 'CACHE_CONFUNDED_UNKNOWN_STATE')
        self.assertEqual(cache_fairness({**matched, 'telemetry_complete': False}, matched), 'CACHE_CONFUNDED_INCOMPLETE_TELEMETRY')
        self.assertEqual(cache_fairness(matched, {**matched, 'initial_state_hash': 'b' * 64}), 'CACHE_CONFUNDED_UNMATCHED_INITIAL_STATE')
        self.assertEqual(cache_fairness(matched, {**matched, 'isolation_verified': False}), 'CACHE_CONFUNDED_ISOLATION_UNVERIFIED')

    def test_missing_cached_usage_invalidates_economics(self):
        run = copy.deepcopy(self.attempts[('f1-r1', 'M1')])
        run['requests'][0]['raw_usage']['input_tokens_details'].pop('cached_tokens')
        result = analyze_attempt(run)
        self.assertFalse(result['complete_usage'])
        self.assertIsNone(result['cost_usd'])

    def test_complete_usage_costs_disjoint_cache_categories_and_inversion(self):
        def run(mode, input_tokens, cached, write30, output):
            raw = {'input_tokens': input_tokens, 'output_tokens': output,
                   'total_tokens': input_tokens + output,
                   'input_tokens_details': {'cached_tokens': cached, 'cache_write_tokens': write30,
                                            'cache_write_tokens_details': {'ttl_30m_tokens': write30, 'ttl_12h_tokens': 0}}}
            return {'pair_id': 'p', 'family': 'f1', 'mode': mode, 'request_count_reported': 1,
                    'requests': [{'index': 0, 'raw_usage': raw}],
                    'treatment': {'status': 'verified_applied', 'verified_requests': 1} if mode == 'M2' else {}}
        m1 = run('M1', 1000, 500, 100, 100)
        m2 = run('M2', 900, 0, 900, 300)
        self.assertTrue(analyze_attempt(m1)['complete_usage'])
        result = evaluate_pair(m1, m2, 'CACHE_MATCHED_TELEMETRY_COMPLETE')
        self.assertTrue(result['eligible_for_economics'])
        self.assertGreater(float(result['m2_cost_minus_m1_usd']), 0)
        self.assertIn('COST_INVERSION_M2_MORE_EXPENSIVE', result['warnings'])
        self.assertFalse(evaluate_pair(m1, m2, 'CACHE_CONFUNDED_UNKNOWN_STATE')['eligible_for_economics'])
        self.assertFalse(evaluate_pair(m1, {**m2, 'treatment': {}}, 'CACHE_MATCHED_TELEMETRY_COMPLETE')['eligible_for_economics'])

    def test_contradictory_provider_usage_is_rejected(self):
        run = {'request_count_reported': 1, 'requests': [{'index': 0, 'raw_usage': {
            'input_tokens': 100, 'output_tokens': 10, 'total_tokens': 111,
            'input_tokens_details': {'cached_tokens': 0, 'cache_write_tokens': 0,
                                     'cache_write_tokens_details': {'ttl_30m_tokens': 0, 'ttl_12h_tokens': 0}},
        }}]}
        with self.assertRaisesRegex(ValueError, 'total_tokens contradiction'):
            analyze_attempt(run)

    def test_extra_requests_are_included_in_complete_session_cost(self):
        request = {'input_tokens': 100, 'output_tokens': 10, 'total_tokens': 110,
                   'input_tokens_details': {'cached_tokens': 0, 'cache_write_tokens': 0,
                                            'cache_write_tokens_details': {'ttl_30m_tokens': 0, 'ttl_12h_tokens': 0}}}
        one = {'request_count_reported': 1, 'requests': [{'index': 0, 'raw_usage': request}]}
        two = copy.deepcopy(one)
        two['request_count_reported'] = 2
        two['requests'].append({'index': 1, 'raw_usage': copy.deepcopy(request)})
        first_cost, session_cost = analyze_attempt(one)['cost_usd'], analyze_attempt(two)['cost_usd']
        self.assertEqual(len(two['requests']), 2)
        self.assertEqual(float(session_cost), 2 * float(first_cost))

    def test_pricing_uses_long_context_rates_for_full_request_above_threshold(self):
        def run(input_tokens):
            usage = {'input_tokens': input_tokens, 'output_tokens': 1,
                     'total_tokens': input_tokens + 1,
                     'input_tokens_details': {'cached_tokens': 0, 'cache_write_tokens': 0,
                                              'cache_write_tokens_details': {'ttl_30m_tokens': 0, 'ttl_12h_tokens': 0}}}
            return {'request_count_reported': 1, 'requests': [{'index': 0, 'raw_usage': usage}]}

        short = analyze_attempt(run(272000))['cost_usd']
        long = analyze_attempt(run(272001))['cost_usd']
        self.assertEqual(short, '0.0272005')
        self.assertEqual(long, '0.05440095')


if __name__ == '__main__':
    unittest.main()
