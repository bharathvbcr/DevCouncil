"""Install the approved, pinned benchmark tools into the isolated task directory."""
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tarfile
import urllib.request

OUT=Path(__file__).resolve().parent
PLAN=json.loads((OUT/'plan.json').read_text())
SCRATCH=Path(PLAN['scratch'])
TOOLS=SCRATCH/'tools'

def main():
    TOOLS.mkdir(parents=True,exist_ok=False)
    identities=[]
    for item in PLAN['binary_assets']:
        archive=TOOLS/item['name']
        with urllib.request.urlopen(item['url'],timeout=60) as response:
            data=response.read(item['bytes']+1)
        if len(data)!=item['bytes'] or hashlib.sha256(data).hexdigest()!=item['sha256']:
            raise RuntimeError('Release size or checksum mismatch: '+item['name'])
        archive.write_bytes(data)
        destination=TOOLS/item['repo'].split('/')[-1]
        destination.mkdir()
        with tarfile.open(archive,'r:gz') as bundle:
            bundle.extractall(destination,filter='data')
        executable=next(p for p in destination.rglob(item['repo'].split('/')[-1]) if p.is_file())
        identities.append({**item,'executable':str(executable),'binary_sha256':hashlib.sha256(executable.read_bytes()).hexdigest()})
        print('Verified and unpacked',item['repo'],item['version'],flush=True)
        (OUT/'installed-binaries.json').write_text(json.dumps(identities,indent=2)+'\n')
    environment=TOOLS/'graphify-venv'
    python=shutil.which('python3.12')
    if not python: raise RuntimeError('The preflight Python 3.12 interpreter is missing')
    subprocess.run(['uv','venv','--no-project','--python',python,str(environment)],check=True,timeout=120)
    subprocess.run(['uv','pip','install','--python',str(environment/'bin/python'),'graphifyy==0.9.59'],check=True,timeout=600)
    lock=subprocess.check_output(['uv','pip','freeze','--python',str(environment/'bin/python')],text=True,timeout=60)
    (OUT/'graphify-installed-requirements.txt').write_text(lock)
    print('Installed isolated graphifyy 0.9.59',flush=True)

if __name__=='__main__':
    main()
