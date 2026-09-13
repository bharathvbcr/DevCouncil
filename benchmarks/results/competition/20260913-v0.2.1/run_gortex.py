import json,os,subprocess,sys
from measure import OUT,SCRATCH,measure
from run_expanded import TARGETS,validate_snapshot
B=json.loads((OUT/'tool-paths.json').read_text())['gortex']; ENV=json.loads((OUT/'gortex-env.json').read_text());ROOT=SCRATCH/'gortex-corpus';PIDS=[]
def run(label,args):return measure(label,[B,*args],ROOT,600,ENV,PIDS)
def search(name):return ['query','symbol',name,'--index',str(ROOT),'--format','json','--limit','100']
def refresh():return ['call','reindex_repository','--arg','path='+str(ROOT),'--index',str(ROOT),'--format','json']
def status(label):run(label,['daemon','status'])
for i in range(1,4):
 r=measure(f'cold-gortex-{i}',[sys.executable,str(OUT/'gortex_cold.py'),str(i)],ROOT,650,ENV)
 if not r['measurement_ok']:raise RuntimeError('Gortex cold run failed')
 with (OUT/'storage.jsonl').open('a') as f:f.write(json.dumps({'stage':'cold','tool':'gortex','iteration':i,'bytes':sum(p.stat().st_size for p in SCRATCH.glob(f'gortex-cold-{i}.sqlite*') if p.is_file())})+'\n')
PIDS=[int((OUT/'gortex-active-pid.txt').read_text())]
original=(ROOT/'benchmarks/tasks.py').read_bytes()
try:
 status('gortex-initial-status')
 for i in range(1,6):run(f'warm-gortex-{i}',refresh())
 for i in range(1,4):
  name=f'devmap_competition_revision_{i}'
  (ROOT/'benchmarks/tasks.py').write_bytes(original+f'\n\ndef {name}():\n    return {i}\n'.encode())
  run(f'edit-gortex-{i}',refresh());run(f'validate-edit-gortex-{i}',search(name))
 (ROOT/'benchmarks/tasks.py').write_bytes(original);run('restore-gortex',refresh())
 for name in ['devmap_competition_revision_3','devmap_competition_nonexistent_52d63a1a']:run('negative-gortex-'+name,search(name))
 # Resolve IDs from actual search output before calling traversal endpoints.
 ids={}
 for name,path in TARGETS.items():
  r=run('resolve-gortex-'+name,search(name));data=json.loads((OUT/r['stdout_file']).read_text());matched=[n for n in data.get('results',[]) if n.get('name')==name and n.get('absolute_file_path')==str(ROOT/path)]
  if len(matched)!=1:raise RuntimeError('Gortex target resolution failed: '+name)
  ids[name]=matched[0]['id']
 for i in range(1,6):
  for name in TARGETS:
   run(f'search-gortex-{name}-{i}',search(name))
   run(f'callers-gortex-{name}-{i}',['query','callers',ids[name],'--index',str(ROOT),'--format','json','--depth','1','--limit','100'])
 status('gortex-final-status');run('gortex-final-stats',['query','stats','--index',str(ROOT),'--format','json'])
 (OUT/'gortex-corpus-verification.json').write_text(json.dumps(validate_snapshot('gortex'),indent=2))
finally:
 (ROOT/'benchmarks/tasks.py').write_bytes(original)
 r=subprocess.run([B,'daemon','stop'],cwd=ROOT,env={**os.environ,**ENV},capture_output=True,text=True,timeout=60)
 (OUT/'gortex-final-stop.txt').write_text(r.stdout+r.stderr)
 if r.returncode:raise RuntimeError('Owned daemon did not stop: '+r.stderr)
print('COMPLETE',flush=True)
