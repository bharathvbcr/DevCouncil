"""Foreground-owned Gortex lifecycle; records query-ready and fully enriched gates."""
import json,os,signal,subprocess,sys,time
from measure import OUT,SCRATCH
B=json.loads((OUT/'tool-paths.json').read_text())['gortex']; ENV={**os.environ,**json.loads((OUT/'gortex-env.json').read_text())};i=int(sys.argv[1]);db=SCRATCH/f'gortex-cold-{i}.sqlite';log=OUT/f'gortex-cold-{i}.log'
if db.exists():raise RuntimeError('Cold database already exists')
start=time.perf_counter();stream=log.open('w');p=subprocess.Popen([B,'daemon','start','--backend-path',str(db)],cwd=SCRATCH/'gortex-corpus',env=ENV,stdout=stream,stderr=stream,start_new_session=True)
(OUT/'gortex-active-pid.txt').write_text(str(p.pid));ready=None;enriched=None;success=False
try:
 while time.perf_counter()-start<600:
  if p.poll() is not None:raise RuntimeError('Daemon exited '+str(p.returncode))
  text=log.read_text()
  if ready is None and '"msg":"daemon: graph queryable"' in text:ready=time.perf_counter()-start
  if '"msg":"daemon: enrichment complete"' in text:enriched=time.perf_counter()-start;break
  time.sleep(.05)
 if ready is None or enriched is None:raise TimeoutError('Gortex did not reach query-ready and enriched gates in 600s')
 result={'iteration':i,'pid':p.pid,'query_ready_seconds':ready,'enriched_seconds':enriched,'database':str(db),'log':log.name}
 (OUT/f'gortex-cold-{i}-gates.json').write_text(json.dumps(result,indent=2));print(json.dumps(result),flush=True);success=True
finally:
 if i!=3 or not success:
  p.send_signal(signal.SIGTERM)
  try:p.wait(timeout=30)
  except subprocess.TimeoutExpired:p.kill();p.wait(timeout=5)
 stream.close()
