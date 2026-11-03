import os,json,tempfile,shutil,threading,subprocess,hashlib,yaml,uuid,socket,time,sqlite3,copy
from datetime import datetime,timezone
from pathlib import Path
from http.server import ThreadingHTTPServer,BaseHTTPRequestHandler
B=Path(__file__).resolve().parent;captured=[]
protocol=json.loads((B/'protocol.json').read_text())
vault=Path.home()/'.local/share/context-watcher/cw104b-evidence-vault'
history=(vault/'cw104b-frozen-history.txt').read_text()
if hashlib.sha256(history.encode()).hexdigest()!=protocol['history_sha256']:raise RuntimeError('Frozen history hash mismatch')
class Handler(BaseHTTPRequestHandler):
 def log_message(self,*args):pass
 def do_POST(self):
  body=json.loads(self.rfile.read(int(self.headers['Content-Length'])));captured.append(body)
  self.send_response(200);self.send_header('Content-Type','text/event-stream');self.end_headers()
  first=len(captured)%2==1
  delta={'role':'assistant','tool_calls':[{'index':0,'id':'call_mock_read','type':'function','function':{'name':'read','arguments':json.dumps({'path':'cache.py'})}}]} if first else {'role':'assistant','content':'OK'}
  for x in [{'id':'mock','object':'chat.completion.chunk','model':body['model'],'choices':[{'index':0,'delta':delta,'finish_reason':None}]},{'id':'mock','object':'chat.completion.chunk','model':body['model'],'choices':[{'index':0,'delta':{},'finish_reason':'tool_calls' if first else 'stop'}]}]:self.wfile.write(('data: '+json.dumps(x)+'\n\n').encode())
  self.wfile.write(b'data: [DONE]\n\n');self.wfile.flush()
server=ThreadingHTTPServer(('127.0.0.1',0),Handler);threading.Thread(target=server.serve_forever,daemon=True).start();results=[]
try:
 with tempfile.TemporaryDirectory(prefix='cw104b-mock-') as tmp:
  T=Path(tmp);home=T/'home'
  def port():
   with socket.socket() as sock:sock.bind(('127.0.0.1',0));return sock.getsockname()[1]
  control_port,proxy_port=port(),port();cw=B.parent.parent/'target/debug/cw';daemon_env={k:os.environ[k] for k in ['PATH','LANG'] if k in os.environ};daemon_env.update(HOME=str(home),CW_CONTROL_TOKEN_FILE=str(T/'control-token'),CW_OPENAI_UPSTREAM=f'http://127.0.0.1:{server.server_port}',CW_CAPTURE_WIRE_EGRESS='1')
  daemon=subprocess.Popen([str(cw),'--db',str(T/'store.db'),'serve','--port',str(control_port),'--proxy-port',str(proxy_port)],env=daemon_env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
  for _ in range(100):
   try:
    with socket.create_connection(('127.0.0.1',control_port),timeout=.1):break
   except OSError:time.sleep(.05)
  else:raise RuntimeError('Isolated CW daemon did not start')
  provider=yaml.safe_load(Path('/tmp/cw104a-local-qual/home/.omp/profiles/cw104a-m1/agent/models.yml').read_text())['providers']['machinist-strata'];provider['apiKey']='mock-local-only';provider['baseUrl']=f'http://127.0.0.1:{proxy_port}/openai/v1'
  cfg=home/'.omp/profiles/mock/agent';cfg.mkdir(parents=True);(cfg/'models.yml').write_text(yaml.safe_dump({'providers':{'machinist-strata':provider}}));(cfg/'config.yml').write_text('temperature: 0\ntopP: 1\n')
  native=Path('/tmp/cw104a-local-qual/home/.omp/natives')
  if native.exists():shutil.copytree(native,home/'.omp/natives',copy_function=os.link)
  for arm in ['m1','m2']:
   w=T/'workspace';shutil.copytree(B/'fixtures/f3',w);env={k:os.environ[k] for k in ['PATH','LANG','TERM'] if k in os.environ};env.update(HOME=str(home),OMP_PROFILE='mock',PI_CODING_AGENT_DIR=str(T/'state'/arm),CW104B_TRACE_FILE=str(T/'hook-trace.jsonl'),CW_DAEMON_URL=f'http://127.0.0.1:{control_port}',CW_CONTROL_TOKEN_FILE=str(T/'control-token'))
   prompt='Implement the change requested in the opening brief.'
   early=protocol['families']['f3']['opening_brief']+'\n\nHistorical development reference:\n'+history+'\nCW104B_FINAL_REQUEST_BOUNDARY'
   seed=T/(arm+'-user-only.jsonl');now=datetime.now(timezone.utc).isoformat();sid=str(uuid.uuid4());seed.write_text(json.dumps({'type':'session','version':3,'id':sid,'timestamp':now,'cwd':str(w)})+'\n'+json.dumps({'type':'message','id':'opening-user-input','parentId':None,'timestamp':now,'message':{'role':'user','content':[{'type':'text','text':early}],'timestamp':int(datetime.now().timestamp()*1000)}})+'\n')
   if arm=='m2':
    subprocess.run([str(cw),'--db',str(T/'store.db'),'import','--harness','omp','--workspace',str(w),str(seed)],env=env,capture_output=True,check=True)
    subprocess.run([str(cw),'--db',str(T/'store.db'),'pin','CW104B-Constraints','Preserve public signatures. Do not modify store.py, test_cache_system.py, or policy.txt.','--session','omp_'+sid],env=env,capture_output=True,check=True)
   args=['omp','--model=machinist-strata/Qwen3.8-Flash-Next-GSQ-RCO-IQ2_XS',f'--cwd={w}',f'--session-dir={T}/sessions/{arm}',f'--resume={seed}','--system-prompt=Use relative file paths to implement the user request.','--tools=read,edit,write,grep,glob','--no-context-files','--no-skills','--no-rules','--no-extensions',f'--extension={B.parent.parent}/plugins/pi-opencode/index.js',f'--extension={B}/request_options.js','--thinking=off','--no-title','--max-time=30','-p',prompt]
   cp=subprocess.run(args,env=env,capture_output=True,text=True,timeout=40);results.append({'arm':arm,'exit':cp.returncode,'captured_requests':len(captured),'stderr':cp.stderr});w.rename(T/('completed-'+arm))
  db=sqlite3.connect(T/'store.db');rules=[{'status':x[0]} for x in db.execute('select status from mutations')];db.close()
  originals_pin_absent=all(('\n<cw_pinned_context>\n- [CW104B-Constraints]: '+protocol['families']['f3']['pin']+'\n</cw_pinned_context>') not in json.loads((T/(arm+'-user-only.jsonl')).read_text().splitlines()[1])['message']['content'][0]['text'] for arm in ['m1','m2'])
  trace=(T/'hook-trace.jsonl').read_text() if (T/'hook-trace.jsonl').exists() else ''
finally:
 if 'daemon' in globals():
  daemon.terminate()
  try:daemon.wait(timeout=5)
  except subprocess.TimeoutExpired:daemon.kill();daemon.wait()
 server.shutdown();server.server_close()
if len(captured)==2:
 vault.mkdir(parents=True,exist_ok=True,mode=0o700)
 for arm,body in zip(['m1','m2'],captured):
  target=vault/('cw104b-long-preflight-'+arm+'.json');target.write_text(json.dumps(body));target.chmod(0o600)
expected=copy.deepcopy(captured[0]) if captured else {}
if expected:
 expected['messages'][-1]['content'].append({'type':'text','text':'\n<cw_pinned_context>\n- [CW104B-Constraints]: Preserve public signatures. Do not modify store.py, test_cache_system.py, or policy.txt.\n</cw_pinned_context>'})
result={'kind':'mock-only harness preflight; no model inference','runs':results,'event_trace':trace,'history_sha256':protocol['history_sha256'],'history_characters':len(history),'requests':len(captured),'request_top_keys':[list(x) for x in captured],'metadata_transmitted':bool(captured) and all(x.get('seed')==104002 and x.get('strata_prefix',{}).get('messages',0)>0 for x in captured),'full_expected_transformation_equal':len(captured)==4 and expected==captured[2],'tool_followup_full_payload_equal':len(captured)==4 and captured[1]==captured[3],'committed_rules':rules,'original_transcripts_pin_absent':originals_pin_absent,'request_hashes':[hashlib.sha256(json.dumps(x,sort_keys=True).encode()).hexdigest() for x in captured]};Path('.audit/cw104b/followup-mock-preflight.json').write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result,indent=2))
if not(result['metadata_transmitted'] and result['full_expected_transformation_equal'] and result['tool_followup_full_payload_equal'] and originals_pin_absent and len(rules)==1 and rules[0]['status']=='committed' and all(r['exit']==0 for r in results)):raise SystemExit(1)
