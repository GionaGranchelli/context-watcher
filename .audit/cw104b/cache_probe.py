import json,time,uuid,urllib.request,yaml
from pathlib import Path
A=Path(__file__).resolve().parent;cfg=Path('/tmp/cw104a-local-qual/home/.omp/profiles/cw104a-m1/agent/models.yml');key=yaml.safe_load(cfg.read_text())['providers']['machinist-strata']['apiKey'];base='http://192.168.1.156:8095';model='Qwen3.8-Flash-Next-GSQ-RCO-IQ2_XS'
def call(path,body=None):
 q=urllib.request.Request(base+path,data=json.dumps(body).encode() if body is not None else None,headers={'Authorization':'Bearer '+key,'Content-Type':'application/json'});return json.load(urllib.request.urlopen(q,timeout=30))
result={'kind':'two-request bounded consecutive-prefix cache probe','model':model,'requests':[]};dest=A/'cache-probe-result.json'
def save():dest.write_text(json.dumps(result,indent=2)+'\n')
before=call('/metrics');result['before']={k:before.get(k) for k in ['time','live','conversation_cache','requests_kept']};save()
if before['live']['state']!='idle' or before['live'].get('queued'):raise RuntimeError('Runtime busy; no probe dispatched')
if not before['conversation_cache'].get('enabled'):raise RuntimeError('Cache disabled; no probe dispatched')
messages=[{'role':'system','content':'Cache diagnostic '+str(uuid.uuid4())+'. Reply exactly OK. Do not use tools.'},{'role':'user','content':'Reference contract: preserve public function signatures and designated files. A cache lookup returns a cached non-None value, otherwise consults the backing store. A write updates cache and backing store. A missing value must not be cached. This is a cache transport diagnostic, not an implementation task. Acknowledge with OK.'}]
for n in [1,2]:
 body={'model':model,'messages':messages,'temperature':0,'top_p':1,'seed':104002,'max_tokens':16,'stream':False,'chat_template_kwargs':{'enable_thinking':False}}
 start=time.monotonic();r=call('/v1/chat/completions',body);after=call('/metrics');choice=r['choices'][0];entry={'request':n,'seconds':time.monotonic()-start,'response_model':r.get('model'),'finish_reason':choice.get('finish_reason'),'usage':r.get('usage'),'content':choice.get('message',{}).get('content'),'metrics':{k:after.get(k) for k in ['time','live','conversation_cache','requests_kept','requests']}};result['requests'].append(entry);save()
 if r.get('model')!=model or choice.get('finish_reason')!='stop':raise RuntimeError('Model or completion inconsistent; stopped')
 if after['live']['state']!='idle' or after['live'].get('queued'):raise RuntimeError('Concurrent work observed; stopped')
 if n==1:messages=messages+[choice['message'],{'role':'user','content':'Acknowledge the same reference contract again with OK.'}]
result['status']='completed_pending_cache_isolation_audit';save();print(json.dumps({'status':result['status'],'usage':[x['usage'] for x in result['requests']],'seconds':[x['seconds'] for x in result['requests']]}))
