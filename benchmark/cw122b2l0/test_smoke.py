import copy
from datetime import datetime, timedelta, timezone
import hashlib
import json
import multiprocessing
import os
from pathlib import Path
import sqlite3
import sys
import tempfile
import threading
import textwrap
import unittest
from unittest.mock import patch
from contextlib import redirect_stdout
from io import StringIO
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import smoke


def claim_worker(record, root, queue):
    try:
        smoke.claim_once(record, Path(root))
        queue.put('claimed')
    except Exception as exc:
        queue.put(type(exc).__name__)


class SmokeAuthorizationTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.claim_tmp = tempfile.TemporaryDirectory(dir=Path.home())
        base = Path(self.tmp.name)
        self.keydir = base / 'keydir'; self.keydir.mkdir(mode=0o700)
        self.cred = self.keydir / 'key'
        self.cred.write_text('mock-key')
        self.cred.chmod(0o600)
        self.evidence = base / 'evidence'
        self.evidence.mkdir(mode=0o700)
        self.claims = Path(self.claim_tmp.name) / 'claims'
        self.claims.mkdir(mode=0o700)
        self.codex = base / 'codex'; self.codex.write_bytes(b'mock codex executable'); self.codex.chmod(0o700)
        self.cw = base / 'cw'; self.cw.write_bytes(b'mock cw executable'); self.cw.chmod(0o700)
        self.sandbox = base / 'sandbox'; self.sandbox.write_bytes(b'mock sandbox executable'); self.sandbox.chmod(0o700)
        now = datetime.now(timezone.utc)
        self.record = {
            'authorization_id':'mock-auth-1','approver_identity':'test operator','approval_reference':'offline fixture',
            'issued_at':now.isoformat(),'expires_at':(now+timedelta(minutes=5)).isoformat(),
            'provider':'openai','model':'gpt-6-luna','endpoint':'https://api.openai.com/v1','processing_tier':'standard',
            'runner_sha256':smoke.runner_sha256(), 'pricing_sha256':smoke.sha256(smoke.PRICING),
            'codex_executable_sha256':smoke.sha256(self.codex),'cw_executable_sha256':smoke.sha256(self.cw),'request_cap':1,'deadline_seconds':60,
            'spend_ceiling_usd':0.3,'reservation_overrun_risk_accepted':True,'credential_file':str(self.cred),
            'credential_sha256':smoke.sha256(self.cred),
            'private_evidence_directory':str(self.evidence),'isolation_profile':'bubblewrap-read-only-hidden-provider-credential-v1',
            'codex_executable_path':str(self.codex),'cw_executable_path':str(self.cw),
            'sandbox_executable_path':str(self.sandbox),'sandbox_executable_sha256':smoke.sha256(self.sandbox),
            'authorization_claim_directory':str(self.claims),
        }
        self.selection = {'runner_sha256':self.record['runner_sha256'],'codex_executable_sha256':self.record['codex_executable_sha256'],'cw_executable_sha256':self.record['cw_executable_sha256'],
                          'codex_executable_path':str(self.codex),'cw_executable_path':str(self.cw),
                          'sandbox_executable_path':str(self.sandbox),
                          'credential_file':str(self.cred),'private_evidence_directory':str(self.evidence),
                          'authorization_claim_directory':str(self.claims)}

    def tearDown(self):
        self.tmp.cleanup()
        self.claim_tmp.cleanup()

    def test_valid_record_and_hash_bound_pricing(self):
        self.assertTrue(smoke.validate(self.record, self.selection))
        changed = copy.deepcopy(self.record)
        changed['model'] = 'gpt-6-luna-preview'
        with self.assertRaises(ValueError):
            smoke.validate(changed, self.selection)
        changed = copy.deepcopy(self.record)
        changed['endpoint'] = 'https://example.invalid/v1'
        with self.assertRaises(ValueError):
            smoke.validate(changed, self.selection)
        changed = copy.deepcopy(self.record)
        changed['runner_sha256'] = '0' * 64
        with self.assertRaises(ValueError):
            smoke.validate(changed, self.selection)
        changed = copy.deepcopy(self.record)
        changed['isolation_profile'] = 'unsandboxed'
        with self.assertRaises(ValueError):
            smoke.validate(changed, self.selection)

    def test_authorization_id_must_fit_authenticated_session_contract(self):
        maximum = copy.deepcopy(self.record)
        maximum['authorization_id'] = 'a' * (128 - len(smoke.SMOKE_SESSION_PREFIX))
        self.assertTrue(smoke.validate(maximum, self.selection))
        for invalid in ('contains/slash', 'has space', 'a' * (129 - len(smoke.SMOKE_SESSION_PREFIX)), 'é'):
            with self.subTest(authorization_id=invalid):
                changed = copy.deepcopy(self.record)
                changed['authorization_id'] = invalid
                with self.assertRaisesRegex(ValueError, 'session ID contract'):
                    smoke.validate(changed, self.selection)

    def test_runner_bundle_digest_binds_imported_fault_controls(self):
        original = smoke.sha256
        baseline = smoke.runner_sha256()
        with patch.object(
            smoke,
            'sha256',
            side_effect=lambda path: 'f' * 64
            if smoke.Path(path).name == 'run_mock_campaign.py'
            else original(path),
        ):
            self.assertNotEqual(smoke.runner_sha256(), baseline)

    def test_disabled_contract_and_example_match_validator_fields(self):
        root = smoke.Path(smoke.__file__).parent
        contract = json.loads((root/'authorization-contract.json').read_bytes())
        example = json.loads((root/'authorization.example.json').read_bytes())
        self.assertFalse(contract['enabled'])
        self.assertFalse(contract['authorization_issued'])
        self.assertEqual(set(contract['required_fields']), smoke.FIELDS)
        self.assertEqual(set(example), smoke.FIELDS)
        self.assertEqual(contract['session_binding']['prefix'], smoke.SMOKE_SESSION_PREFIX)
        self.assertEqual(contract['session_binding']['maximum_total_characters'], 128)

    def test_authorization_rejects_replaced_provider_credential(self):
        self.assertTrue(smoke.validate(self.record, self.selection))
        self.cred.write_text('replacement-key')
        self.cred.chmod(0o600)
        with self.assertRaisesRegex(ValueError, 'credential contents'):
            smoke.validate(self.record, self.selection)

    def test_absent_expired_and_mismatched_authorizations_fail(self):
        with self.assertRaises(ValueError):
            smoke.validate(None, self.selection)
        expired = copy.deepcopy(self.record)
        expired['expires_at'] = (datetime.now(timezone.utc)-timedelta(seconds=1)).isoformat()
        with self.assertRaises(ValueError):
            smoke.validate(expired, self.selection)
        mismatch = copy.deepcopy(self.record)
        mismatch['pricing_sha256'] = 'd'*64
        with self.assertRaises(ValueError):
            smoke.validate(mismatch, self.selection)

    def test_single_use_claim_rejects_reuse_and_race(self):
        smoke.claim_once(self.record, self.claims)
        with self.assertRaises(FileExistsError):
            smoke.claim_once(self.record, self.claims)
        self.evidence.rmdir()
        self.evidence.mkdir(mode=0o700)
        with self.assertRaises(FileExistsError):
            smoke.claim_once(self.record, self.claims)
        fresh = copy.deepcopy(self.record)
        fresh['authorization_id'] = 'race-auth'
        q = multiprocessing.Queue()
        workers = [multiprocessing.Process(target=claim_worker, args=(fresh, str(self.claims), q)) for _ in range(2)]
        for p in workers: p.start()
        for p in workers: p.join(5)
        self.assertTrue(all(not p.is_alive() and p.exitcode == 0 for p in workers))
        self.assertCountEqual([q.get(timeout=1), q.get(timeout=1)], ['claimed','FileExistsError'])

    def test_symlink_credential_and_nonempty_evidence_rejected(self):
        link = self.cred.parent / 'key-link'
        link.symlink_to(self.cred)
        bad = copy.deepcopy(self.record)
        bad['credential_file'] = str(link)
        with self.assertRaises(ValueError):
            smoke.validate(bad, self.selection | {'credential_file':str(link)})
        (self.evidence/'unexpected').write_text('x')
        with self.assertRaises(ValueError):
            smoke.validate(self.record, self.selection)

    def test_symlinked_parent_paths_rejected(self):
        alias = self.cred.parent / 'alias'
        alias.symlink_to(self.cred.parent, target_is_directory=True)
        bad = copy.deepcopy(self.record)
        bad['credential_file'] = str(alias / self.cred.name)
        with self.assertRaises(ValueError):
            smoke.validate(bad, self.selection | {'credential_file':bad['credential_file']})
        evidence_alias = self.evidence.parent / 'evidence-alias'
        evidence_alias.symlink_to(self.evidence, target_is_directory=True)
        bad = copy.deepcopy(self.record)
        bad['private_evidence_directory'] = str(evidence_alias)
        with self.assertRaises(ValueError):
            smoke.validate(bad, self.selection | {'private_evidence_directory':bad['private_evidence_directory']})
        bad = copy.deepcopy(self.record)
        bad['authorization_claim_directory'] = str(evidence_alias)
        with self.assertRaises(ValueError):
            smoke.validate(bad, self.selection | {'authorization_claim_directory':bad['authorization_claim_directory']})

    def test_private_raw_exchange_captures_exact_bytes_and_rejects_redirects(self):
        state = {'redirect':False,'request':None}
        class Handler(BaseHTTPRequestHandler):
            def do_POST(self):
                state['request'] = self.rfile.read(int(self.headers['Content-Length']))
                if state['redirect']:
                    self.send_response(307); self.send_header('Location','https://api.openai.com/v1'); self.end_headers(); return
                body = b'{"usage":{"input_tokens":1,"output_tokens":2}}'
                self.send_response(200); self.send_header('Content-Length',str(len(body))); self.end_headers(); self.wfile.write(body)
            def log_message(self, *_args): pass
        server = ThreadingHTTPServer(('127.0.0.1',0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True); thread.start()
        try:
            url = f'http://127.0.0.1:{server.server_port}/v1/responses'
            payload = {'model':'gpt-6-luna','input':'mock only'}
            status, body, hashes = smoke.mock_exchange(url,payload,self.evidence)
            self.assertEqual(status,200)
            self.assertEqual((self.evidence/'ingress.json').read_bytes(),state['request'])
            self.assertEqual((self.evidence/'response.bin').read_bytes(),body)
            self.assertEqual(hashes['ingress_sha256'],smoke.sha256(self.evidence/'ingress.json'))
            self.assertEqual(hashes['response_sha256'],smoke.sha256(self.evidence/'response.bin'))
            for file in self.evidence.iterdir(): self.assertEqual(file.stat().st_mode & 0o777,0o600)
            with self.assertRaises(ValueError):
                smoke.mock_exchange('https://api.openai.com/v1/responses',payload,self.evidence)
        finally:
            server.shutdown(); thread.join(2); server.server_close()

    def test_mock_exchange_does_not_follow_redirect(self):
        state = {'redirect':True}
        class Handler(BaseHTTPRequestHandler):
            def do_POST(self):
                self.rfile.read(int(self.headers['Content-Length']))
                self.send_response(307); self.send_header('Location','https://api.openai.com/v1'); self.end_headers()
            def log_message(self, *_args): pass
        server = ThreadingHTTPServer(('127.0.0.1',0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True); thread.start()
        try:
            with self.assertRaises(Exception):
                smoke.mock_exchange(f'http://127.0.0.1:{server.server_port}/',{'x':1},self.evidence)
        finally:
            server.shutdown(); thread.join(2); server.server_close()

    def test_cli_defaults_and_run_flag_are_no_go_before_any_launch(self):
        with self.assertRaises(SystemExit) as absent:
            smoke.main([])
        self.assertIn('NO_GO', str(absent.exception))
        with self.assertRaises(SystemExit) as live:
            smoke.main(['--run','--authorization','/does/not/matter'])
        self.assertIn('NO_GO', str(live.exception))
        with self.assertRaises(ValueError):
            smoke.main(['--mock-only'])

    def test_check_validates_private_auth_but_never_claims_or_launches(self):
        auth = self.cred.parent / 'authorization.json'
        auth.write_text(json.dumps(self.record))
        auth.chmod(0o600)
        stdout = StringIO()
        with patch.object(smoke, 'run_authorized_smoke') as run, redirect_stdout(stdout):
            result = smoke.main([
                '--check','--authorization',str(auth),
                '--codex-executable',str(self.codex),'--cw-executable',str(self.cw),
                '--sandbox-executable',str(self.sandbox),
            ])
        self.assertEqual(result, 0)
        self.assertEqual(json.loads(stdout.getvalue())['provider_calls'], 0)
        run.assert_not_called()
        self.assertEqual(list(self.claims.iterdir()), [])

    def test_codex_environment_drops_provider_secrets_and_sets_only_dummy(self):
        env = smoke.build_codex_environment({'PATH':'/bin','OPENAI_API_KEY':'real-secret',
            'CW_OPENAI_API_KEY_FILE':'/private/key','PASSWORD':'pw'}, control_token='local-control', session_id='smoke-1')
        self.assertEqual(env['CW_PROXY_DUMMY_API_KEY'], 'cw-local-proxy-dummy')
        self.assertNotIn('OPENAI_API_KEY', env)
        self.assertNotIn('CW_OPENAI_API_KEY_FILE', env)
        self.assertNotIn('PASSWORD', env)

    def test_mock_runner_refuses_evidence_write_failure_before_codex_lookup(self):
        with patch.object(smoke, 'write_raw_artifact', side_effect=OSError('disk full')) as write, \
             patch.object(smoke.shutil, 'which') as which:
            with self.assertRaises(OSError):
                smoke.run_mock_codex(self.evidence, control_token='t', session_id='s')
            write.assert_called_once()
            which.assert_not_called()

    def test_mock_runner_kills_and_records_timed_out_process(self):
        codex = self.cred.parent / 'mock-codex'
        codex.write_text('#!/bin/sh\necho mock-codex 1.0\n')
        codex.chmod(0o700)
        with patch.object(smoke.shutil, 'which', return_value=str(codex)):
            result = smoke.run_mock_codex(
                self.evidence, control_token='t', session_id='s', timeout_seconds=0.05,
                command=[sys.executable, '-c', 'import time; print("started", flush=True); time.sleep(30)'],
            )

        self.assertEqual(result['status'], 'TIMEOUT')
        self.assertNotEqual(result['exit_code'], 0)
        output = (self.evidence / 'mock-runner-output.bin').read_bytes()
        self.assertIn(b'started', output)
        self.assertEqual(result['output_sha256'], hashlib.sha256(output).hexdigest())
        manifest = json.loads((self.evidence / 'mock-run-manifest.json').read_bytes())
        self.assertEqual(manifest, result)
        for path in self.evidence.iterdir():
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)

    def test_authorized_runner_completes_against_local_mock_only(self):
        self.cw.write_text(textwrap.dedent('''\
            #!/usr/bin/python3
            import json, sqlite3, sys
            from http.server import BaseHTTPRequestHandler, HTTPServer
            args = sys.argv[1:]
            if 'wrap' in args:
                import os, subprocess
                port = args[args.index('--proxy-port') + 1]
                session = args[args.index('--session-id') + 1]
                command = args[args.index('--') + 1:]
                settings = [
                    'model_provider="context_watcher"',
                    'model_providers.context_watcher.name="contextWatcher"',
                    f'model_providers.context_watcher.base_url="http://127.0.0.1:{port}/openai/v1"',
                    'model_providers.context_watcher.wire_api="responses"',
                    'model_providers.context_watcher.requires_openai_auth=true',
                    'model_providers.context_watcher.env_key="CW_PROXY_DUMMY_API_KEY"',
                    'model_providers.context_watcher.env_http_headers={"x-cw-session-id"="CW_SESSION_ID","x-cw-control-token"="CW_CONTROL_TOKEN"}',
                    'shell_environment_policy.filters.CW_CONTROL_TOKEN="exclude"',
                    'shell_environment_policy.filters.CW_PROXY_DUMMY_API_KEY="exclude"',
                ]
                config_args = [arg for setting in settings for arg in ('--config',setting)]
                command[-1:-1] = config_args
                os.environ.update({'CW_SESSION_ID':session,'CW_CONTROL_TOKEN':'a'*32,
                                   'CW_PROXY_DUMMY_API_KEY':'cw-local-proxy-dummy'})
                raise SystemExit(subprocess.run(command).returncode)
            db = args[args.index('--db') + 1]
            if 'auth-token' in args:
                print('a' * 32)
                raise SystemExit(0)
            port = int(args[args.index('--proxy-port') + 1])
            conn = sqlite3.connect(db)
            conn.executescript("""
                CREATE TABLE turns(id TEXT PRIMARY KEY,turn_json TEXT NOT NULL,created_at TEXT NOT NULL);
                CREATE TABLE wire_requests(turn_id TEXT PRIMARY KEY,body BLOB NOT NULL);
                CREATE TABLE wire_egress(turn_id TEXT PRIMARY KEY,body BLOB NOT NULL);
                CREATE TABLE wire_responses(turn_id TEXT PRIMARY KEY,body BLOB NOT NULL,complete INTEGER NOT NULL,truncated INTEGER NOT NULL);
                CREATE TABLE spend_guard_ledger(turn_id TEXT PRIMARY KEY,session_id TEXT NOT NULL,provider TEXT NOT NULL,model TEXT NOT NULL,admission_status TEXT NOT NULL,reservation_usd REAL,settlement_status TEXT NOT NULL,settled_usd REAL,usage_json TEXT,error TEXT,updated_at TEXT NOT NULL);
            """)
            class Handler(BaseHTTPRequestHandler):
                def do_POST(self):
                    body = self.rfile.read(int(self.headers['Content-Length']))
                    request = json.loads(body)
                    usage = {'input_tokens':10,'output_tokens':2,'total_tokens':12,
                        'input_tokens_details':{'cached_tokens':2,'cache_write_tokens':2,
                        'cache_write_tokens_details':{'ttl_30m_tokens':1,'ttl_12h_tokens':1}}}
                    response = (b'data: {"type":"response.completed","response":{"usage":'
                        + json.dumps(usage,separators=(',',':')).encode() + b'}}\\n\\n')
                    turn = {'model':request['model'],'outcome':'completed','observed_usage':usage,
                            'applied_mutation_ids':[],'skipped_mutations':[]}
                    session = self.headers['x-cw-session-id']
                    conn.execute('INSERT INTO turns VALUES(?,?,?)',('turn-1',json.dumps(turn),'2026-11-05'))
                    conn.execute('INSERT INTO wire_requests VALUES(?,?)',('turn-1',body))
                    conn.execute('INSERT INTO wire_egress VALUES(?,?)',('turn-1',body))
                    conn.execute('INSERT INTO wire_responses VALUES(?,?,?,?)',('turn-1',response,1,0))
                    conn.execute('INSERT INTO spend_guard_ledger VALUES(?,?,?,?,?,?,?,?,?,?,?)',
                        ('turn-1',session,'OpenAI',request['model'],'ADMITTED',0.3,'SETTLED',0.000002,
                         json.dumps(usage),None,'2026-11-05'))
                    conn.commit()
                    self.send_response(200); self.send_header('Content-Length',str(len(response)))
                    self.send_header('Content-Type','text/event-stream'); self.end_headers(); self.wfile.write(response)
                def log_message(self, *_): pass
            HTTPServer(('127.0.0.1',port),Handler).serve_forever()
        '''))
        self.cw.chmod(0o700)
        self.codex.write_text(textwrap.dedent('''\
            #!/usr/bin/python3
            import json,os,sys,urllib.request
            if 'OPENAI_API_KEY' in os.environ or os.environ.get('CW_PROXY_DUMMY_API_KEY') != 'cw-local-proxy-dummy':
                raise SystemExit('Codex credential boundary failed')
            args=sys.argv[1:]
            setting=next(x for x in args if x.startswith('model_providers.context_watcher.base_url='))
            url=setting.split('=',1)[1].strip('"')+'/responses'
            model=args[args.index('--model')+1]
            req=urllib.request.Request(url,data=json.dumps({'model':model,'input':'local mock'}).encode(),
                headers={'Authorization':'Bearer cw-local-proxy-dummy',
                    'x-cw-session-id':__import__('os').environ['CW_SESSION_ID'],
                    'x-cw-control-token':__import__('os').environ['CW_CONTROL_TOKEN'],
                    'Content-Type':'application/json'},method='POST')
            with urllib.request.urlopen(req,timeout=5) as response: response.read()
            print('{"type":"agent_message","text":"CW-L0-SMOKE-OK"}')
        '''))
        self.codex.chmod(0o700)
        self.sandbox.write_text(textwrap.dedent('''\
            #!/usr/bin/python3
            import os,sys
            args=sys.argv[1:]; env=os.environ.copy(); cwd=None; i=0
            takes={'--ro-bind':2,'--dev':1,'--proc':1,'--tmpfs':1,'--bind':2,'--dir':1,'--chdir':1,'--setenv':2}
            while i < len(args):
                if args[i]=='--unshare-pid': i+=1; continue
                if args[i]=='--':
                    cmd=args[i+1:]; break
                option=args[i]; count=takes[option]; values=args[i+1:i+1+count]
                if option=='--setenv': env[values[0]]=values[1]
                if option=='--chdir': cwd=values[0]
                i+=1+count
            os.chdir(cwd or '/')
            os.execvpe(cmd[0],cmd,env)
        '''))
        self.sandbox.chmod(0o700)
        self.record['codex_executable_sha256'] = smoke.sha256(self.codex)
        self.record['cw_executable_sha256'] = smoke.sha256(self.cw)
        self.record['sandbox_executable_sha256'] = smoke.sha256(self.sandbox)
        self.selection.update({
            'codex_executable_sha256':self.record['codex_executable_sha256'],
            'cw_executable_sha256':self.record['cw_executable_sha256'],
            'sandbox_executable_path':str(self.sandbox),
        })
        self.record['codex_executable_path'] = str(self.codex)
        self.record['cw_executable_path'] = str(self.cw)
        self.record['sandbox_executable_path'] = str(self.sandbox)
        self.selection.update({
            'codex_executable_path':str(self.codex),'cw_executable_path':str(self.cw),
        })
        self.assertTrue(smoke.validate(self.record,self.selection))
        with patch.object(smoke,'verify_bubblewrap'):
            result=smoke.run_authorized_smoke(
                self.record,evidence=self.evidence,cw_executable=self.cw,codex_executable=self.codex)
        self.assertEqual(result['status'],'PASS', (
            result,
            (self.evidence/'evidence-error.json').read_text() if (self.evidence/'evidence-error.json').exists() else None,
            (self.evidence/'codex-events.jsonl').read_text(errors='replace') if (self.evidence/'codex-events.jsonl').exists() else None,
        ))
        self.assertEqual(result['provider_request_records'],1)
        self.assertEqual(result['provider_spend_usd'],0.000002)
        self.assertTrue((self.evidence/'request-00'/'response.bin').is_file())
        self.assertTrue((self.evidence/'spend-guard-ledger.json').is_file())
        self.assertTrue((self.evidence/'codex-home').is_dir())

    def test_resource_failure_consumes_auth_and_writes_inconclusive_manifest(self):
        with patch.object(smoke,'verify_bubblewrap'), \
             patch.object(smoke,'private_runtime_root',side_effect=OSError('resource exhausted')):
            result = smoke.run_authorized_smoke(
                self.record,evidence=self.evidence,cw_executable=self.cw,codex_executable=self.codex)
        self.assertEqual(result['status'],'INCONCLUSIVE')
        self.assertEqual(result['provider_request_records'],0)
        self.assertTrue((self.evidence/'runner-error.json').is_file())
        self.assertTrue((self.evidence/'run-manifest.json').is_file())
        with self.assertRaises(FileExistsError):
            smoke.claim_once(self.record,self.claims)

    def test_partial_provider_evidence_is_preserved_and_marked_unavailable(self):
        db = self.evidence.parent / 'partial.db'
        conn = sqlite3.connect(db)
        conn.executescript('''
            CREATE TABLE turns(id TEXT,turn_json TEXT,created_at TEXT);
            CREATE TABLE wire_requests(turn_id TEXT,body BLOB);
            CREATE TABLE wire_egress(turn_id TEXT,body BLOB);
            CREATE TABLE wire_responses(turn_id TEXT,body BLOB,complete INTEGER,truncated INTEGER);
            CREATE TABLE spend_guard_ledger(turn_id TEXT,session_id TEXT,provider TEXT,model TEXT,
                admission_status TEXT,reservation_usd REAL,settlement_status TEXT,settled_usd REAL,
                usage_json TEXT,error TEXT,updated_at TEXT);
        ''')
        conn.execute('INSERT INTO turns VALUES(?,?,?)',('turn-1',json.dumps({'outcome':'blocked'}),'now'))
        conn.execute('INSERT INTO wire_requests VALUES(?,?)',('turn-1',b'{"model":"gpt-6-luna"}'))
        conn.commit(); conn.close()
        records, ledger, issues = smoke.collect_live_evidence(db,self.evidence,None)
        self.assertEqual(len(records),1)
        self.assertEqual(ledger,[])
        self.assertGreaterEqual(len(issues),4)
        self.assertTrue((self.evidence/'cw.db').is_file())
        self.assertEqual((self.evidence/'request-00'/'ingress.bin').read_bytes(),b'{"model":"gpt-6-luna"}')
        self.assertEqual(json.loads((self.evidence/'request-00'/'usage.json').read_bytes()),{'status':'NOT_AVAILABLE'})
        self.assertTrue((self.evidence/'evidence-issues.json').is_file())


if __name__ == '__main__':
    unittest.main()
