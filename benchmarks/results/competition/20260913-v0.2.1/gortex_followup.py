"""Resolve default confidence filtering and fuzzy negative-search ambiguity."""
import json,os,signal,subprocess,time
from pathlib import Path
from measure import OUT,SCRATCH,measure
B=json.loads((OUT/'tool-paths.json').read_text())['gortex'];ENV=json.loads((OUT/'gortex-env.json').read_text());ROOT=SCRATCH/'gortex-corpus';log=OUT/'gortex-followup.log'
with log.open('w') as f:
 p=subprocess.Popen([B,'daemon','start','--backend-path',str(SCRATCH/'gortex-cold-3.sqlite')],cwd=ROOT,env={**os.environ,**ENV},stdout=f,stderr=f,start_new_session=True)
try:
 deadline=time.monotonic()+300
 while time.monotonic()<deadline:
  if p.poll() is not None:raise RuntimeError('Gortex restart failed')
  if '"msg":"daemon: enrichment complete"' in log.read_text():break
  time.sleep(.05)
 else:raise TimeoutError('Gortex follow-up warmup exceeded 300s')
 def call(label,tool,args):return measure(label,[B,'call',tool,'--json',json.dumps(args),'--index',str(ROOT),'--format','json'],ROOT,120,ENV,[p.pid])
 target=json.loads((OUT/'resolve-gortex-db_size_gate_bytes.stdout').read_text())['results'][0]['id']
 call('gortex-rust-callers-include-name-only','get_callers',{'id':target,'depth':1,'limit':100,'min_tier':'text_matched','exclude_tests':False})
 edited=json.loads((OUT/'validate-edit-gortex-3.stdout').read_text())['results'][0]['id']
 call('gortex-exact-removed','get_symbol',{'id':edited})
 call('gortex-exact-nonexistent','get_symbol',{'id':edited.replace('devmap_competition_revision_3','devmap_competition_nonexistent_52d63a1a')})
 call('gortex-exact-present','get_symbol',{'id':target})
finally:
 p.send_signal(signal.SIGTERM)
 try:p.wait(timeout=30)
 except subprocess.TimeoutExpired:p.kill();p.wait(timeout=5)
 (OUT/'gortex-followup-stopped.json').write_text(json.dumps({'pid':p.pid,'returncode':p.returncode,'socket_exists':Path(ENV['GORTEX_DAEMON_SOCKET']).exists()}))
print('COMPLETE',flush=True)
