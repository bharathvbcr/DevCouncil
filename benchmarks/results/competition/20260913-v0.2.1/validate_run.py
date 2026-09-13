"""Verify campaign completeness, evidence integrity, links, and daemon cleanup."""
import datetime
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
from urllib.parse import unquote, urlsplit

OUT = Path(__file__).resolve().parent
REPO = OUT.parents[3]
PRIOR = OUT.parent / '20260912-expanded'

def data(path):
    return json.loads(path.read_text())

def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def main():
    plan = data(OUT / 'plan.json')
    scratch = Path(plan['scratch'])
    rows = list(map(json.loads, (OUT / 'measurements.jsonl').read_text().splitlines()))
    assert len(rows) == len({r['label'] for r in rows}), 'Duplicate labels'
    summary = data(OUT / 'summary.json')
    assert len(summary) == 57 and sum(x['attempted'] for x in summary) == 261
    for group in summary:
        expected = 3 if group['stage'] in ['cold', 'edit'] else 5
        assert group['attempted'] == group['succeeded'] == expected, group
    assert all(r['measurement_ok'] for r in rows), 'A recorded invocation failed'
    assert all(r['rss_samples'] > 0 and r['sampled_tree_peak_rss_bytes'] > 0 for r in rows)
    control = data(OUT / 'version-control.json')
    assert control['timing_samples'] == 82 and len(control['groups']) == 18
    assert len(control['answers']) == 60
    assert all(x.get('exact_definition', True) and not x.get('missing') for x in control['answers'])
    for group in control['groups']:
        assert group['samples'] == (3 if group['stage'] in ['cold', 'edit'] else 5)
    corpus = data(OUT / 'corpus-manifest.json')
    verified = []
    for tool in ['devmap', 'graphify', 'gitnexus', 'codegraph', 'cbm', 'gortex']:
        root = scratch / (tool + '-corpus')
        for item in corpus:
            assert sha(root / item['path']) == item['sha256'], (tool, item['path'])
        verified.append({'tool': tool, 'files': len(corpus), 'mismatches': []})
    for item in data(OUT / 'caller-source-evidence.json'):
        lines = (scratch / 'devmap-corpus' / item['file']).read_text().splitlines()
        assert '\n'.join(lines[item['start_line']-1:item['end_line']]).strip() == item['source'].strip()
    provenance = data(OUT / 'provenance.json')
    assert sha(Path(provenance['devmap_binary'])) == provenance['devmap_sha256']
    for asset in data(OUT / 'installed-binaries.json'):
        assert sha(Path(asset['executable'])) == asset['binary_sha256']
    prior_manifest = data(PRIOR / 'artifact-sha256.json')
    for item in prior_manifest:
        assert sha(PRIOR / item['path']) == item['sha256'], item['path']
    assert sha(PRIOR / 'artifact-sha256.json') == data(OUT / 'baseline-integrity.json')['manifest_sha256']
    pids = [data(OUT / f'gortex-cold-{i}-gates.json')['pid'] for i in (1, 2, 3)]
    pids.append(data(OUT / 'gortex-followup-stopped.json')['pid'])
    assert not data(OUT / 'gortex-followup-stopped.json')['socket_exists']
    assert not Path(data(OUT / 'gortex-env.json')['GORTEX_DAEMON_SOCKET']).exists()
    pid_checks = []
    for pid in pids:
        result = subprocess.run(['ps', '-p', str(pid), '-o', 'args='], capture_output=True, text=True, timeout=10)
        assert result.returncode in (0, 1)
        owned_alive = result.returncode == 0 and str(scratch) in result.stdout and 'gortex' in result.stdout
        assert not owned_alive, (pid, result.stdout)
        pid_checks.append({'pid': pid, 'owned_daemon_alive': owned_alive, 'pid_exists': result.returncode == 0})
    report = OUT / 'REPORT.md'
    before = sha(report)
    subprocess.run([sys.executable, str(OUT / 'write_report.py')], check=True, timeout=30)
    assert before == sha(report), 'Report regeneration changed output'
    docs = [report, OUT / 'observations.md', OUT / 'PLAN.md', OUT.parent / 'README.md',
            REPO / 'docs/devmap/README.md', REPO / 'benchmarks/README.md', REPO / 'README.md']
    checked = []
    external = 0
    for doc in docs:
        text = re.sub(r'```.*?```', '', doc.read_text(), flags=re.S)
        for raw in re.findall(r'\[[^\]\n]*\]\(([^)\n]+)\)', text):
            target = raw.strip().strip('<>')
            parts = urlsplit(target)
            if parts.scheme or parts.netloc:
                external += 1
                continue
            path = (doc.parent / unquote(parts.path)).resolve() if parts.path else doc
            assert path.exists(), (doc, target)
            if parts.fragment and path.suffix == '.md':
                headings = re.findall(r'^#+\s+(.+)$', path.read_text(), flags=re.M)
                anchors = {re.sub(r'[^\w\- ]', '', h.lower()).replace(' ', '-') for h in headings}
                assert unquote(parts.fragment) in anchors, (doc, target)
            checked.append({'from': str(doc.relative_to(REPO)), 'target': target})
    audit = data(OUT / 'audit.json')
    receipt = {
        'completed_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
        'competitive_timing_samples': 261, 'groups': 57, 'total_timed_records': len(rows),
        'alternating_version_control_samples': 82,
        'failed_invocations': [], 'control_failures': [x['label'] for x in audit['controls'] if x['passed'] is False],
        'initially_inconclusive_controls': [x['label'] for x in audit['controls'] if x['passed'] is None],
        'corpora': verified, 'source_snippets_match': True, 'preserved_binary_hashes_match': True,
        'baseline_files_unchanged': len(prior_manifest), 'daemon_checks': pid_checks,
        'socket_removed': True, 'report_regeneration_identical': True,
        'local_links_checked': len(checked), 'external_links_not_rechecked': external,
        'product_tests_not_run': 'Benchmark artifacts and documentation only; no product source changes or v0.2.1 product-test qualification claimed.'
    }
    (OUT / 'validation.json').write_text(json.dumps(receipt, indent=2) + '\n')
    manifest = []
    for path in sorted(OUT.rglob('*')):
        if not path.is_file() or path.name == 'artifact-sha256.json' or '__pycache__' in path.parts:
            continue
        manifest.append({'path': path.relative_to(OUT).as_posix(), 'bytes': path.stat().st_size, 'sha256': sha(path)})
    (OUT / 'artifact-sha256.json').write_text(json.dumps(manifest, indent=2) + '\n')
    for item in data(OUT / 'artifact-sha256.json'):
        assert sha(OUT / item['path']) == item['sha256']
    print(json.dumps({'samples': 261, 'records': len(rows), 'verified_artifacts': len(manifest),
                      'local_links': len(checked), 'retained_control_failures': receipt['control_failures']}, indent=2))

if __name__ == '__main__':
    main()
