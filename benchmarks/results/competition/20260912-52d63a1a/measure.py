"""Task-local command recorder; uses the repository's peak-RSS measurement helper."""
import json
import os
from pathlib import Path
import signal
import subprocess
import time

OUT = Path(__file__).resolve().parent
META = json.loads((OUT / 'provenance.json').read_text())
SCRATCH = Path(META['scratch'])
BINARY = META['devmap_binary']

def measure(label, argv, cwd, timeout=300):
    stem = OUT / label
    env = dict(os.environ)
    env['GITNEXUS_HOME'] = str(SCRATCH / 'gitnexus-home')
    env['PEAK_RSS_STDOUT_FILE'] = str(stem) + '.stdout'
    start = time.perf_counter()
    proc = subprocess.Popen(['bash', '-c', '. "$1"; shift; peak_rss_bytes "$@"', '_', str(OUT / 'peak_rss.sh'), str(stem) + '.stderr', *argv], cwd=cwd, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, start_new_session=True)
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
    try:
        rss = int(stdout.strip()) if proc.returncode == 0 and not timed_out else None
    except ValueError:
        rss = None
    result = {'label': label, 'argv': argv, 'cwd': str(cwd), 'seconds': elapsed, 'rss_bytes': rss, 'exit_code': proc.returncode, 'timed_out': timed_out, 'measurement_ok': proc.returncode == 0 and rss is not None and rss > 0 and not timed_out, 'wrapper_stderr': stderr, 'stdout_file': stem.name+'.stdout', 'stderr_file': stem.name+'.stderr', 'load_average': os.getloadavg()}
    with (OUT / 'measurements.jsonl').open('a') as stream:
        stream.write(json.dumps(result)+'\n')
    print(json.dumps({k:result[k] for k in ['label','seconds','rss_bytes','exit_code','measurement_ok']}), flush=True)
    return result

if __name__ == '__main__':
    measure('pilot-devmap-cold', [BINARY, '--db', str(SCRATCH/'pilot.sqlite'), '--progress', 'never', '--json', 'build', str(SCRATCH/'devmap-corpus')], SCRATCH/'devmap-corpus')
    measure('pilot-gitnexus-cold', ['gitnexus', 'analyze', str(SCRATCH/'gitnexus-corpus'), '--index-only', '--name', 'devcouncil-bench-52d63a1a'], SCRATCH/'gitnexus-corpus')
