"""Replay the historical 128K mismatch through an isolated CW copy; no inference."""
import copy,hashlib,json,os,socket,sqlite3,subprocess,tempfile,time,urllib.request
from pathlib import Path
ROOT=Path(__file__).resolve().parents[2]
TURN='turn_2853e5539eda4e8bb1f2eb11fd17037c'
SESSION='omp_01a19f74-af46-73c7-9ae9-d505af565379'
def port():
 with socket.socket() as s:s.bind(('127.0.0.1',0));return s.getsockname()[1]
def main():
 source=sqlite3.connect('file:/tmp/cw104a-local-qual/cw-run2.db?mode=ro',uri=True)
 row=source.execute('select wr.body,we.body from wire_requests wr join wire_egress we on wr.turn_id=we.turn_id where wr.turn_id=?',(TURN,)).fetchone()
 if not row:raise RuntimeError('Original wire unavailable')
 request,egress=map(json.loads,row);messages=copy.deepcopy(request['messages']);tail=messages[-1]['content'];blocks=[b for b in tail if b.get('type')=='text' and b.get('text','').strip().startswith('<cw_pinned_context>') and b.get('text','').strip().endswith('</cw_pinned_context>')]
 if len(blocks)!=1:raise RuntimeError('Standalone experiment pin is ambiguous')
 tail.remove(blocks[0]);literal_tags=sum(json.dumps(m).count('<cw_pinned_context>') for m in messages)
 with tempfile.TemporaryDirectory(prefix='cw104b-replay-') as d:
  temp=Path(d);db=sqlite3.connect(temp/'store.db');source.backup(db);db.close();source.close();(temp/'store.db').chmod(0o600);control,proxy=port(),port();env={k:os.environ[k] for k in ['PATH','LANG'] if k in os.environ};env.update(HOME=str(temp),CW_CONTROL_TOKEN_FILE=str(temp/'control-token'))
  process=subprocess.Popen([str(ROOT/'target/debug/cw'),'--db',str(temp/'store.db'),'serve','--port',str(control),'--proxy-port',str(proxy)],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
  try:
   for _ in range(100):
    try:
     with socket.create_connection(('127.0.0.1',control),timeout=.1):break
    except OSError:time.sleep(.05)
   else:raise RuntimeError('Isolated daemon unavailable')
   token=(temp/'control-token').read_text().strip();q=urllib.request.Request(f'http://127.0.0.1:{control}/api/v1/sessions/{SESSION}/transform',data=json.dumps({'messages':messages,'provider':'openai'}).encode(),headers={'Content-Type':'application/json','x-cw-control-token':token},method='POST')
   with urllib.request.urlopen(q,timeout=30) as response:transformed=json.load(response)
   expected=copy.deepcopy(request);expected['messages']=transformed['messages'];result={'turn':TURN,'kind':'isolated offline transform replay; no provider request','request_equals_egress':request==egress,'exact_full_payload_replay':expected==egress,'applied_rules':transformed.get('applied_rules'),'historical_literal_pin_tags_preserved':literal_tags,'removed_standalone_blocks':1,'original_broad_verifier_source':'NOT_AVAILABLE','raw_sha256':[hashlib.sha256(x if isinstance(x,bytes) else x.encode()).hexdigest() for x in row]}
   (ROOT/'.audit/cw104b/matrix-mismatch-replay.json').write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result))
   if not result['exact_full_payload_replay']:raise RuntimeError('Historical mismatch still reproduces')
  finally:
   process.terminate()
   try:process.wait(timeout=5)
   except subprocess.TimeoutExpired:process.kill();process.wait()
if __name__=='__main__':main()
