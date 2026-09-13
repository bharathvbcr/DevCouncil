"""Prepare fresh corpus clones for the build-48cd3c7 rerun, reusing verified local tools."""
import datetime
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess

OUT = Path(__file__).resolve().parent
REPO = OUT.parents[3]
PRIOR = OUT.parent / '20260913-v0.2.1'
SCRATCH = REPO / '.devcouncil/benchmarks' / OUT.name

def run(argv, cwd=REPO):
    return subprocess.check_output(argv, cwd=cwd, text=True, timeout=180).strip()

def write(name, value):
    (OUT / name).write_text(json.dumps(value, indent=2) + '\n')

def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def main():
    plan = json.loads((PRIOR / 'plan.json').read_text())
    assets = json.loads((PRIOR / 'installed-binaries.json').read_text())
    for asset in assets:
        assert sha(Path(asset['executable'])) == asset['binary_sha256'], asset['repo']
    paths = json.loads((PRIOR / 'tool-paths.json').read_text())
    python = Path(plan['python_package']['environment']) / 'bin/python'
    freeze = run(['uv', 'pip', 'freeze', '--python', str(python)])
    assert freeze == (PRIOR / 'graphify-installed-requirements.txt').read_text().strip()
    assert run(['gitnexus', '--version']) == '1.6.9'
    assert run(['rg', '--version']).splitlines()[0] == 'ripgrep 15.1.0'
    installed = Path(shutil.which('devmap'))
    identity = json.loads(run([str(installed), 'paths', '--json']))
    assert identity['version'] == '0.2.1' and not identity['build']['dirty']
    revision = run(['git', 'rev-parse', identity['build']['git']])
    # The point of this rerun is that the installed executable moved on from the
    # one the previous report measured. Refuse to repeat that report's numbers.
    previous = json.loads((PRIOR / 'provenance.json').read_text())
    assert sha(installed) != previous['devmap_sha256'], 'Installed binary is the one already reported'
    assert sha(Path(previous['devmap_binary'])) == previous['devmap_sha256'], 'Preserved prior binary altered'
    SCRATCH.mkdir(parents=True, exist_ok=False)
    binary = SCRATCH / 'devmap'
    shutil.copy2(installed, binary)
    assert sha(binary) == sha(installed)
    plan.update(status='prepared', scratch=str(SCRATCH), devmap_binary=str(binary),
                comparison_baseline='../20260913-v0.2.1', devmap_version='0.2.1',
                prepared_utc=datetime.datetime.now(datetime.timezone.utc).isoformat())
    for key in ['completed_at_utc', 'competitive_timing_samples', 'report']:
        plan.pop(key, None)
    write('plan.json', plan)
    write('tool-paths.json', paths)
    write('installed-binaries.json', assets)
    copied = ['measure.py', 'peak_rss.sh', 'run_expanded.py', 'run_gortex.py',
              'gortex_cold.py', 'final_checks.py', 'gortex_followup.py',
              'audit_results.py', 'summarize.py', 'corpus-manifest.json',
              'caller-source-evidence.json', 'graphify-installed-requirements.txt']
    for name in copied:
        shutil.copy2(PRIOR / name, OUT / name)
    write('reused-artifacts.json', [{'path': name, 'prior_sha256': sha(PRIOR / name)} for name in copied])
    env = json.loads((PRIOR / 'gortex-env.json').read_text())
    old_scratch = json.loads((PRIOR / 'plan.json').read_text())['scratch']
    env = {key: value.replace(old_scratch, str(SCRATCH)) for key, value in env.items()}
    env['GORTEX_DAEMON_SOCKET'] = '/tmp/dmbench-48cd3c7.sock'
    assert not Path(env['GORTEX_DAEMON_SOCKET']).exists()
    write('gortex-env.json', env)
    config = Path(env['XDG_CONFIG_HOME']) / 'gortex/config.yaml'
    config.parent.mkdir(parents=True)
    config.write_text('repos:\n  - path: ' + str(SCRATCH / 'gortex-corpus') + '\n    name: devcouncil-bench\n')
    manifest = json.loads((OUT / 'corpus-manifest.json').read_text())
    verification = []
    for tool in ['devmap', 'graphify', 'gitnexus', 'codegraph', 'cbm', 'gortex']:
        root = SCRATCH / (tool + '-corpus')
        run(['git', 'clone', '--quiet', '--no-checkout', '--no-hardlinks', str(REPO), str(root)])
        run(['git', 'checkout', '--quiet', '--detach', plan['corpus_commit']], root)
        tracked = set(run(['git', 'ls-files', '-z'], root).rstrip('\0').split('\0'))
        assert tracked == {item['path'] for item in manifest}
        for item in manifest:
            assert sha(root / item['path']) == item['sha256'], (tool, item['path'])
        verification.append({'tool': tool, 'files': len(manifest), 'mismatches': []})
        print('Verified frozen corpus:', tool, len(manifest), flush=True)
    write('corpus-before-verification.json', verification)
    write('provenance.json', {
        'task_id': '52d63a1a-266d-4a5a-8a09-9b8458a98334', 'repo': str(REPO),
        'corpus_commit': plan['corpus_commit'], 'devmap_source_commit': revision,
        'devmap_build': identity['build'], 'devmap_version': run([str(binary), '--version']),
        'devmap_binary': str(binary), 'devmap_sha256': sha(binary),
        'previous_report_build': previous['devmap_build']['id'],
        'previous_report_source_commit': previous['devmap_source_commit'],
        'commits_since_previous_report': int(run(['git', 'rev-list', '--count',
                                                  previous['devmap_source_commit'] + '..' + revision])),
        'source_status_before': run(['git', 'status', '--short']),
        'platform': platform.platform(), 'machine': run(['sysctl', '-n', 'machdep.cpu.brand_string']),
        'memory_bytes': int(run(['sysctl', '-n', 'hw.memsize'])), 'cpu_count': os.cpu_count(),
        'corpus_files': len(manifest), 'corpus_bytes': sum((SCRATCH / 'devmap-corpus' / i['path']).stat().st_size for i in manifest),
        'gitnexus_version': run(['gitnexus', '--version']), 'rg_version': run(['rg', '--version']).splitlines()[0],
        'prepared_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
        'tool_setup': 'Reused verified prior local competitor installations; all index/config/corpus state is new.'
    })
    print('PREPARED', OUT, flush=True)

if __name__ == '__main__':
    main()
