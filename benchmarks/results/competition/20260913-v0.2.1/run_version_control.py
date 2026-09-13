"""Alternate preserved DevMap versions after the competitor campaign, on one corpus."""
import hashlib
import json
from pathlib import Path
import statistics
import run_expanded as native
from measure import OUT, SCRATCH, BINARY, measure
from audit_results import EXPECTED, callers, found

PRIOR = OUT.parent / '20260912-expanded'
OLD = json.loads((PRIOR / 'provenance.json').read_text())
CURRENT = json.loads((OUT / 'provenance.json').read_text())
VERSIONS = {'0.2.0': OLD['devmap_binary'], '0.2.1': BINARY}
ROOT = native.root('devmap')

def activate(version, iteration=3):
    native.BINARY = VERSIONS[version]
    native.DB = SCRATCH / f'version-control-{version}-cold-{iteration}.sqlite'

def run(version, stage, iteration, argv, target=None):
    label = f'version-control-{version}-{stage}-' + (target + '-' if target else '') + str(iteration)
    result = measure(label, argv, ROOT, 300, native.ENV)
    assert result['measurement_ok'], label
    return {'version': version, 'stage': stage, 'iteration': iteration, 'target': target, **result}

def order(iteration):
    return list(VERSIONS) if iteration % 2 else list(reversed(VERSIONS))

def main():
    for version, provenance in [('0.2.0', OLD), ('0.2.1', CURRENT)]:
        assert hashlib.sha256(Path(VERSIONS[version]).read_bytes()).hexdigest() == provenance['devmap_sha256']
    native.validate_snapshot('devmap')
    rows = []
    for iteration in range(1, 4):
        for version in order(iteration):
            activate(version, iteration)
            assert not native.DB.exists(), 'Version-control cold database already exists'
            row = run(version, 'cold', iteration, native.build('devmap', True))
            row['store_bytes'] = native.DB.stat().st_size
            rows.append(row)
    for iteration in range(1, 6):
        for version in order(iteration):
            activate(version)
            rows.append(run(version, 'warm', iteration, native.build('devmap')))
    probe = ROOT / 'benchmarks/tasks.py'
    original = probe.read_bytes()
    try:
        for iteration in range(1, 4):
            name = f'devmap_competition_revision_{iteration}'
            probe.write_bytes(original + f'\n\ndef {name}():\n    return {iteration}\n'.encode())
            for version in order(iteration):
                activate(version)
                rows.append(run(version, 'edit', iteration, native.build('devmap')))
                control = run(version, 'check-edit', iteration, native.query('devmap', name, 'benchmarks/tasks.py'))
                answer = json.loads((OUT / control['stdout_file']).read_text())
                assert any(x['symbol_name'] == name and x['file_path'] == 'benchmarks/tasks.py' for x in answer['items'])
    finally:
        probe.write_bytes(original)
    for version in VERSIONS:
        activate(version)
        run(version, 'restore', 1, native.build('devmap'))
        result = run(version, 'check-removed', 1, native.query('devmap', 'devmap_competition_revision_3', 'benchmarks/tasks.py'))
        answer = json.loads((OUT / result['stdout_file']).read_text())
        assert answer['resolution'] == 'Available' and not answer['items']
    answers = []
    for iteration in range(1, 6):
        for name, path in native.TARGETS.items():
            for version in order(iteration):
                activate(version)
                for stage in ['search', 'callers']:
                    result = run(version, stage, iteration, native.query('devmap', name, path, stage), name)
                    rows.append(result)
                    response = (OUT / result['stdout_file']).read_text()
                    if stage == 'search':
                        answers.append({'version': version, 'stage': stage, 'target': name, 'iteration': iteration,
                                        'exact_definition': found('devmap', response, name, path)})
                    else:
                        actual = callers('devmap', response, name)
                        answers.append({'version': version, 'stage': stage, 'target': name, 'iteration': iteration,
                                        'matched': sorted(actual & EXPECTED[name]), 'missing': sorted(EXPECTED[name] - actual),
                                        'additional_unjudged': sorted(actual - EXPECTED[name])})
    groups = []
    for version in VERSIONS:
        for stage, target in [('cold', None), ('warm', None), ('edit', None)] + [(s,t) for s in ['search','callers'] for t in native.TARGETS]:
            selected = [r for r in rows if (r['version'], r['stage'], r['target']) == (version, stage, target)]
            seconds = [r['seconds'] for r in selected]
            groups.append({'version': version, 'stage': stage, 'target': target, 'samples': len(selected),
                           'seconds': {'median': statistics.median(seconds), 'min': min(seconds), 'max': max(seconds)},
                           'sampled_tree_peak_bytes': statistics.median(r['sampled_tree_peak_rss_bytes'] for r in selected),
                           'store_bytes': statistics.median(r['store_bytes'] for r in selected) if stage == 'cold' else None})
    (OUT / 'version-control.json').write_text(json.dumps({'protocol': 'Same corpus, independent stores, alternating version order; no simultaneous timing processes. Supplemental to the 261 competitor samples.',
        'timing_samples': len(rows), 'versions': VERSIONS, 'groups': groups, 'answers': answers,
        'corpus_verification': native.validate_snapshot('devmap')}, indent=2) + '\n')
    print('VERSION CONTROL COMPLETE', flush=True)

if __name__ == '__main__':
    main()
