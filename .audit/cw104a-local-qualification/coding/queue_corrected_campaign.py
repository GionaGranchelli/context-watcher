import time,json,subprocess
from pathlib import Path
from datetime import datetime
A=Path('/home/gionag/Development/contextWatcher/.audit/cw104a-local-qualification/coding');state=A/'corrected-campaign-queue.json';deadline=datetime.fromisoformat('2026-11-03T07:22:22+00:00').timestamp()
def save(v):state.write_text(json.dumps(v,indent=2)+'\n')
def verify(fam,mode,rep):
 subprocess.run(['python3',str(A/'capture_corrected_coding.py')],capture_output=True,check=True)
 tag=f'{fam}-32k-v2-r{rep}-{mode}';s=json.loads((A/'live-evidence'/tag/'summary.json').read_text());r=s['run'];cs=[x for x in s['captures'] if x['stage']=='coding']
 if r.get('status')!='finished' or not r.get('acceptance_passed') or not cs:raise RuntimeError(tag+' incomplete acceptance/capture')
 if not 28000<=cs[0]['usage']['prompt_tokens']<=36000:raise RuntimeError(tag+' context outside frozen band')
 for c in cs:
  if c['pin_occurrences']!=(1 if mode=='m2' else 0) or not c['request_equals_egress'] or c['temperature']!=0 or c['top_p']!=1:raise RuntimeError(tag+' wire validation failed')
 return tag
save({'status':'waiting_for_first_pair','pid':1092305})
while Path('/proc/1092305').exists():
 if time.time()>=deadline:save({'status':'deadline'});raise SystemExit()
 time.sleep(5)
try:
 completed=[verify('f1',m,1) for m in ['m2','m1']]
 plan=[('f3',1,['m1','m2']),('f1',2,['m1','m2']),('f3',2,['m2','m1']),('f1',3,['m2','m1']),('f3',3,['m1','m2'])]
 for fam,rep,modes in plan:
  for mode in modes:
   if deadline-time.time()<120:save({'status':'deadline','completed':completed});raise SystemExit()
   save({'status':'running','family':fam,'rep':rep,'mode':mode,'completed':completed})
   p=subprocess.run(['python3','/tmp/cw104a-local-qual/run_corrected_coding_v2.py',fam,mode,str(rep)],capture_output=True,text=True)
   tag=f'{fam}-32k-v2-r{rep}-{mode}';(A/(tag+'-queue.stdout')).write_text(p.stdout);(A/(tag+'-queue.stderr')).write_text(p.stderr)
   if p.returncode:raise RuntimeError(tag+' runner failed')
   completed.append(verify(fam,mode,rep))
 save({'status':'finished_pending_full_audit','completed':completed})
except Exception as e:save({'status':'halted_for_inspection','reason':str(e)})
