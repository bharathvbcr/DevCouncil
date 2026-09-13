"""Pinned local CLI comparison. Run once after setup; existing outputs are preserved."""
import hashlib,json,sys
from pathlib import Path
from measure import OUT,SCRATCH,BINARY,measure
B=json.loads((OUT/'tool-paths.json').read_text()); B.update(devmap=BINARY,gitnexus='/opt/homebrew/bin/gitnexus')
TOOLS=['devmap','graphify','gitnexus','codegraph','cbm']; B['cbm']=B['codebase-memory-mcp']
ENV={'XDG_CONFIG_HOME':str(SCRATCH/'xdg/config'),'XDG_CACHE_HOME':str(SCRATCH/'xdg/cache'),'XDG_DATA_HOME':str(SCRATCH/'xdg/data'),'CBM_CACHE_DIR':str(SCRATCH/'cbm-cache'),'DO_NOT_TRACK':'1','NO_COLOR':'1'}
TARGETS={'db_size_gate_bytes':'rust/devmap-extract/src/model.rs','loadBounded':'backend/go_orchestrator/repomap/repomap.go','describe_corpus':'benchmarks/map_bench.py'}
DB=SCRATCH/'dm-cold-3.sqlite'
def root(t):return SCRATCH/(t+'-corpus')
def dm(*a):return [BINARY,'--db',str(DB),'--progress','never','--json',*a]
def build(t,cold=False):
 r=str(root(t))
 if t=='devmap':return dm('build',r)
 if t=='gitnexus':return [B[t],'analyze',r,'--index-only','--name','devcouncil-expanded']
 if t=='graphify':return [B[t],'extract',r,'--code-only','--no-cluster']
 if t=='codegraph':return [B[t],'init' if cold else 'sync',r,*(['--yes'] if cold else [])]
 if t=='cbm':return [B[t],'cli','index_repository','--repo-path',r,'--name','devcouncil-bench','--mode','full','--persistence','false']
def indexpath(t):return {'devmap':DB,'gitnexus':root(t)/'.gitnexus','graphify':root(t)/'graphify-out','codegraph':root(t)/'.codegraph','cbm':SCRATCH/'cbm-cache'}[t]
def store(stage,t,i):
 p=indexpath(t); size=p.stat().st_size if p.is_file() else sum(x.stat().st_size for x in p.rglob('*') if x.is_file() and not x.is_symlink())
 with (OUT/'storage.jsonl').open('a') as f:f.write(json.dumps({'stage':stage,'tool':t,'iteration':i,'bytes':size})+'\n')
def run(label,t,args):return measure(label,args,root(t),600,ENV)
def query(t,name,path,kind='search'):
 if t=='devmap':return dm('search' if kind=='search' else 'explore',name,*([] if kind=='search' else ['--budget','8000']))
 if t=='gitnexus':return [B[t],'context',name,'-r',str(root(t)),'-f',path,'-l','100','--content']
 if t=='graphify':return [B[t],'explain' if kind=='search' else 'affected',name,*([] if kind=='search' else ['--relation','calls','--depth','1']),'--graph',str(indexpath(t)/'graph.json')]
 if t=='codegraph':return [B[t],'query' if kind=='search' else 'callers',name,'--path',str(root(t)),'--limit','100','--json']
 if t=='cbm':return [B[t],'cli',*(['search_graph','--name-pattern','^'+name+'$'] if kind=='search' else ['trace_path','--function-name',name,'--direction','inbound','--depth','1','--include-tests','true']),'--project','devcouncil-bench','--format','json','--limit','100']
def validate_snapshot(t):
 m=json.loads((OUT/'corpus-manifest.json').read_text());bad=[x['path'] for x in m if hashlib.sha256((root(t)/x['path']).read_bytes()).hexdigest()!=x['sha256']]
 if bad:raise RuntimeError((t,bad))
 return {'tool':t,'files':len(m),'mismatches':bad}
if __name__=='__main__':
 for t in TOOLS:validate_snapshot(t)
 for i in range(1,4):
  DB=SCRATCH/f'dm-cold-{i}.sqlite'
  for t in TOOLS:
   p=indexpath(t)
   if p.exists():
    dest=SCRATCH/f'archived-{t}-{i}'
    if dest.exists():raise RuntimeError('Run already started: '+str(dest))
    p.rename(dest)
  for t in (TOOLS if i%2 else list(reversed(TOOLS))):
   r=run(f'cold-{t}-{i}',t,build(t,True))
   if not r['measurement_ok']:raise RuntimeError('Cold indexing failed: '+t)
   store('cold',t,i)
 for i in range(1,6):
  for t in (TOOLS if i%2 else list(reversed(TOOLS))):run(f'warm-{t}-{i}',t,build(t))
 probe='benchmarks/tasks.py'; original=(root('devmap')/probe).read_bytes()
 try:
  for i in range(1,4):
   name=f'devmap_competition_revision_{i}'
   for t in (TOOLS if i%2 else list(reversed(TOOLS))):
    (root(t)/probe).write_bytes(original+f'\n\ndef {name}():\n    return {i}\n'.encode())
    run(f'edit-{t}-{i}',t,build(t));run(f'validate-edit-{t}-{i}',t,query(t,name,probe));store('edit',t,i)
 finally:
  for t in TOOLS:(root(t)/probe).write_bytes(original)
 for t in TOOLS:
  run(f'restore-{t}',t,build(t))
  for name in ['devmap_competition_revision_3','devmap_competition_nonexistent_52d63a1a']:
   run(f'negative-{t}-{name}',t,query(t,name,probe))
 for i in range(1,6):
  for name,path in TARGETS.items():
   for t in (TOOLS if i%2 else list(reversed(TOOLS))):
    for kind in ['search','callers']:run(f'{kind}-{t}-{name}-{i}',t,query(t,name,path,kind))
 (OUT/'corpus-verification.json').write_text(json.dumps([validate_snapshot(t) for t in TOOLS],indent=2))
 print('COMPLETE',flush=True)
