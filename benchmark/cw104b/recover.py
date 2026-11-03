"""Rebuild recovered metrics from original per-run artifacts without extracting payloads."""
import argparse,collections,hashlib,json,tarfile
from pathlib import Path
ARCHIVE_SHA='67639549b81d5175035b885aa61848bb5d661eb44ff4457d8a82745e5635bf35'
BASE=Path(__file__).resolve().parents[2]

def recover(archive):
 if hashlib.sha256(archive.read_bytes()).hexdigest()!=ARCHIVE_SHA:raise RuntimeError('Archive hash mismatch')
 runs={}
 manifest=json.loads((BASE/'.audit/cw104b/original-file-sha256.json').read_text())
 with tarfile.open(archive) as tar:
  for name,digest in manifest.items():
   data=tar.extractfile('cw104a-local-qualification/'+name).read()
   if hashlib.sha256(data).hexdigest()!=digest:raise RuntimeError('Original file hash mismatch: '+name)
  for family in ['f1','f3']:
   for rep in [1,2,3]:
    for arm in ['m1','m2']:
     tag=f'{family}-32k-v2-r{rep}-{arm}';raw=json.load(tar.extractfile(f'cw104a-local-qualification/coding/live-evidence/{tag}/summary.json'));run=raw['run'];scopes={}
     if run['status']!='finished':raise RuntimeError('Attempt not finished: '+tag)
     for scope in ['bootstrap','coding','session']:
      rows=[r for r in raw['captures'] if scope=='session' or r['stage']==scope];missing=sum(not r.get('usage') for r in rows)
      scopes[scope]={'dispatches':len(rows),'missing_usage_records':missing,'outcomes':dict(collections.Counter(r['outcome'] for r in rows)),'known_input_tokens':sum((r.get('usage') or {}).get('prompt_tokens',0) for r in rows),'known_output_tokens':sum((r.get('usage') or {}).get('completion_tokens',0) for r in rows),'complete_usage':missing==0}
     runs[tag]={'status':run['status'],'acceptance_passed':run['acceptance_passed'],'bootstrap_seconds':run['bootstrap_seconds'],'coding_seconds':run['task_seconds'],'scopes':scopes,'initial_fixture_hashes':run['initial_hashes']}
 pairs=[];totals={f'{arm}_{metric}':0 for arm in ['m1','m2'] for metric in ['seconds','input']}
 for family in ['f1','f3']:
  for rep in [1,2,3]:
   a,b=[runs[f'{family}-32k-v2-r{rep}-{arm}'] for arm in ['m1','m2']];clean=all(r['scopes']['coding']['complete_usage'] and set(r['scopes']['coding']['outcomes'])=={'completed'} for r in [a,b])
   pairs.append({'family':family,'rep':rep,'clean_transport':clean,'both_acceptance':a['acceptance_passed'] and b['acceptance_passed'],'m2_minus_m1_seconds':b['coding_seconds']-a['coding_seconds'],'m2_minus_m1_input':b['scopes']['coding']['known_input_tokens']-a['scopes']['coding']['known_input_tokens'] if clean else 'NOT_AVAILABLE'})
   if clean:
    for arm,r in [('m1',a),('m2',b)]:totals[arm+'_seconds']+=r['coding_seconds'];totals[arm+'_input']+=r['scopes']['coding']['known_input_tokens']
 return {'runs':runs,'pairs':pairs,'clean_transport_totals':totals,'usage_note':'Known token counts are lower bounds when missing records exist. Historical artifacts have not been overwritten.'}

if __name__=='__main__':
 parser=argparse.ArgumentParser();parser.add_argument('archive',type=Path);args=parser.parse_args();actual=recover(args.archive);expected=json.loads((BASE/'.audit/cw104b/recovered-results.json').read_text())
 if actual!=expected:raise SystemExit('Recovered aggregate differs from committed comparison')
 print(json.dumps({'original_hashes_verified':len(json.loads((BASE/'.audit/cw104b/original-file-sha256.json').read_text())),'attempts':len(actual['runs']),'pairs':len(actual['pairs']),'aggregate_exact_match':True,'clean_transport_totals':actual['clean_transport_totals']}))
