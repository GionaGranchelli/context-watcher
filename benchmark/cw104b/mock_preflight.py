import os,json,tempfile,shutil,threading,subprocess,hashlib,yaml,uuid
from datetime import datetime,timezone
from pathlib import Path
from http.server import ThreadingHTTPServer,BaseHTTPRequestHandler
B=Path(__file__).resolve().parent;captured=[]
class Handler(BaseHTTPRequestHandler):
 def log_message(self,*args):pass
 def do_POST(self):
  body=json.loads(self.rfile.read(int(self.headers['Content-Length'])));captured.append(body)
  self.send_response(200);self.send_header('Content-Type','text/event-stream');self.end_headers()
  for x in [{'id':'mock','object':'chat.completion.chunk','model':body['model'],'choices':[{'index':0,'delta':{'role':'assistant','content':'OK'},'finish_reason':None}]},{'id':'mock','object':'chat.completion.chunk','model':body['model'],'choices':[{'index':0,'delta':{},'finish_reason':'stop'}]}]:self.wfile.write(('data: '+json.dumps(x)+'\n\n').encode())
  self.wfile.write(b'data: [DONE]\n\n');self.wfile.flush()
server=ThreadingHTTPServer(('127.0.0.1',0),Handler);threading.Thread(target=server.serve_forever,daemon=True).start();results=[]
try:
 with tempfile.TemporaryDirectory(prefix='cw104b-mock-') as tmp:
  T=Path(tmp);home=T/'home';provider=yaml.safe_load(Path('/tmp/cw104a-local-qual/home/.omp/profiles/cw104a-m1/agent/models.yml').read_text())['providers']['machinist-strata'];provider['apiKey']='mock-local-only';provider['baseUrl']=f'http://127.0.0.1:{server.server_port}/v1'
  cfg=home/'.omp/profiles/mock/agent';cfg.mkdir(parents=True);(cfg/'models.yml').write_text(yaml.safe_dump({'providers':{'machinist-strata':provider}}));(cfg/'config.yml').write_text('temperature: 0\ntopP: 1\n')
  native=Path('/tmp/cw104a-local-qual/home/.omp/natives')
  if native.exists():shutil.copytree(native,home/'.omp/natives',copy_function=os.link)
  for arm in ['m1','m2']:
   w=T/'workspace';shutil.copytree(B/'fixtures/f3',w);env={k:os.environ[k] for k in ['PATH','LANG','TERM'] if k in os.environ};env.update(HOME=str(home),OMP_PROFILE='mock',PI_CODING_AGENT_DIR=str(T/'state'/arm),CW104B_TRACE_FILE=str(T/'hook-trace.jsonl'))
   prompt='Implement the change requested in the opening brief.'
   early='Opening brief: implement the requested change while preserving designated files.\nHistorical reference: previous source review.\nCW104B_FINAL_REQUEST_BOUNDARY'
   seed=T/(arm+'-user-only.jsonl');now=datetime.now(timezone.utc).isoformat();sid=str(uuid.uuid4());seed.write_text(json.dumps({'type':'session','version':3,'id':sid,'timestamp':now,'cwd':str(w)})+'\n'+json.dumps({'type':'message','id':'opening-user-input','parentId':None,'timestamp':now,'message':{'role':'user','content':[{'type':'text','text':early}],'timestamp':int(datetime.now().timestamp()*1000)}})+'\n')
   args=['omp','--model=machinist-strata/Qwen3.8-Flash-Next-GSQ-RCO-IQ2_XS',f'--cwd={w}',f'--session-dir={T}/sessions/{arm}',f'--resume={seed}','--system-prompt=Use relative file paths to implement the user request.','--tools=read,edit,write,grep,glob','--no-context-files','--no-skills','--no-rules','--no-extensions',f'--extension={B}/request_options.js','--thinking=off','--no-title','--max-time=30','-p',prompt]
   cp=subprocess.run(args,env=env,capture_output=True,text=True,timeout=40);results.append({'arm':arm,'exit':cp.returncode,'captured_requests':len(captured),'stderr':cp.stderr});w.rename(T/('completed-'+arm))
  trace=(T/'hook-trace.jsonl').read_text() if (T/'hook-trace.jsonl').exists() else ''
finally:server.shutdown();server.server_close()
result={'kind':'mock-only harness preflight; no model inference','runs':results,'event_trace':trace,'requests':len(captured),'request_top_keys':[list(x) for x in captured],'metadata_transmitted':bool(captured) and all(x.get('seed')==104002 and x.get('strata_prefix',{}).get('messages',0)>0 for x in captured),'full_request_equal':len(captured)==2 and captured[0]==captured[1],'request_hashes':[hashlib.sha256(json.dumps(x,sort_keys=True).encode()).hexdigest() for x in captured]};Path('.audit/cw104b/omp-mock-preflight.json').write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result,indent=2))
if not(result['metadata_transmitted'] and result['full_request_equal'] and all(r['exit']==0 for r in results)):raise SystemExit(1)
