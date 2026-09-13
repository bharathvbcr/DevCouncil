"""Run the same comparison in fresh local DevCouncil snapshots and output paths."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys

HERE=Path(__file__).resolve().parent
original=json.loads((HERE/'provenance.json').read_text())
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--repo',type=Path,default=Path(original['repo']))
p.add_argument('--binary',type=Path,default=Path(original['devmap_binary']))
p.add_argument('--commit',default=original['commit'])
p.add_argument('--out',type=Path,required=True)
p.add_argument('--scratch',type=Path,required=True)
a=p.parse_args()
repo=a.repo.resolve();out=a.out.resolve();scratch=a.scratch.resolve();binary=a.binary.resolve()
if out.exists() or scratch.exists(): p.error('Output and scratch paths must both be new')
if not binary.is_file(): p.error('Supply an existing DevMap release binary with --binary')
def command(argv,cwd=None): return subprocess.check_output(argv,cwd=cwd,text=True,timeout=120).strip()
commit=command(['git','rev-parse',a.commit+'^{commit}'],repo)
out.mkdir(parents=True);scratch.mkdir(parents=True)
for tool in ['devmap','gitnexus']:
    dest=scratch/(tool+'-corpus')
    subprocess.run(['git','clone','--shared','--no-checkout','--quiet',str(repo),str(dest)],check=True,timeout=120)
    subprocess.run(['git','-c','core.hooksPath=/dev/null','checkout','--quiet','--detach',commit],cwd=dest,check=True,timeout=120)
shutil.copy2(binary,scratch/'devmap')
meta={'task_id':original['task_id'],'repo':str(repo),'commit':commit,'scratch':str(scratch),'devmap_binary':str(scratch/'devmap'),'devmap_sha256':hashlib.sha256((scratch/'devmap').read_bytes()).hexdigest(),'devmap_version':command([str(scratch/'devmap'),'--version']),'gitnexus_version':command(['gitnexus','--version']),'rg_version':command(['rg','--version']).splitlines()[0],'platform':platform.platform(),'cpu_count':os.cpu_count(),'source_status_before':command(['git','status','--porcelain'],repo),'generated_at':datetime.now(timezone.utc).isoformat()}
manifest=[]
files=subprocess.check_output(['git','ls-files','-z'],cwd=scratch/'devmap-corpus',timeout=120).split(b'\0')
for raw in files:
    if not raw: continue
    rel=os.fsdecode(raw);f=scratch/'devmap-corpus'/rel
    if f.is_file() and not f.is_symlink(): manifest.append({'path':rel,'bytes':f.stat().st_size,'sha256':hashlib.sha256(f.read_bytes()).hexdigest()})
meta['corpus_files']=len(manifest);meta['corpus_bytes']=sum(x['bytes'] for x in manifest)
(out/'provenance.json').write_text(json.dumps(meta,indent=2)+'\n');(out/'corpus-manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
for name in ['measure.py','run_comparison.py','summarize.py','audit_results.py','check_removed.py','peak_rss.sh','reproduce.py']:
    shutil.copy2(HERE/name,out/name)
for name in ['run_comparison.py','check_removed.py','summarize.py','audit_results.py']:
    subprocess.run([sys.executable,str(out/name)],check=True,timeout=1800)
print('Results:',out)
