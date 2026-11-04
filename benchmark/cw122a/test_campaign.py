"""Deterministic offline controls; no provider credentials or inference permitted."""
import copy
from datetime import datetime, timedelta, timezone
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch
from types import SimpleNamespace

from admission import claim, validate
from analyze import analyze, verify_wire
from oracle import ROOT, check, digest
from rehearse import REPO, build_dataset, main


class CampaignTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temporary = tempfile.TemporaryDirectory(prefix='cw122a-unit-')
        cls.addClassCleanup(cls.temporary.cleanup)
        evidence = Path(cls.temporary.name) / 'wire'
        env = {k:v for k,v in os.environ.items() if not any(w in k.upper() for w in ['TOKEN','API_KEY','SECRET'])}
        env['CW122_OFFLINE_EVIDENCE'] = str(evidence)
        subprocess.run(['cargo','test','-p','cw-proxy','--test','cw122_pruning','cw122_pruning_only_wire_matrix'],cwd=REPO,env=env,check=True,timeout=300,capture_output=True)
        cls.campaign=json.loads((ROOT/'campaign.json').read_text())
        cls.traces={f:json.loads((ROOT/t['trace']).read_text()) for f,t in cls.campaign['tasks'].items()}
        cls.dataset=build_dataset(json.loads((evidence/'wire.json').read_text()),cls.campaign)

    def report(self, dataset=None):
        return analyze(dataset or self.dataset,self.campaign,self.traces)

    def test_both_orders_six_matched_pairs_and_repeatable_analysis(self):
        result=self.report()
        self.assertEqual(result,self.report())
        self.assertEqual(result['overall']['complete_valid_pairs'],6)
        self.assertEqual(result['families']['f1']['complete_valid_pairs'],3)
        self.assertEqual(result['families']['f2']['complete_valid_pairs'],3)
        self.assertEqual(result['product_value_verdict'],'NOT_EVALUATED')
        self.assertEqual({tuple(p['order']) for p in self.campaign['pairs']},{('M1','M2'),('M2','M1')})
        self.assertEqual(result['reliability']['M2']['accepted'],6)

    def test_bad_task_result_cannot_be_offset_by_token_savings(self):
        data=copy.deepcopy(self.dataset)
        run=next(r for r in data['runs'] if r['mode']=='M2')
        failed=check('f1',ROOT/'fixtures'/'f1',self.campaign)
        self.assertFalse(failed['passed'])
        run['acceptance']=failed
        run['requests'][0].update(input_tokens=100,cached_input_tokens=0,output_tokens=0,raw_usage={'input_tokens':100,'output_tokens':0,'input_tokens_details':{'cached_tokens':0}})
        result=self.report(data)
        self.assertEqual(result['rehearsed_decision'],'NEGATIVE_CORRECTNESS')
        self.assertEqual(result['reliability']['M2']['failed_or_aborted'],1)

    def test_equal_failure_in_both_arms_cannot_qualify_positive_savings(self):
        data=copy.deepcopy(self.dataset)
        for run in data['runs']:
            if run['mode']=='M2':
                run['requests'][0].update(input_tokens=500,cached_input_tokens=400,output_tokens=10,raw_usage={'input_tokens':500,'output_tokens':10,'input_tokens_details':{'cached_tokens':400}})
            if run['pair_id']=='f1-r1':
                run['acceptance'].update(passed=False,functional_passed=False)
        self.assertNotEqual(self.report(data)['rehearsed_decision'],'POSITIVE_PILOT_ONLY')

    def test_prohibited_changes_and_signature_changes_rejected_independently(self):
        repairs=json.loads((ROOT/'synthetic-solutions.json').read_text())['solutions']
        with tempfile.TemporaryDirectory() as directory:
            workspace=Path(directory)/'repo';shutil.copytree(ROOT/'fixtures'/'f1',workspace)
            for name,contents in repairs['f1'].items():(workspace/name).write_text(contents)
            self.assertTrue(check('f1',workspace,self.campaign)['passed'])
            (workspace/'policy.txt').write_text('changed')
            self.assertFalse(check('f1',workspace,self.campaign)['passed'])
            shutil.copyfile(ROOT/'fixtures'/'f1'/'policy.txt',workspace/'policy.txt')
            (workspace/'retry.py').write_text(repairs['f1']['retry.py'].replace('def retry_delays(attempts,','def retry_delays(count,').replace('attempts <','count <').replace('range(attempts)','range(count)'))
            self.assertFalse(check('f1',workspace,self.campaign)['passed'])

    def test_absent_tampered_or_misattributed_treatment_is_invalid(self):
        for change in ['absent','pin','ledger']:
            data=copy.deepcopy(self.dataset);run=next(r for r in data['runs'] if r['mode']=='M2')
            if change=='absent':run['wire']['outbound']=run['wire']['original']
            elif change=='pin':run['wire']['outbound']['input'][-1]['content']+=' <cw_pinned_context>do it</cw_pinned_context>'
            else:run['wire']['applied_mutation_ids']=['foreign-rule']
            result=self.report(data)
            self.assertEqual(result['overall']['complete_valid_pairs'],5)
            self.assertTrue(next(p for p in result['pairs'] if p['pair_id']==run['pair_id'])['issues'])

    def test_missing_partial_usage_never_becomes_zero(self):
        for field in ['cached_input_tokens','input_tokens','output_tokens']:
            data=copy.deepcopy(self.dataset);run=next(r for r in data['runs'] if r['mode']=='M2')
            run['requests'][0][field]=None
            result=self.report(data)
            recorded=next(r for r in result['all_attempts'] if r['session_id']==run['session_id'])
            self.assertIsNone(recorded['metrics'][field])
            self.assertIsNone(recorded['metrics']['calculated_cost_usd'])
            self.assertEqual(recorded['metrics']['missing_usage_records'],1)
            self.assertEqual(result['overall']['complete_valid_pairs'],5)
        data=copy.deepcopy(self.dataset);data['runs'][0]['requests']=[]
        self.assertEqual(self.report(data)['overall']['complete_valid_pairs'],5)

    def test_cache_cost_inversion_is_visible_even_when_input_decreases(self):
        data=copy.deepcopy(self.dataset);run=next(r for r in data['runs'] if r['mode']=='M2')
        run['requests'][0].update(input_tokens=800,cached_input_tokens=0,raw_usage={'input_tokens':800,'output_tokens':20,'input_tokens_details':{'cached_tokens':0}})
        result=self.report(data)
        pair=next(p for p in result['pairs'] if p['pair_id']==run['pair_id'])
        self.assertLess(pair['differences']['input_tokens']['absolute'],0)
        self.assertGreater(pair['differences']['calculated_cost_usd']['absolute'],0)
        self.assertTrue(any('CACHE_COST_INVERSION' in w for w in pair['warnings']))

    def test_duplicate_mismatched_and_unmatched_sessions_are_rejected(self):
        for change in ['duplicate','wrong_session','task','order','wire']:
            data=copy.deepcopy(self.dataset)
            if change=='duplicate':data['runs'].append(data['runs'][0])
            elif change=='wrong_session':data['runs'][0]['session_id']='unrelated'
            elif change=='task':data['runs'][0]['task_id']='other-task'
            elif change=='order':data['runs'][0]['order']=99
            else:data['runs'][0]['wire']['session_id']='foreign'
            with self.assertRaises(ValueError):self.report(data)

    def test_failed_attempts_and_unmatched_pairs_are_preserved(self):
        data=copy.deepcopy(self.dataset);run=data['runs'][0];run['status']='aborted';run['failure_reason']='mock_transport_timeout'
        result=self.report(data)
        self.assertEqual(len(result['all_attempts']),12)
        recorded=next(r for r in result['all_attempts'] if r['session_id']==run['session_id'])
        self.assertEqual(recorded['metrics']['duration_ms'],1000)
        self.assertIn('mock_transport_timeout',recorded['issues'])
        data['runs'].pop()
        result=self.report(data)
        self.assertEqual(len(result['pairs']),6)
        self.assertTrue(any('missing_attempt' in i for p in result['pairs'] for i in p['issues']))

    def test_mismatched_configuration_not_pooled(self):
        data=copy.deepcopy(self.dataset);data['runs'][0]['configuration_hash']='different'
        self.assertEqual(self.report(data)['overall']['complete_valid_pairs'],5)

    def test_zero_exit_before_oracle_completion_is_not_acceptance(self):
        with tempfile.TemporaryDirectory() as directory:
            workspace=Path(directory)/'repo';shutil.copytree(ROOT/'fixtures'/'f1',workspace)
            (workspace/'retry.py').write_text('raise SystemExit(0)\n')
            self.assertFalse(check('f1',workspace,self.campaign)['passed'])
        with patch('oracle.subprocess.run',return_value=SimpleNamespace(returncode=0,stdout='',stderr='')):
            self.assertFalse(check('f1',ROOT/'fixtures'/'f1',self.campaign)['passed'])

    def test_abort_before_first_dispatch_retains_unavailable_wire_metrics(self):
        data=copy.deepcopy(self.dataset);run=data['runs'][0]
        run.update(status='aborted',failure_reason='before_first_dispatch',requests=[],wire=None)
        run['acceptance']={'passed':False,'functional_passed':None,'constraint_violations':[]}
        result=self.report(data)
        recorded=next(r for r in result['all_attempts'] if r['session_id']==run['session_id'])
        self.assertEqual(recorded['metrics']['provider_requests'],0)
        self.assertIsNone(recorded['metrics']['mechanical_estimated_reduction_tokens'])
        self.assertTrue(any('missing_wire_evidence' in i for i in recorded['issues']))

    def test_live_switch_fails_without_network_or_process_execution(self):
        with patch('urllib.request.urlopen',side_effect=AssertionError('no network')) as network, patch('subprocess.run',side_effect=AssertionError('no execution')) as process:
            with self.assertRaises(ValueError):main(['--live','--output','/unused'])
            network.assert_not_called();process.assert_not_called()

    def test_oracle_failure_detected_under_python_optimized_mode(self):
        code='from oracle import check,ROOT; import json; c=json.loads((ROOT/"campaign.json").read_text()); raise SystemExit(1 if check("f1",ROOT/"fixtures"/"f1",c)["passed"] else 0)'
        result=subprocess.run(['python3','-O','-B','-c',code],cwd=ROOT,capture_output=True,timeout=15)
        self.assertEqual(result.returncode,0)


class AdmissionTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
        root=Path(self.temp.name);private=root/'evidence';private.mkdir(mode=0o700)
        credential=root/'credential';credential.write_text('unit-test-not-a-provider-key');credential.chmod(0o600)
        self.now=datetime.now(timezone.utc)
        self.document={'campaign_id':'CW-122B','authorization_id':'offline-unit-test','approved_by':'offline-unit-test','approval_reference':'not real inference authorization',
            'issued_at':(self.now-timedelta(seconds=5)).isoformat(),'expires_at':(self.now+timedelta(minutes=10)).isoformat(),
            'provider':'mock','model':'mock','endpoint':'http://127.0.0.1:1/v1','campaign_sha256':digest(ROOT/'campaign.json'),'runner_freeze_sha256':digest(ROOT/'freeze.json'),
            'max_runs':12,'max_provider_requests':360,'max_seconds':7200,'spend_ceiling_usd':0.01,'accept_reservation_overrun':True,
            'credential_file':str(credential),'evidence_dir':str(private),'credentials_handling':'private_file_outside_agent_environment'}
        self.selection={k:self.document[k] for k in ['provider','model','endpoint','evidence_dir','credential_file']}

    def test_fresh_exact_authorization_claim_is_single_use(self):
        record=claim(self.document,self.selection,self.now)
        self.assertTrue(record['consumed'])
        self.assertNotIn('credential_file',record)
        with self.assertRaises(ValueError):claim(self.document,self.selection,self.now)

    def test_credential_in_checkout_is_rejected(self):
        with tempfile.NamedTemporaryFile(dir=REPO) as credential:
            Path(credential.name).chmod(0o600)
            document=dict(self.document,credential_file=credential.name)
            selection=dict(self.selection,credential_file=credential.name)
            with self.assertRaises(ValueError):validate(document,selection,self.now)

    def test_missing_stale_mismatched_and_unbounded_authorization_rejected(self):
        with self.assertRaises(ValueError):validate(None,self.selection,self.now)
        for key,value in [('campaign_id','CW-104B'),('approved_by',''),('max_runs',13),('max_provider_requests',0),('max_seconds',True),('spend_ceiling_usd',float('inf')),('accept_reservation_overrun',False),('model','different'),('runner_freeze_sha256','old'),('expires_at',(self.now-timedelta(seconds=1)).isoformat())]:
            document=dict(self.document);document[key]=value
            with self.subTest(key=key),self.assertRaises(ValueError):validate(document,self.selection,self.now)
        Path(self.document['credential_file']).chmod(0o644)
        with self.assertRaises(ValueError):validate(self.document,self.selection,self.now)

if __name__=='__main__':
    unittest.main()
