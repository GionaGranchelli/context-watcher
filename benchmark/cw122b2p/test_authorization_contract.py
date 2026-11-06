import json
import hashlib
from pathlib import Path
import tempfile
import unittest
from datetime import datetime, timedelta, timezone
import stat

from authorization import CAMPAIGN_REQUEST_GUARD, FIELDS, claim_once, validate

ROOT = Path(__file__).resolve().parent


class AuthorizationContractTests(unittest.TestCase):
    def test_limits_and_non_callable_state_are_explicit(self):
        contract = json.loads((ROOT / 'authorization-contract.json').read_text())
        self.assertEqual(contract['immutable_limits'], {
            'exact_run_count': 12, 'max_requests': 360, 'max_seconds': 7200,
            'max_requests_per_attempt': 30, 'max_seconds_per_attempt': 600,
        })
        self.assertFalse(contract['live_runner_callable'])
        self.assertFalse(contract['authorization_issued'])
        self.assertEqual(contract['campaign_request_guard'], CAMPAIGN_REQUEST_GUARD)
        self.assertIn('request 361 must fail before provider dispatch', contract['request_limit_semantics'])

    def test_exact_hash_bound_contract_and_overrun_acknowledgement(self):
        now = datetime.now(timezone.utc)
        with tempfile.TemporaryDirectory(dir=Path.home()) as temp:
            root = Path(temp)
            evidence = root / 'evidence'
            evidence.mkdir(mode=0o700)
            evidence.chmod(0o700)
            claims = root / 'claims'
            claims.mkdir(mode=0o700)
            claims.chmod(0o700)
            credential = root / 'credential'
            credential.write_text('offline-placeholder')
            credential.chmod(0o600)
            record = {
                'authorization_id': 'review-1', 'approver_identity': 'operator', 'approval_reference': 'change-control-1',
                'issued_at': (now - timedelta(seconds=1)).isoformat(), 'expires_at': (now + timedelta(minutes=5)).isoformat(),
                'provider': 'OpenAI API', 'model': 'gpt-6-luna', 'endpoint': 'https://api.openai.com/v1/responses',
                'campaign_sha256': 'a' * 64, 'runner_sha256': 'b' * 64, 'codex_executable_sha256': 'c' * 64,
                'profile_sha256': 'd' * 64, 'pricing_sha256': 'e' * 64,
                'exact_run_count': 12, 'max_requests': 360, 'max_requests_per_attempt': 30,
                'max_seconds': 7200, 'max_seconds_per_attempt': 600,
                'campaign_request_guard': CAMPAIGN_REQUEST_GUARD, 'spend_ceiling_usd': 0.1,
                'reservation_overrun_risk_accepted': True,
                'credential_file': str(credential),
                'credential_sha256': hashlib.sha256(b'offline-placeholder').hexdigest(),
                'private_evidence_directory': str(evidence),
                'authorization_claim_directory': str(claims),
            }
            selection = {key: record[key] for key in ('provider', 'model', 'endpoint', 'campaign_sha256', 'runner_sha256',
                                                        'codex_executable_sha256', 'profile_sha256', 'pricing_sha256',
                                                        'credential_file', 'credential_sha256',
                                                        'private_evidence_directory',
                                                        'authorization_claim_directory')}
            self.assertEqual(set(record), FIELDS)
            self.assertTrue(validate(record, selection, now))
            with self.assertRaisesRegex(ValueError, 'authorization fields missing or unknown'):
                validate(None, selection, now)
            expired = dict(record, expires_at=(now - timedelta(seconds=1)).isoformat())
            with self.assertRaisesRegex(ValueError, 'authorization expired'):
                validate(expired, selection, now)
            claim_once(record)
            claimed = list(claims.iterdir())
            self.assertEqual(len(claimed), 1)
            self.assertEqual(stat.S_IMODE(claimed[0].stat().st_mode), 0o600)
            with self.assertRaises(FileExistsError):
                claim_once(record)
            for key, value in [('runner_sha256', 'f' * 64), ('max_requests', 361),
                               ('max_seconds_per_attempt', 601), ('reservation_overrun_risk_accepted', False)]:
                changed = dict(record, **{key: value})
                with self.subTest(key=key), self.assertRaises(ValueError):
                    validate(changed, selection, now)
            changed_guard = dict(record, campaign_request_guard={**CAMPAIGN_REQUEST_GUARD, 'limit': 359})
            with self.assertRaises(ValueError):
                validate(changed_guard, selection, now)
            credential.write_text('replacement-key')
            with self.assertRaisesRegex(ValueError, 'credential file hash mismatch'):
                validate(record, selection, now)
            credential.write_text('offline-placeholder')
            credential_link = root / 'credential-link'
            credential_link.symlink_to(credential)
            linked_record = dict(record, credential_file=str(credential_link))
            linked_selection = dict(selection, credential_file=str(credential_link))
            with self.assertRaisesRegex(ValueError, 'no symlink components'):
                validate(linked_record, linked_selection, now)
            claim_link = root / 'claims-link'
            claim_link.symlink_to(claims, target_is_directory=True)
            linked_claim = dict(record, authorization_claim_directory=str(claim_link))
            linked_claim_selection = dict(selection, authorization_claim_directory=str(claim_link))
            with self.assertRaisesRegex(ValueError, 'no symlink components'):
                validate(linked_claim, linked_claim_selection, now)
            with self.assertRaises(ValueError):
                validate(dict(record, unexpected=True), selection, now)
            with tempfile.NamedTemporaryFile(dir=ROOT.parent.parent) as inside_checkout:
                Path(inside_checkout.name).chmod(0o600)
                with self.assertRaises(ValueError):
                    validate(dict(record, credential_file=inside_checkout.name), selection, now)


if __name__ == '__main__':
    unittest.main()
