import json,shutil,subprocess,tempfile
from pathlib import Path
B=Path(__file__).resolve().parent
refs={'f1':"def divide_or_fallback(a,b,fallback=0.0):\n if b==0:return fallback\n return a/b\ndef safe_mean(numbers):\n if not numbers:return 0.0\n return sum(numbers)/len(numbers)\n",'f3':"class CachedStore:\n def __init__(self,backing_store):self.backing_store=backing_store;self.cache={}\n def get(self,k):\n  if k in self.cache:return self.cache[k]\n  v=self.backing_store.get(k)\n  if v is not None:self.cache[k]=v\n  return v\n def put(self,k,v):self.cache[k]=v;self.backing_store.put(k,v)\n"}
results=[]
for family in ['f1','f3']:
 source='math_ops.py' if family=='f1' else 'cache.py'
 for case in ['buggy','reference','protected','signature','behavior']:
  with tempfile.TemporaryDirectory(prefix='cw104b-acceptance-') as tmp:
   w=Path(tmp);shutil.copytree(B/'fixtures'/family,w,dirs_exist_ok=True)
   if case!='buggy':(w/source).write_text(refs[family])
   if case=='protected':(w/'policy.txt').write_text('modified')
   if case=='signature':(w/source).write_text((w/source).read_text().replace('fallback=0.0','fallback=1.0') if family=='f1' else (w/source).read_text().replace('def get(self,k):','def get(self,k,extra=None):'))
   if case=='behavior':(w/source).write_text((w/source).read_text().replace('return a/b','return float(a/b)') if family=='f1' else (w/source).read_text().replace('if v is not None:','if v:'))
   p=subprocess.run(['python3','-O',str(B/'acceptance.py'),family,str(w)],capture_output=True,text=True,check=False);r=json.loads(p.stdout);expected=case=='reference'
   if r['passed']!=expected:raise RuntimeError(f'Acceptance selfcheck failed: {family}/{case}')
   results.append({'family':family,'case':case,'passed':r['passed'],'violations':r['violations'],'expected_result_confirmed':True})
print(json.dumps({'cases':results,'all_expected_results_confirmed':True},indent=2))
