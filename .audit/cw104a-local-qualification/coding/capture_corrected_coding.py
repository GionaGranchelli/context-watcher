import json,sqlite3,gzip,hashlib,shutil
from pathlib import Path
A=Path(__file__).resolve().parent; B=Path('/tmp/cw104a-local-qual'); c=sqlite3.connect(B/'cw-run2.db');c.row_factory=sqlite3.Row
pin='Preserve public function signatures, existing valid-input behavior, and test files. Keep the requested fix focused.'
for result in A.glob('f*-32k-*-r*-m*.json'):
 run=json.loads(result.read_text())
 if 'tag' not in run or 'workspace' not in run:continue
 tag=run['tag'];caps=[];dst=A/'live-evidence'/tag;dst.mkdir(parents=True,exist_ok=True)
 if run.get('transcript') and Path(run['transcript']).exists():
  raw=Path(run['transcript']).read_bytes()
  with gzip.open(dst/'transcript-snapshot.jsonl.gz','wb') as f:f.write(raw)
  run['transcript_snapshot_sha256']=hashlib.sha256(raw).hexdigest()
 for r in c.execute('select t.id,t.created_at,t.turn_json,wr.body req,we.body egr from turns t join wire_requests wr on wr.turn_id=t.id join wire_egress we on we.turn_id=t.id'):
  e=json.loads(r['egr'])
  if tag not in json.dumps(e):continue
  raw={'request':json.loads(r['req']),'egress':e,'turn':json.loads(r['turn_json'])}
  with gzip.open(dst/(r['id']+'.json.gz'),'wt') as f:json.dump(raw,f)
  caps.append({'stage':'coding' if ('Evaluation ID: '+tag) in json.dumps(e) else 'bootstrap','turn_id':r['id'],'created_at':r['created_at'],'request_equals_egress':r['req']==r['egr'],'egress_sha256':hashlib.sha256(r['egr']).hexdigest(),'pin_occurrences':json.dumps(e).count(pin),'temperature':e.get('temperature'),'top_p':e.get('top_p'),'usage':raw['turn'].get('observed_usage'),'outcome':raw['turn'].get('outcome')})
 summary={'run':run,'captures':caps,'snapshot_only':run.get('status')!='finished'}
 (dst/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
 print(tag,run.get('status'),'captured requests',len(caps),'completed',sum(x['outcome']=='completed' for x in caps))
