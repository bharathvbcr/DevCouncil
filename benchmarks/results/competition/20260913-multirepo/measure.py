"""Task-local command recorder; uses the repository's peak-RSS measurement helper."""
import json
import os
from pathlib import Path
import signal
import subprocess
import time
import threading

OUT = Path(__file__).resolve().parent
META = json.loads((OUT / 'plan.json').read_text())
SCRATCH = Path(META['scratch'])
BINARY = META['devmap_binary']

def measure(label, argv, cwd, timeout=300, extra_env=None, resident_pids=()):
    stem = OUT / label
    env = dict(os.environ)
    env.update(extra_env or {})
    env['GITNEXUS_HOME'] = str(SCRATCH / 'gitnexus-home')
    env['PEAK_RSS_STDOUT_FILE'] = str(stem) + '.stdout'
    start = time.perf_counter()
    proc = subprocess.Popen(['bash', '-c', '. "$1"; shift; peak_rss_bytes "$@"', '_', str(OUT / 'peak_rss.sh'), str(stem) + '.stderr', *argv], cwd=cwd, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, start_new_session=True)
    # Sampling includes supervised workers that BSD time's rusage omits.
    # Aggregate RSS double-counts shared pages and is a sampled lower bound.
    samples=[]
    stop=threading.Event()
    known={proc.pid, *resident_pids}
    def sample():
        while not stop.is_set():
            try:
                rows=subprocess.check_output(['ps','-axo','pid=,ppid=,rss='],text=True)
                pop={}
                for row in rows.splitlines():
                    pid,parent,rss=map(int,row.split());pop[pid]=(parent,rss*1024)
                before=-1
                while before!=len(known):
                    before=len(known)
                    known.update(pid for pid,(parent,_) in pop.items() if parent in known)
                samples.append({'t':time.perf_counter()-start,'rss':sum(pop[pid][1] for pid in known if pid in pop),'processes':len(known.intersection(pop))})
            except (OSError,ValueError,subprocess.SubprocessError):
                pass
            stop.wait(.05)
    sampler=threading.Thread(target=sample,daemon=True);sampler.start()
    timed_out = False
    try:
        stdout, stderr = proc.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        timed_out = True
        os.killpg(proc.pid, signal.SIGTERM)
        try:
            stdout, stderr = proc.communicate(timeout=5)
        except subprocess.TimeoutExpired:
            os.killpg(proc.pid, signal.SIGKILL)
            stdout, stderr = proc.communicate(timeout=5)
    elapsed = time.perf_counter() - start
    stop.set();sampler.join(timeout=2)
    try:
        rss = int(stdout.strip()) if proc.returncode == 0 and not timed_out else None
    except ValueError:
        rss = None
    result = {'label': label, 'argv': argv, 'cwd': str(cwd), 'seconds': elapsed, 'rss_bytes': rss, 'exit_code': proc.returncode, 'timed_out': timed_out, 'measurement_ok': proc.returncode == 0 and rss is not None and rss > 0 and not timed_out, 'wrapper_stderr': stderr, 'stdout_file': stem.name+'.stdout', 'stderr_file': stem.name+'.stderr', 'load_average': os.getloadavg(), 'sampled_tree_peak_rss_bytes': max((s['rss'] for s in samples),default=None), 'rss_samples':len(samples), 'resident_pids':list(resident_pids)}
    with (OUT / 'measurements.jsonl').open('a') as stream:
        stream.write(json.dumps(result)+'\n')
    print(json.dumps({k:result[k] for k in ['label','seconds','rss_bytes','exit_code','measurement_ok']}), flush=True)
    return result

