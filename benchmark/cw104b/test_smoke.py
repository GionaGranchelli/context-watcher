"""Admission/default-mode tests. Provider networking is forbidden in every case."""
import contextlib
from datetime import datetime, timedelta, timezone
import io
import json
from pathlib import Path
import runpy
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import smoke


class SmokeSafetyTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='cw-smoke-admission-')
        self.addCleanup(self.temporary.cleanup)
        self.base = Path(self.temporary.name)
        self.options = SimpleNamespace(live=True, endpoint='http://127.0.0.1:8095',
            model='offline-test-model', provider='offline-test-provider',
            evidence_dir=self.base / 'private', authorization_file=self.base / 'approval.json')
        self.now = datetime.now(timezone.utc)
        self.document = {'authorization_id': 'test-only-approval', 'approved_by': 'unit-test-only',
            'approval_reference': 'not authorization for real inference',
            'issued_at': (self.now - timedelta(seconds=10)).isoformat(),
            'expires_at': (self.now + timedelta(minutes=10)).isoformat(),
            'scope': 'one-local-m1-m2-smoke-pair', 'max_pairs': 1, 'max_seconds': 3600,
            'max_provider_requests': 30, 'cloud_spend_allowed': False,
            'endpoint': self.options.endpoint, 'model': self.options.model,
            'provider': self.options.provider, 'evidence_dir': str(self.options.evidence_dir.resolve()),
            'runner_freeze_sha256': smoke.verify_runner_freeze()}
        self.network = patch.object(smoke.urllib.request, 'urlopen', side_effect=AssertionError('No provider calls allowed')).start()
        self.opener_network = patch.object(smoke.urllib.request.OpenerDirector, 'open', side_effect=AssertionError('No provider calls allowed')).start()
        self.addCleanup(patch.stopall)

    def authorize(self, document=None):
        self.options.authorization_file.write_text(json.dumps(document or self.document))
        return smoke.live_authorization(self.options, self.document['runner_freeze_sha256'], now=self.now)

    def invoke(self, flags):
        with contextlib.redirect_stdout(io.StringIO()):
            code = smoke.main(['--manifest-dir', str(self.base / 'manifests')] + flags)
        manifest = json.loads(next((self.base / 'manifests').glob('*.json')).read_text())
        self.network.assert_not_called()
        self.opener_network.assert_not_called()
        return code, manifest

    def owner_flags(self):
        return ['--model-profile', str(self.base / 'models.json'), '--provider', self.options.provider,
                '--model', self.options.model, '--evidence-dir', str(self.options.evidence_dir),
                '--history-file', str(self.base / 'private-history.txt')]

    def test_no_arguments_choose_portable_offline_without_owner_data(self):
        with patch.object(smoke.subprocess, 'run', return_value=SimpleNamespace(returncode=0)) as cargo, \
                patch.object(smoke, 'run_owner') as owner, patch.dict('sys.modules', {'yaml': None}):
            code, manifest = self.invoke([])
        self.assertEqual(code, 0)
        self.assertEqual(manifest['mode'], 'offline-fixture')
        self.assertEqual(manifest['status'], 'OFFLINE_FIXTURE_PASS')
        self.assertEqual(manifest['authorization'], {'record_valid': False, 'consumed': False})
        self.assertEqual(manifest['inference_calls'], 0)
        self.assertIn('pin_lifecycle', cargo.call_args.args[0])
        owner.assert_not_called()

    def test_explicit_mock_alias_is_offline(self):
        with patch.object(smoke.subprocess, 'run', return_value=SimpleNamespace(returncode=0)):
            code, manifest = self.invoke(['--mock-only'])
        self.assertEqual(code, 0)
        self.assertEqual(manifest['mode'], 'offline-fixture')

    def test_live_switch_alone_cannot_start_owner_execution(self):
        with patch.object(smoke, 'run_owner') as owner:
            code, manifest = self.invoke(['--live'])
        self.assertEqual(code, 2)
        self.assertEqual(manifest['status'], 'PREFLIGHT_BLOCKED')
        self.assertFalse(manifest['authorization']['record_valid'])
        owner.assert_not_called()

    def test_complete_environment_without_authorization_is_denied(self):
        with patch.object(smoke, 'run_owner') as owner, patch.object(smoke, 'model_profile') as profile:
            code, _ = self.invoke(['--live', '--endpoint', self.options.endpoint] + self.owner_flags())
        self.assertEqual(code, 2)
        profile.assert_not_called()
        owner.assert_not_called()

    def test_authorization_file_does_not_imply_live_switch(self):
        code, _ = self.invoke(['--authorization-file', str(self.options.authorization_file)])
        self.assertEqual(code, 2)

    def test_missing_authorization_file_blocks_before_profile_loading(self):
        with patch.object(smoke, 'run_owner') as owner, patch.object(smoke, 'model_profile') as profile:
            code, manifest = self.invoke(['--live', '--endpoint', self.options.endpoint,
                '--authorization-file', str(self.options.authorization_file)] + self.owner_flags())
        self.assertEqual(code, 2)
        self.assertEqual(manifest['provider_requests_admitted'], 0)
        profile.assert_not_called()
        owner.assert_not_called()

    def test_valid_attestation_is_recorded_and_consumed_without_executing_it(self):
        self.authorize()
        def owner_double(options, result, public):
            result['status'] = 'TEST_DOUBLE_NO_INFERENCE'
            return True
        with patch.object(smoke, 'run_owner', side_effect=owner_double):
            code, manifest = self.invoke(['--live', '--endpoint', self.options.endpoint,
                '--authorization-file', str(self.options.authorization_file)] + self.owner_flags())
        self.assertEqual(code, 0)
        self.assertTrue(manifest['authorization']['record_valid'])
        self.assertTrue(manifest['authorization']['consumed'])
        self.assertEqual(manifest['authorization']['approval_reference'], self.document['approval_reference'])
        self.assertNotIn('evidence_dir', manifest['authorization'])
        self.assertNotIn('inference_calls', manifest)  # Never synthesize live usage as zero.

    def test_approval_is_single_use_in_its_bound_directory(self):
        authorization = self.authorize()
        self.options.evidence_dir.mkdir(mode=0o700)
        smoke.claim_authorization(self.options.evidence_dir, authorization)
        with self.assertRaisesRegex(ValueError, 'already been consumed'):
            smoke.claim_authorization(self.options.evidence_dir, authorization)

    def test_stale_future_naive_and_oversized_time_windows_are_denied(self):
        cases = [
            {'issued_at': (self.now - timedelta(days=2)).isoformat(), 'expires_at': (self.now - timedelta(days=1)).isoformat()},
            {'issued_at': (self.now + timedelta(minutes=1)).isoformat()},
            {'issued_at': self.now.replace(tzinfo=None).isoformat()},
            {'expires_at': (self.now + timedelta(days=2)).isoformat()},
        ]
        for changes in cases:
            with self.subTest(changes=changes), self.assertRaises(ValueError):
                self.authorize(dict(self.document, **changes))

    def test_scope_configuration_and_budget_mismatches_are_denied(self):
        for changes in [{'scope':'another-campaign'}, {'max_pairs':2}, {'max_pairs':True},
                        {'max_seconds':3601}, {'max_provider_requests':31}, {'cloud_spend_allowed':True},
                        {'runner_freeze_sha256':'changed'}, {'model':'another-model'},
                        {'provider':'another-provider'}, {'endpoint':'http://127.0.0.1:8096'},
                        {'evidence_dir':str(self.base / 'different')}, {'approval_reference':''},
                        {'apiKey':'must-not-be-recorded'}]:
            with self.subTest(changes=changes), self.assertRaises(ValueError):
                self.authorize(dict(self.document, **changes))

    def test_public_dns_credentials_and_non_origin_endpoints_are_denied(self):
        for endpoint in ['https://api.openai.com', 'http://8.8.8.8',
                         'http://user:secret@127.0.0.1:8095', 'http://127.0.0.1:8095/v1',
                         'http://127.0.0.1:8095?key=secret', 'http://127.0.0.1:bad']:
            with self.subTest(endpoint=endpoint), self.assertRaises(ValueError):
                smoke.local_endpoint(endpoint)
        self.assertEqual(smoke.local_endpoint('http://192.168.10.2:8095/'), 'http://192.168.10.2:8095')
        self.assertEqual(smoke.local_endpoint('http://[::1]:8095/'), 'http://[::1]:8095')

    def test_explicit_owner_replay_stays_mock_and_needs_no_approval(self):
        with patch.object(smoke, 'run_owner', return_value=True) as owner:
            code, manifest = self.invoke(['--owner-replay'] + self.owner_flags())
        self.assertEqual(code, 0)
        self.assertFalse(owner.call_args.args[0].live)
        self.assertFalse(manifest['authorization']['record_valid'])

    def test_provider_redirects_are_denied_without_networking(self):
        with self.assertRaisesRegex(ValueError, 'redirects are disabled'):
            smoke.NoProviderRedirect().redirect_request(None, None, 302, '', {}, 'https://api.openai.com')

    def test_profile_loading_is_explicit_and_keeps_keys_out_of_omp_configuration(self):
        self.options.model_profile = self.base / 'profile.json'
        self.options.model_profile.write_text(json.dumps({'providers': {self.options.provider: {
            'api':'openai-completions', 'apiKey':'unit-test-not-a-real-key',
            'models':[{'id':self.options.model}]}}}))
        provider, key = smoke.model_profile(self.options)
        self.assertNotIn('apiKey', provider)
        self.assertEqual(key, 'unit-test-not-a-real-key')
        self.assertEqual(provider['models'], [{'id': self.options.model}])

    def test_historical_live_probe_entrypoints_are_retired_before_any_profile_read(self):
        for name in ['benchmark/cw104b/long_prefix_probe.py', '.audit/cw104b/cache_probe.py',
                     '.audit/cw104b/shared_prefix_probe.py']:
            with self.subTest(name=name), patch.object(Path, 'read_text', side_effect=AssertionError('No private reads')):
                with self.assertRaisesRegex(SystemExit, 'Historical probe retired'):
                    runpy.run_path(str(smoke.ROOT / name), run_name='__main__')
        self.network.assert_not_called()


if __name__ == '__main__':
    unittest.main()
