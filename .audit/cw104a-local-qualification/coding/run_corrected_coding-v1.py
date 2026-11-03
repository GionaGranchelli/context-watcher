import os,json,subprocess,time,shutil,sys,hashlib
from pathlib import Path
R=Path('/home/gionag/Development/contextWatcher'); B=Path('/tmp/cw104a-local-qual'); A=R/'.audit/cw104a-local-qualification/coding'
family,mode,rep=sys.argv[1:]; tag=f'{family}-32k-corrected-r{rep}-{mode}'; work=B/'corrected-workspaces'/tag; sessions=B/'corrected-sessions'/tag
if work.exists(): raise RuntimeError('Refusing to reuse workspace')
shutil.copytree(A/'pristine-fixtures'/family,work);sessions.mkdir(parents=True)
record={'tag':tag,'status':'bootstrap_running','workspace':str(work),'initial_hashes':{p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in work.glob('*.py')}}
result=A/(tag+'.json')
def save(): result.write_text(json.dumps(record,indent=2)+'\n')
save()
env=os.environ.copy();env.update(HOME=str(B/'home'),OMP_PROFILE='cw104a-'+mode,PI_CODING_AGENT_DIR=str(B/'corrected-agent-state'/tag),CW_DAEMON_URL='http://127.0.0.1:14400',CW_CONTROL_TOKEN_FILE=str(B/'control-token'))
cmd=['omp','--model=machinist-strata/Qwen3.8-Flash-Next-GSQ-RCO-IQ2_XS',f'--cwd={work}',f'--session-dir={sessions}','--tools=read,edit,write,grep,glob','--no-context-files','--no-skills','--no-rules','--no-extensions',f'--extension={R}/plugins/pi-opencode/index.js','--thinking=low','--no-title','--max-time=1800','--approval-mode=yolo']
source=json.loads((A/'transcript-audit.json').read_text());original=next(x for x in source if x['tag'].startswith(family))
def run(args,stage):
 start=time.monotonic();p=subprocess.run(cmd+args,cwd=R,env=env,capture_output=True,text=True,timeout=1900)
 (A/(tag+'-'+stage+'.stdout')).write_text(p.stdout);(A/(tag+'-'+stage+'.stderr')).write_text(p.stderr)
 record[stage+'_rc']=p.returncode;record[stage+'_seconds']=time.monotonic()-start;save()
 if p.returncode:raise RuntimeError(stage+' failed')
run(['-p',original['users'][0]['text']],'bootstrap')
transcript=next(sessions.glob('*.jsonl'));record['transcript']=str(transcript)
if mode=='m2':
 cw=[str(R/'target/debug/cw'),'--db',str(B/'cw-run2.db')]
 p=subprocess.run(cw+['import','--harness','omp','--workspace',str(work),str(transcript)],env=env,capture_output=True,text=True,check=True)
 sid=p.stdout.strip().split()[-1];record['cw_session']=sid
 subprocess.run(cw+['pin','CW104A-TaskConstraints','Preserve public function signatures, existing valid-input behavior, and test files. Keep the requested fix focused.','--session',sid],env=env,capture_output=True,text=True,check=True)
record['status']='task_running';save()
run([f'--resume={transcript}',f'@{A}/prior-repository-review-history.txt','-p',original['users'][-1]['text']+' Evaluation ID: '+tag],'task')
p=subprocess.run(['python3','-B','-m','unittest','discover'],cwd=work,capture_output=True,text=True,timeout=30)
record.update(status='finished',acceptance_passed=p.returncode==0,acceptance_output=p.stdout+p.stderr);save();print(json.dumps(record))
