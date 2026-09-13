"""Check that restored source does not leave deleted probe symbols indexed."""
import json
import os
import subprocess
from measure import OUT, SCRATCH, BINARY

env=dict(os.environ)
env['GITNEXUS_HOME']=str(SCRATCH/'gitnexus-home')
results=[]
for tool in ['devmap','gitnexus']:
    for i in [1,2,3]:
        name=f'devmap_competition_revision_{i}'
        argv=[BINARY,'--db',str(SCRATCH/'cold-3.sqlite'),'--json','search',name] if tool=='devmap' else ['gitnexus','context',name,'-r',str(SCRATCH/'gitnexus-corpus')]
        p=subprocess.run(argv,capture_output=True,text=True,env=env,timeout=60,cwd=SCRATCH/(tool+'-corpus'))
        (OUT/f'removed-{tool}-{i}.stdout').write_text(p.stdout)
        (OUT/f'removed-{tool}-{i}.stderr').write_text(p.stderr)
        if p.returncode: raise RuntimeError(f'Lookup failed: {tool} {name}: {p.stderr}')
        data=json.loads(p.stdout)
        if tool=='devmap':
            if data.get('resolution')!='Available': raise RuntimeError('DevMap lookup unavailable')
            found=any(x.get('symbol_name')==name for x in data.get('items',[]))
        elif data.get('status')=='found':
            found=True
        elif data.get('error')==f"Symbol '{name}' not found":
            found=False
        else:
            raise RuntimeError('GitNexus lookup unavailable or malformed: '+json.dumps(data))
        results.append({'tool':tool,'symbol':name,'stale_symbol_found':found,'exit_code':p.returncode,'argv':argv})
(OUT/'removed-symbol-checks.json').write_text(json.dumps(results,indent=2)+'\n')
print(json.dumps(results,indent=2))
