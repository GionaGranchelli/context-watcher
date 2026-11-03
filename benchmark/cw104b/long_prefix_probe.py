"""Bounded cache diagnostic: frozen OMP payloads, no agent tools executed."""
import copy, hashlib, json, time, urllib.request
from pathlib import Path
import yaml
BASE=Path(__file__).resolve().parent
VAULT=Path.home()/'.local/share/context-watcher/cw104b-evidence-vault'
DEST=BASE.parent.parent/'.audit/cw104b/long-prefix-probe.json'
ENDPOINT='http://192.168.1.156:8095'
MODEL='Qwen3.8-Flash-Next-GSQ-RCO-IQ2_XS'

def run():
 key=yaml.safe_load(Path('/tmp/cw104a-local-qual/home/.omp/profiles/cw104a-m1/agent/models.yml').read_text())['providers']['machinist-strata']['apiKey']
 deadline=time.monotonic()+600
 if DEST.exists():raise RuntimeError('Evidence already exists; preserve it and explicitly select a new attempt before rerunning')
 result={'kind':'bounded long-prefix cache diagnostic; no coding or tool execution','status':'RUNNING','requests':[]}
 def save():DEST.write_text(json.dumps(result,indent=2)+'\n')
 def call(path,body=None):
  remaining=deadline-time.monotonic()
  if remaining<=0:raise RuntimeError('Diagnostic deadline exceeded')
  req=urllib.request.Request(ENDPOINT+path,data=json.dumps(body).encode() if body is not None else None,headers={'Authorization':'Bearer '+key,'Content-Type':'application/json'})
  with urllib.request.urlopen(req,timeout=min(remaining,300)) as response:return json.load(response)
 def idle(metrics):
  if metrics['live']['state']!='idle' or metrics['live'].get('queued'):raise RuntimeError('Runtime busy or queued')
  if not metrics['conversation_cache']['enabled']:raise RuntimeError('Cache disabled')
 try:
  bodies={arm:json.loads((VAULT/('cw104b-long-preflight-'+arm+'.json')).read_text()) for arm in ['m1','m2']}
  mock=json.loads((DEST.parent/'committed-pin-mock-preflight.json').read_text())
  for i,arm in enumerate(['m1','m2']):
   if hashlib.sha256(json.dumps(bodies[arm],sort_keys=True).encode()).hexdigest()!=mock['request_hashes'][i]:raise RuntimeError('Preflight payload hash mismatch')
  if not all(mock[k] for k in ['full_expected_transformation_equal','metadata_transmitted','original_transcripts_pin_absent']):raise RuntimeError('Mock gate failed')
  for label,arm in [('warmup','m1'),('m2','m2'),('m1','m1')]:
   body=copy.deepcopy(bodies[arm]);body['stream']=False;body.pop('stream_options',None);body['max_completion_tokens']=16
   before=call('/metrics');idle(before);result['pending_dispatch']={'label':label,'metrics_before':{k:before.get(k) for k in ['time','live','conversation_cache','requests_kept']}};save();started=time.monotonic();response=call('/v1/chat/completions',body);after=call('/metrics')
   usage=response.get('usage');entry={'label':label,'elapsed_seconds':time.monotonic()-started,'model':response.get('model'),'usage':usage,'finish_reason':response['choices'][0].get('finish_reason'),'request_sha256':hashlib.sha256(json.dumps(body,sort_keys=True).encode()).hexdigest(),'metrics_before':{k:before.get(k) for k in ['time','live','conversation_cache','requests_kept']},'metrics_after':{k:after.get(k) for k in ['time','live','conversation_cache','requests_kept']},'server_request':after['requests'][0]}
   result['requests'].append(entry);result.pop('pending_dispatch',None);save();idle(after)
   if response.get('model')!=MODEL or not usage:raise RuntimeError('Model identity or usage missing')
   if after['conversation_cache']['requests']-before['conversation_cache']['requests']!=1:raise RuntimeError('Concurrent request counter mismatch')
   if not 16384<=usage['prompt_tokens']<=32768:raise RuntimeError('Actual provider context outside frozen band')
  matched=result['requests'][1:];cached=[x['usage'].get('prompt_tokens_details',{}).get('cached_tokens') for x in matched];reused=[x['server_request'].get('reused') for x in matched]
  result['cached_tokens']=cached;result['server_reused_tokens']=reused
  if None in cached or cached[0]!=cached[1] or cached[0]<16384 or reused!=cached:raise RuntimeError('Equivalent long-prefix cache state not established')
  result['status']='LONG_PREFIX_DIAGNOSTIC_PASS_OMP_CODING_STILL_REQUIRED'
 except Exception as error:
  result['status']='BLOCKED_WITH_EVIDENCE';result['error_type']=type(error).__name__;result['error']=str(error);save();raise
 save();print(json.dumps({'status':result['status'],'cached_tokens':result['cached_tokens']}))
if __name__=='__main__':run()
