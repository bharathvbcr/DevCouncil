"""Measured CLI workload for task 52d63a1a; run after snapshot preparation."""
import hashlib
import json
from pathlib import Path
import shutil
import sqlite3

from measure import OUT, SCRATCH, BINARY, measure

TARGETS = {
    'db_size_gate_bytes': 'rust/devmap-extract/src/model.rs',
    'loadBounded': 'backend/go_orchestrator/repomap/repomap.go',
    'describe_corpus': 'benchmarks/map_bench.py',
}
DB = SCRATCH / 'cold-3.sqlite'
CHECKS = []

def dm(*args):
    return [BINARY, '--db', str(DB), '--progress', 'never', '--json', *args]

def gn(*args):
    return ['gitnexus', *args, '-r', str(SCRATCH/'gitnexus-corpus')]

def build(tool):
    root = SCRATCH/(tool+'-corpus')
    return dm('build',str(root)) if tool=='devmap' else ['gitnexus','analyze',str(root),'--index-only','--name','devcouncil-bench-52d63a1a']

def size(path):
    if path.is_file(): return path.stat().st_size
    return sum(p.stat().st_size for p in path.rglob('*') if p.is_file() and not p.is_symlink())

def body(result):
    return json.loads((OUT/result['stdout_file']).read_text())

def check(result, ok, detail):
    CHECKS.append({'label':result['label'], 'ok':bool(ok), 'detail':detail})
    (OUT/'checks.json').write_text(json.dumps(CHECKS,indent=2)+'\n')

def measured(label,tool,argv,timeout=300):
    result=measure(label,argv,SCRATCH/(tool+'-corpus'),timeout)
    if not result['measurement_ok']:
        raise RuntimeError('Measurement failed: '+label)
    return result

# Cold means empty application index; OS filesystem caches are not evicted.
# Alternate tool order to reduce fixed-order bias. Pilot samples are excluded.
for iteration in range(1,4):
    DB=SCRATCH/f'cold-{iteration}.sqlite'
    if DB.exists(): raise RuntimeError('Fresh store already exists: '+str(DB))
    index=SCRATCH/'gitnexus-corpus/.gitnexus'
    if index.exists(): index.rename(SCRATCH/f'gitnexus-previous-{iteration}')
    for tool in (['devmap','gitnexus'] if iteration%2 else ['gitnexus','devmap']):
        result=measured(f'cold-{tool}-{iteration}',tool,build(tool))
        if tool=='devmap':
            p=body(result); check(result,p.get('files_indexed',0)>0 and p.get('symbols',0)>0,p)
        else:
            p=json.loads((index/'meta.json').read_text()); check(result,p['stats']['files']>0 and p['stats']['nodes']>0,{k:v for k,v in p.items() if k not in ['fileHashes','cacheKeys']})
        with (OUT/'storage.jsonl').open('a') as stream:
            stream.write(json.dumps({'stage':'cold','tool':tool,'iteration':iteration,'bytes':size(DB if tool=='devmap' else index)})+'\n')

for iteration in range(1,6):
    for tool in (['gitnexus','devmap'] if iteration%2 else ['devmap','gitnexus']):
        result=measured(f'warm-{tool}-{iteration}',tool,build(tool))
        text=(OUT/result['stdout_file']).read_text()
        ok=body(result).get('file_progress',{}).get('delta',{}).get('changed')==0 if tool=='devmap' else 'up to date' in text.lower()
        check(result,ok,{'unchanged_output':text[:1500]})

# Replace one tracked file's bytes. A newly named real definition proves refresh.
probe='benchmarks/tasks.py'
original=(SCRATCH/'devmap-corpus'/probe).read_bytes()
assert original==(SCRATCH/'gitnexus-corpus'/probe).read_bytes()
for iteration in range(1,4):
    symbol=f'devmap_competition_revision_{iteration}'
    for tool in ['devmap','gitnexus']:
        (SCRATCH/(tool+'-corpus')/probe).write_bytes(original+f'\n\ndef {symbol}():\n    return {iteration}\n'.encode())
    for tool in (['devmap','gitnexus'] if iteration%2 else ['gitnexus','devmap']):
        measured(f'edit-{tool}-{iteration}',tool,build(tool))
        argv=dm('search',symbol) if tool=='devmap' else gn('context',symbol,'-f',probe)
        result=measured(f'validate-edit-{tool}-{iteration}',tool,argv,60)
        p=body(result)
        ok=any(x.get('symbol_name')==symbol for x in p.get('items',[])) if tool=='devmap' else p.get('status')=='found' and p.get('symbol',{}).get('name')==symbol
        check(result,ok,p)
    with (OUT/'storage.jsonl').open('a') as stream:
        for tool in ['devmap','gitnexus']:
            stream.write(json.dumps({'stage':'edit','tool':tool,'iteration':iteration,'bytes':size(DB if tool=='devmap' else SCRATCH/'gitnexus-corpus/.gitnexus')})+'\n')

for tool in ['devmap','gitnexus']:
    (SCRATCH/(tool+'-corpus')/probe).write_bytes(original)
    measured(f'restore-{tool}',tool,build(tool))

# Named symbols and direct callers from real Rust, Go, and Python source.
for iteration in range(1,6):
    for name,path in TARGETS.items():
        for tool in (['devmap','gitnexus'] if iteration%2 else ['gitnexus','devmap']):
            argv=dm('explore',name,'--budget','8000') if tool=='devmap' else gn('context',name,'-f',path,'-l','100','--content')
            result=measured(f'context-{tool}-{name}-{iteration}',tool,argv,60)
            p=body(result)
            # Detailed shape/caller assertions are derived from retained JSON below.
            check(result,name in json.dumps(p) and not p.get('error'),p)
            argv=dm('impact',name,'--depth','1','--budget','8000') if tool=='devmap' else gn('impact',name,'-f',path,'--depth','1','--include-tests','-l','100')
            result=measured(f'impact-{tool}-{name}-{iteration}',tool,argv,60)
            p=body(result); check(result,not p.get('error'),p)
        result=measure(f'search-ripgrep-{name}-{iteration}',['rg','--json','-n','-F',name,'.'],SCRATCH/'devmap-corpus',60)
        lines=[json.loads(s) for s in (OUT/result['stdout_file']).read_text().splitlines() if s]
        matches=[s for s in lines if s.get('type')=='match']
        check(result,result['measurement_ok'] and any(s['data']['path']['text'].lstrip('./')==path for s in matches),{'matching_lines':len(matches),'expected_path':path})

for tool in ['devmap','gitnexus']:
    target='devmap_competition_no_such_symbol_52d63a1a'
    result=measured('missing-'+tool,tool,dm('search',target) if tool=='devmap' else gn('context',target),60)
    check(result,not body(result).get('items') if tool=='devmap' else body(result).get('status')!='found',body(result))

# Persist exact post-run coverage and verify the entire tracked corpus is unchanged.
manifest=json.loads((OUT/'corpus-manifest.json').read_text())
for tool in ['devmap','gitnexus']:
    root=SCRATCH/(tool+'-corpus')
    changed=[x['path'] for x in manifest if hashlib.sha256((root/x['path']).read_bytes()).hexdigest()!=x['sha256']]
    CHECKS.append({'label':'corpus-restored-'+tool,'ok':not changed,'changed':changed})
with sqlite3.connect(f'file:{DB}?mode=ro',uri=True) as conn:
    coverage=[r[0] for r in conn.execute('SELECT p.path FROM generation_files f JOIN paths p ON p.id=f.file_id WHERE f.generation_id=(SELECT max(id) FROM generations) ORDER BY p.path')]
(OUT/'devmap-indexed-files.json').write_text(json.dumps(coverage,indent=2)+'\n')
meta=json.loads((SCRATCH/'gitnexus-corpus/.gitnexus/meta.json').read_text())
(OUT/'gitnexus-index-meta.json').write_text(json.dumps(meta,indent=2)+'\n')
(OUT/'checks.json').write_text(json.dumps(CHECKS,indent=2)+'\n')
print('DONE:',len(CHECKS),'checks;',sum(not x['ok'] for x in CHECKS),'failed',flush=True)
