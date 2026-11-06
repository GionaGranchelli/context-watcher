import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import stat
import sys
import tempfile
import unittest
import urllib.error
import urllib.request
from datetime import datetime, timedelta, timezone
from unittest.mock import MagicMock, patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
from smoke_authorization import (
    SMOKE_REQUEST_GUARD, _NoRedirect, approval_statement, claim_once,
    fetch_github_comment, validate_and_claim, validate_smoke, verify_approval,
)
import run_smoke


class SmokeAuthorizationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(dir=Path.home())
        self.root = Path(self.temp.name)
        self.root.chmod(0o700)
        self.evidence = self.root / 'evidence'
        self.evidence.mkdir(mode=0o700)
        self.claims = self.root / 'claims'
        self.claims.mkdir(mode=0o700)
        self.credential = self.root / 'credential'
        self.credential.write_bytes(b'offline-only-placeholder')
        self.credential.chmod(0o600)
        self.codex = Path(shutil.which('python3')).resolve()
        self.now = datetime.now(timezone.utc)
        self.record = {
            'experiment': 'CW-122B2-L1-SMOKE', 'authorization_id': 'review-1',
            'approval_reference': '123456789',
            'issued_at': (self.now - timedelta(seconds=1)).isoformat(),
            'expires_at': (self.now + timedelta(minutes=10)).isoformat(),
            'provider': 'OpenAI API', 'model': 'gpt-6-luna',
            'endpoint': 'https://api.openai.com/v1', 'processing_tier': 'standard',
            'campaign_sha256': 'a' * 64, 'runner_sha256': 'b' * 64,
            'codex_executable_path': str(self.codex),
            'codex_executable_sha256': self.digest(self.codex),
            'profile_sha256': 'd' * 64, 'pricing_sha256': 'e' * 64,
            'pair_id': 'f1-r1', 'attempt_order': ['M1', 'M2'],
            'exact_attempt_count': 2, 'max_requests': 60,
            'max_requests_per_attempt': 30, 'max_seconds': 1200,
            'max_seconds_per_attempt': 600,
            'campaign_request_guard': SMOKE_REQUEST_GUARD,
            'spend_ceiling_usd': 5.0, 'request_reservation_usd': 5.0 / 60,
            'reservation_overrun_risk_accepted': True,
            'credential_file': str(self.credential),
            'credential_sha256': hashlib.sha256(self.credential.read_bytes()).hexdigest(),
            'private_evidence_directory': str(self.evidence),
        }
        self.selection = {key: self.record[key] for key in (
            'provider', 'model', 'endpoint', 'campaign_sha256', 'runner_sha256',
            'codex_executable_path', 'codex_executable_sha256', 'profile_sha256',
            'pricing_sha256', 'pair_id', 'credential_file', 'credential_sha256',
            'private_evidence_directory')}

    @staticmethod
    def digest(path):
        return hashlib.sha256(path.read_bytes()).hexdigest()

    def tearDown(self):
        self.temp.cleanup()

    def test_authorized_runner_hash_includes_live_code_and_acceptance_inputs(self):
        required = {
            'benchmark/cw122b1/run_mock_campaign.py',
            'benchmark/cw122a/offline_env.py',
            'benchmark/cw122a/rehearse.py',
            'benchmark/cw122a/analyze.py',
            'benchmark/cw122a/oracle.py',
            'benchmark/cw122a/retry.py',
            'benchmark/cw122a/oracles/f1.py',
            'benchmark/cw122a/fixtures/f1',
            'crates/cw-core/src',
            'crates/cw-store/src',
            'crates/cw-proxy/src',
            'crates/cw-harnesses/src',
        }
        self.assertTrue(required <= set(run_smoke.BUNDLE))
        self.assertIn('benchmark/cw122b2l1/smoke_authorization.py', run_smoke.BUNDLE)
        self.assertNotIn('benchmark/cw122b2p/authorization.py', run_smoke.BUNDLE)

    def test_runner_hash_changes_when_nested_dependency_changes(self):
        source = self.root / 'source'
        source.mkdir()
        nested = source / 'nested.rs'
        nested.write_text('first')
        with patch.object(run_smoke, 'REPO', self.root), patch.object(run_smoke, 'BUNDLE', ('source',)):
            first = run_smoke.runner_sha256()
            nested.write_text('second')
            self.assertNotEqual(first, run_smoke.runner_sha256())

    def test_runner_hash_rejects_nested_symlink_directories(self):
        source = self.root / 'source'
        source.mkdir()
        target = self.root / 'target'
        target.mkdir()
        (source / 'linked').symlink_to(target, target_is_directory=True)
        with patch.object(run_smoke, 'REPO', self.root), patch.object(run_smoke, 'BUNDLE', ('source',)):
            with self.assertRaisesRegex(ValueError, 'runner bundle file is a symlink'):
                run_smoke.runner_sha256()

    def test_runner_defaults_to_no_go_even_when_api_key_is_in_parent_environment(self):
        runner = Path(__file__).with_name('run_smoke.py')
        environment = dict(os.environ, OPENAI_API_KEY='test-only-never-use')
        result = subprocess.run([sys.executable, str(runner)], env=environment,
                                capture_output=True, text=True, check=False)
        self.assertEqual(result.returncode, 2)
        self.assertIn('NO_GO', result.stdout)
        self.assertNotIn('test-only-never-use', result.stdout + result.stderr)

    def approval_comment(self, record=None, **changes):
        record = record or self.record
        created = record['issued_at']
        comment = {
            'id': int(record['approval_reference']),
            'url': f'https://api.github.com/repos/GionaGranchelli/context-watcher/issues/comments/{record["approval_reference"]}',
            'issue_url': 'https://api.github.com/repos/GionaGranchelli/context-watcher/issues/46',
            'body': approval_statement(record),
            'user': {'login': 'GionaGranchelli'},
            'author_association': 'OWNER',
            'created_at': created,
            'updated_at': created,
        }
        return {**comment, **changes}

    def test_approval_digest_binds_envelope_but_not_comment_id(self):
        self.assertEqual(approval_statement(self.record),
                         approval_statement(dict(self.record, approval_reference='987654321')))
        self.assertNotEqual(approval_statement(self.record),
                            approval_statement(dict(self.record, spend_ceiling_usd=4.0)))

    def test_github_comment_fetch_is_fixed_and_rejects_redirects(self):
        url = 'https://api.github.com/repos/GionaGranchelli/context-watcher/issues/comments/123456789'
        response = MagicMock(status=200)
        response.geturl.return_value = url
        response.read.return_value = json.dumps(self.approval_comment()).encode()
        opener = MagicMock()
        opener.open.return_value.__enter__.return_value = response
        with patch('urllib.request.build_opener', return_value=opener) as build_opener:
            fetch_github_comment('123456789')
        request = opener.open.call_args.args[0]
        self.assertEqual(request.full_url, url)
        self.assertEqual(opener.open.call_args.kwargs['timeout'], 5)
        self.assertTrue(any(isinstance(handler, _NoRedirect) for handler in build_opener.call_args.args))
        self.assertIsNone(_NoRedirect().redirect_request(request, None, 302, 'Found', {}, 'https://example.com/'))
        opener.open.side_effect = urllib.error.HTTPError(url, 302, 'Found', {}, None)
        with patch('urllib.request.build_opener', return_value=opener):
            with self.assertRaisesRegex(ValueError, 'could not be verified') as caught:
                fetch_github_comment('123456789')
            caught.exception.__cause__.close()

    def test_github_owner_approval_is_verified_and_edits_fail_closed(self):
        with patch('smoke_authorization.fetch_github_comment', return_value=self.approval_comment()):
            verify_approval(self.record, self.now)
        for changes, message in (
            ({'id': 7}, 'reference mismatch'),
            ({'issue_url': 'https://api.github.com/repos/GionaGranchelli/context-watcher/issues/45'}, 'reference mismatch'),
            ({'user': {'login': 'someone-else'}}, 'authorized repository owner'),
            ({'body': 'looks good'}, 'missing, mismatched, or edited'),
            ({'updated_at': (self.now + timedelta(seconds=1)).isoformat()}, 'missing, mismatched, or edited'),
        ):
            with patch('smoke_authorization.fetch_github_comment',
                       return_value=self.approval_comment(**changes)), self.subTest(changes=changes):
                with self.assertRaisesRegex(ValueError, message):
                    verify_approval(self.record, self.now)

    def test_invalid_github_approval_does_not_consume_authorization(self):
        with patch('smoke_authorization.claim_registry_root', return_value=self.claims), \
                patch('smoke_authorization.fetch_github_comment',
                      return_value=self.approval_comment(body='not an approval')):
            with self.assertRaisesRegex(ValueError, 'approval statement'):
                validate_and_claim(self.record, self.selection, self.now)
        self.assertEqual(list(self.claims.iterdir()), [])

    def test_global_claim_registry_rejects_replay_with_changed_evidence_path(self):
        other_evidence = self.root / 'other-evidence'
        other_evidence.mkdir(mode=0o700)
        changed_copy = dict(self.record, private_evidence_directory=str(other_evidence))
        with patch('smoke_authorization.claim_registry_root', return_value=self.claims):
            claim_once(self.record)
            with self.assertRaisesRegex(ValueError, 'already consumed'):
                claim_once(changed_copy)

    def test_valid_envelope_is_single_use(self):
        self.assertTrue(validate_smoke(self.record, self.selection, self.now))
        with patch('smoke_authorization.claim_registry_root', return_value=self.claims), \
                patch('smoke_authorization.fetch_github_comment', return_value=self.approval_comment()):
            self.assertTrue(validate_and_claim(self.record, self.selection, self.now))
            with self.assertRaisesRegex(ValueError, 'already consumed'):
                validate_and_claim(self.record, self.selection, self.now)

    def test_absent_expired_and_mismatched_authorizations_fail_closed(self):
        with self.assertRaisesRegex(ValueError, 'fields missing or unknown'):
            validate_smoke(None, self.selection, self.now)
        expired = dict(self.record, expires_at=(self.now - timedelta(seconds=1)).isoformat())
        with self.assertRaisesRegex(ValueError, 'expired'):
            validate_smoke(expired, self.selection, self.now)
        for key, value in [('model', 'other-model'), ('processing_tier', 'priority'),
                           ('endpoint', 'https://redirect.example/v1'),
                           ('max_requests', 61), ('exact_attempt_count', 12),
                           ('reservation_overrun_risk_accepted', False),
                           ('authorization_claim_directory', str(self.claims))]:
            altered = dict(self.record, **{key: value})
            with self.subTest(key=key), self.assertRaises(ValueError):
                validate_smoke(altered, self.selection, self.now)

    def test_credential_replacement_and_symlink_fail_closed(self):
        self.credential.write_bytes(b'replaced-secret')
        with self.assertRaisesRegex(ValueError, 'credential file hash mismatch'):
            validate_smoke(self.record, self.selection, self.now)
        self.credential.write_bytes(b'offline-only-placeholder')
        link = self.root / 'credential-link'
        link.symlink_to(self.credential)
        altered = dict(self.record, credential_file=str(link))
        selection = dict(self.selection, credential_file=str(link))
        with self.assertRaisesRegex(ValueError, 'no symlink components'):
            validate_smoke(altered, selection, self.now)

    def test_missing_telemetry_and_unbounded_budget_are_not_accepted(self):
        for key in ('campaign_sha256', 'runner_sha256', 'pricing_sha256'):
            altered = dict(self.record, **{key: ''})
            with self.subTest(key=key), self.assertRaises(ValueError):
                validate_smoke(altered, self.selection, self.now)
        altered = dict(self.record, spend_ceiling_usd=5.01)
        with self.assertRaisesRegex(ValueError, 'at most 5 USD'):
            validate_smoke(altered, self.selection, self.now)


if __name__ == '__main__':
    unittest.main()
