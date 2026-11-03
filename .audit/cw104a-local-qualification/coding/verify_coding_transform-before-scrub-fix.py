import json,sqlite3,urllib.request,urllib.parse,re,hashlib
from pathlib import Path
BASE=Path('/tmp/cw104a-local-qual')
pin='Preserve public function signatures, existing valid-input behavior, and test files. Keep the requested fix focused.'
A=Path('/home/gionag/Development/contextWatcher/.audit/cw104a-local-qualification/coding')
runs={}
for p in (A/'live-evidence').glob('*/summary.json'):
 d=json.loads(p.read_text());r=d['run']
 if not r['tag'].endswith('m2') or not r.get('cw_session'):continue
 r['egress_captures']=[{'turn_id':x['turn_id']} for x in d['captures'] if x['stage']=='coding']
 runs[r['tag']]=r
db=sqlite3.connect(BASE/'cw-run2.db'); db.row_factory=sqlite3.Row
results=[]
for name,run in runs.items():
 captures=run.get('egress_captures',[])
 for cap in captures:
  row=db.execute('select wr.body req,we.body egr,t.turn_json from turns t join wire_requests wr on wr.turn_id=t.id join wire_egress we on we.turn_id=t.id where t.id=?',(cap['turn_id'],)).fetchone()
  if not row: results.append({'run':name,'turn':cap['turn_id'],'error':'turn missing'}); continue
  req=json.loads(row['req']); egr=json.loads(row['egr']); turn=json.loads(row['turn_json'])
  transcript=Path(run['transcript']).read_text()
  rule=db.execute('select id,status,rule_json from mutations where session_id=?',(run['cw_session'],)).fetchall()
  committed=[r for r in rule if r['status'].lower()=='committed' and pin in r['rule_json']]
  messages=req.get('messages',[])
  if not messages or messages[-1].get('role')!='user': results.append({'run':name,'turn':cap['turn_id'],'error':'no user tail'}); continue
  def scrub(v):
   if isinstance(v,str):
    return re.sub(r'<cw_pinned_context>.*?</cw_pinned_context>','',v,flags=re.S).replace(pin,'')
   if isinstance(v,list):
    out=[]
    for x in v:
     if isinstance(x,dict) and isinstance(x.get('text'),str) and re.fullmatch(r'\s*<cw_pinned_context>.*?</cw_pinned_context>\s*',x['text'],flags=re.S):
      continue
     y=scrub(x)
     if isinstance(y,dict) and y.get('text')=='': continue
     out.append(y)
    return out
   if isinstance(v,dict): return {k:scrub(x) for k,x in v.items()}
   return v
  messages[-1]['content']=scrub(messages[-1].get('content'))
  body=json.dumps({'messages':messages,'provider':'openai'},ensure_ascii=False).encode()
  url='http://127.0.0.1:14400/api/v1/sessions/'+urllib.parse.quote(run['cw_session'],safe='')+'/transform'
  q=urllib.request.Request(url,data=body,headers={'Content-Type':'application/json','x-cw-control-token':(BASE/'control-token').read_text().strip()},method='POST')
  transformed=json.load(urllib.request.urlopen(q,timeout=30))
  outmsg=transformed.get('messages')
  def differences(a,b,path='$'):
   if type(a)!=type(b):return [{'path':path,'left_type':type(a).__name__,'right_type':type(b).__name__}]
   if isinstance(a,dict):
    out=[]
    for k in a.keys()|b.keys():
     if k not in a or k not in b:out.append({'path':path+'.'+k,'missing_side':'left' if k not in a else 'right'})
     else:out.extend(differences(a[k],b[k],path+'.'+k))
    return out[:10]
   if isinstance(a,list):
    out=[] if len(a)==len(b) else [{'path':path,'left_len':len(a),'right_len':len(b)}]
    for i,(x,y) in enumerate(zip(a,b)):out.extend(differences(x,y,path+'['+str(i)+']'))
    return out[:10]
   if a!=b:return [{'path':path,'left_length':len(a) if isinstance(a,str) else None,'right_length':len(b) if isinstance(b,str) else None,'left_tail':a[-100:] if isinstance(a,str) and len(a)<1000 else None,'right_tail':b[-100:] if isinstance(b,str) and len(b)<1000 else None}]
   return []
  replay_differences=differences(outmsg,egr.get('messages'))

  txt=json.dumps(egr,ensure_ascii=False,separators=(',',':'))
  tail=egr.get('messages',[])[-1].get('content',[]) if egr.get('messages') else []
  results.append({
   'run':name,'turn':cap['turn_id'],'session':run['cw_session'],'provider_input':(turn.get('observed_usage') or {}).get('prompt_tokens'),
   'committed_rule':committed[0]['id'] if committed else None,'pin_absent_from_saved_transcript':pin not in transcript,
   'request_equals_egress':row['req']==row['egr'],'pin_occurrences':txt.count(pin),
   'pin_in_tail':json.dumps(tail,ensure_ascii=False,separators=(',',':')).count(pin)==1,
   'proxy_applied_mutation_ids':turn.get('applied_mutation_ids'),
   'transform_api_applied_rules':transformed.get('applied_rules'),
   'exact_transform_replay':outmsg==egr.get('messages'),'replay_differences':replay_differences
  })
out={'verified_turns':len(results),'results':results,'note':'The OMP client extension applies the committed overlay before cw-proxy capture. The proxy therefore records request==egress and empty applied_mutation_ids; this verifier replays the local transform endpoint against the unpinned request shape.'}
path=A/'coding-transform-replay.json'
path.write_text(json.dumps(out,indent=2)+'\n')
print(json.dumps(out,indent=2))
