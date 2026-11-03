import json,statistics
from pathlib import Path
A=Path(__file__).resolve().parent;runs={}
for p in A.glob('f*-32k-v2-r*-m[12].json'):
 r=json.loads(p.read_text());snap=A/'live-evidence'/r['tag']/'summary.json'
 if not snap.exists():continue
 cs=[x for x in json.loads(snap.read_text())['captures'] if x['stage']=='coding'];known=[x for x in cs if x.get('usage') is not None];counts={}
 for x in cs:counts[x['outcome']]=counts.get(x['outcome'],0)+1
 complete=bool(cs) and len(known)==len(cs) and all(x['outcome']=='completed' for x in cs)
 runs[r['tag']]={'status':r['status'],'acceptance_passed':r.get('acceptance_passed'),'task_seconds':r.get('task_seconds'),'coding_dispatches':len(cs),'outcomes':counts,'usage_complete':complete,'missing_usage_dispatches':len(cs)-len(known),'known_input_tokens':sum(x['usage'].get('prompt_tokens',0) for x in known),'known_output_tokens':sum(x['usage'].get('completion_tokens',0) for x in known),'first_request_input_tokens':cs[0]['usage'].get('prompt_tokens') if cs and cs[0].get('usage') else None,'interpretation':'Known usage is a lower bound whenever usage_complete is false.'}
pairs=[]
for fam in ['f1','f3']:
 for rep in [1,2,3]:
  tags={m:f'{fam}-32k-v2-r{rep}-{m}' for m in ['m1','m2']}
  if any(t not in runs or runs[t]['status']!='finished' for t in tags.values()):continue
  a,b=[runs[tags[m]] for m in ['m1','m2']];clean=a['usage_complete'] and b['usage_complete']
  pairs.append({'family':fam,'rep':rep,'both_acceptance_passed':a['acceptance_passed'] and b['acceptance_passed'],'clean_completed_dispatches_and_usage':clean,'m2_minus_m1_seconds':b['task_seconds']-a['task_seconds'],'m2_minus_m1_input_tokens':b['known_input_tokens']-a['known_input_tokens'] if clean else None,'interpretation':'Transport-affected or incomplete-usage pairs cannot establish clean treatment performance.' if not clean else 'Single pair descriptive measurement; no general effect established.'})
out={'runs':runs,'completed_pairs':pairs,'planned_pairs':6,'scope':'Frozen corrected v2 only; original and 38K calibration runs excluded. All failures retained.','status':'incomplete' if len(pairs)<6 else 'all_pairs_recorded_pending_completion_audit'};(A/'corrected-campaign-results.json').write_text(json.dumps(out,indent=2)+'\n');print('Recorded runs',len(runs),'completed pairs',len(pairs),'of 6')
