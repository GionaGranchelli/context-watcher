import json,ast,subprocess,hashlib
from pathlib import Path
A=Path(__file__).resolve().parent;dest=A/'corrected-final-behavior.json';previous=json.loads(dest.read_text()) if dest.exists() else {};out={}
def sig(s):return {x.name:ast.dump(x.args) for x in ast.walk(ast.parse(s)) if isinstance(x,ast.FunctionDef)}
for p in A.glob('f*-32k-v2-r*-m[12].json'):
 r=json.loads(p.read_text())
 if r.get('status')!='finished':continue
 W=Path(r['workspace']);fam=r['tag'][:2];source='math_ops.py' if fam=='f1' else 'cache.py';fingerprint=hashlib.sha256(b''.join(x.read_bytes() for x in sorted(W.glob('*.py')))).hexdigest()
 if previous.get(r['tag'],{}).get('fingerprint')==fingerprint:out[r['tag']]=previous[r['tag']];continue
 if fam=='f1':code="from math_ops import *; assert divide_or_fallback(9,3)==3; assert divide_or_fallback(1,0,17)==17; assert divide_or_fallback(-8,2)==-4; assert safe_mean([])==0.0; assert safe_mean([-2,2])==0; assert safe_mean([2,4,6])==4"
 else:code="from cache import CachedStore\nclass B:\n def __init__(self): self.calls=0; self.data={}\n def get(self,k): self.calls+=1; return self.data.get(k)\n def put(self,k,v): self.data[k]=v\nb=B(); c=CachedStore(b)\nassert c.get('missing') is None and c.get('missing') is None and b.calls==2\nfor i,v in enumerate([0,False,'']):\n k=str(i);b.data[k]=v;before=b.calls;assert c.get(k)==v;assert c.get(k)==v;assert b.calls==before+1\nc.put('x',42);assert b.data['x']==42 and c.get('x')==42"
 cp=subprocess.run(['python3','-B','-c',code],cwd=W,capture_output=True,text=True,timeout=20)
 preserved={x.name:x.read_bytes()==(W/x.name).read_bytes() for x in (A/'pristine-fixtures'/fam).glob('*.py') if x.name.startswith('test_') or x.name=='store.py'}
 out[r['tag']]={'fingerprint':fingerprint,'additional_behavior_passed':cp.returncode==0,'public_signatures_preserved':sig((W/source).read_text())==sig((A/'pristine-fixtures'/fam/source).read_text()),'preserved_files':preserved,'diagnostic':cp.stderr}
dest.write_text(json.dumps(out,indent=2)+'\n');print('Validated completed runs:',len(out),'all passed:',all(x['additional_behavior_passed'] and x['public_signatures_preserved'] and all(x['preserved_files'].values()) for x in out.values()))
