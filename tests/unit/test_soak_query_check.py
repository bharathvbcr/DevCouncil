"""Executable receipt-admission regressions for the actual soak IPC helper."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class SoakQueryAdmission(unittest.TestCase):
    def check(self, mode, result, expected, ok=True):
        source = (ROOT / 'rust/tools/soak.sh').read_text()
        start = source.index('  ask() {')
        end = source.index('\n  for i in $(seq 1 "$CYCLES"); do', start)
        program = source[start:end] + '''
probe() { printf '%s\\n' "$TEST_REPLY"; }
IPC_PROBE=probe
ENDPOINT=unused
ask '{}' "$TEST_MODE" '_soak_1' 'main.py'
'''
        process = subprocess.run(
            ['bash', '-c', program],
            env={**os.environ, 'TOOLS': str(ROOT / 'rust/tools'),
                 'TEST_REPLY': json.dumps({'ok': ok, 'result': result}),
                 'TEST_MODE': mode},
            capture_output=True, text=True, timeout=5,
        )
        self.assertEqual(process.returncode, expected, process.stderr)

    @staticmethod
    def result(items=(), **changes):
        return {'items': list(items), 'total': len(items), 'shown': len(items),
                'hidden': 0, 'truncated': False, 'resolution': 'Available', **changes}

    def test_ok_envelope_without_the_inserted_symbol_is_not_visibility(self):
        self.check('present', self.result(), 2)

    def test_another_file_or_prefix_match_is_not_the_inserted_symbol(self):
        for item in [
            {'symbol_name': '_soak_1', 'file_path': 'other.py'},
            {'symbol_name': '_soak_10', 'file_path': 'main.py'},
        ]:
            with self.subTest(item=item):
                self.check('present', self.result([item]), 2)

    def test_exact_symbol_and_file_is_positive_evidence(self):
        self.check('present', self.result([
            {'symbol_name': '_soak_1', 'file_path': 'main.py'}]), 0)

    def test_removed_symbol_must_really_disappear(self):
        self.check('absent', self.result([
            {'symbol_name': '_soak_1', 'file_path': 'main.py'}]), 2)
        self.check('absent', self.result(), 0)

    def test_capped_or_inconsistent_results_cannot_prove_absence(self):
        for changes in [{'truncated': True}, {'hidden': 1, 'total': 1},
                        {'total': 1}, {'shown': 1}]:
            with self.subTest(changes=changes):
                self.check('absent', self.result(**changes), 2)

    def test_unavailable_or_unknown_resolution_cannot_prove_absence(self):
        for resolution in ['Unavailable', None, 'Unknown']:
            with self.subTest(resolution=resolution):
                self.check('absent', self.result(resolution=resolution), 1)
                self.check('present', self.result([
                    {'symbol_name': '_soak_1', 'file_path': 'main.py'}],
                    resolution=resolution), 1)

    def test_malformed_results_and_server_errors_fail_closed(self):
        for result in [None, {}, {'items': None}]:
            with self.subTest(result=result):
                self.check('present', result, 1)
        self.check('present', self.result(), 1, ok=False)

    def test_standalone_uses_the_same_observation_contract(self):
        for mode, result, expected in [
            ('present', self.result(), 2),
            ('absent', self.result(), 0),
            ('absent', self.result(truncated=True), 2),
        ]:
            with self.subTest(mode=mode, result=result):
                process = subprocess.run(
                    ['perl', str(ROOT / 'rust/tools/soak-query-check.pl'),
                     'cli', mode, '_soak_1', 'main.py'],
                    input=json.dumps(result), text=True, capture_output=True,
                    timeout=5,
                )
                self.assertEqual(process.returncode, expected, process.stderr)

    def test_absent_mutations_stop_at_the_convergence_deadline(self):
        source = (ROOT / 'rust/tools/soak.sh').read_text()
        start = source.index('  ask() {')
        end = source.index('\n  for i in $(seq 1 "$CYCLES"); do', start)
        program = source[start:end] + '''
probe() { printf '%s\\n' "$TEST_REPLY"; }
IPC_PROBE=probe
ENDPOINT=unused
TARGET_RELATIVE=main.py
SOAK_SETTLE_SECONDS=1
await_symbol '_soak_1' present
'''
        process = subprocess.run(
            ['bash', '-c', program],
            env={**os.environ, 'TOOLS': str(ROOT / 'rust/tools'),
                 'TEST_REPLY': json.dumps({'ok': True, 'result': self.result()})},
            capture_output=True, text=True, timeout=4,
        )
        self.assertEqual(process.returncode, 1, process.stderr)
        self.assertIn('not verified present and fresh within 1 seconds', process.stdout)

    def test_freshness_must_be_verified(self):
        self.check('fresh', {'is_fresh': False}, 2)
        self.check('fresh', {'is_fresh': True}, 0)


class SoakCsvAdmission(unittest.TestCase):
    def test_all_csv_writes_propagate_failure(self):
        source = (ROOT / 'rust/tools/soak.sh').read_text()
        header_start = source.index('echo "cycle,rss_bytes,db_bytes"')
        header_end = source.index('\nFAILS=0', header_start)
        blocks = [('header', source[header_start:header_end])]
        for name, marker in [('daemon', '    echo "$i,$(( RSS * 1024 )),'),
                             ('build', '    echo "$i,$RSS,')]:
            start = source.index(marker)
            end = source.index('\n    if [ $((i % 10))', start)
            blocks.append((name, source[start:end]))
        for name, block in blocks:
            for invalid in [False, True]:
                with self.subTest(write=name, directory=invalid):
                    with tempfile.TemporaryDirectory(prefix='soak-csv-') as directory:
                        csv = Path(directory) if invalid else Path(directory) / 'samples.csv'
                        process = subprocess.run(
                            ['bash', '-c', 'FAILS=0; RSS=123; db_bytes() { echo 456; }; '
                             'for i in 1; do\n' + block + '\ndone\nexit "$FAILS"'],
                            env={**os.environ, 'CSV': str(csv)},
                            capture_output=True, text=True, timeout=5,
                        )
                        self.assertEqual(process.returncode, int(invalid),
                                         process.stdout + process.stderr)
                        if invalid:
                            self.assertIn('SOAK FAIL:', process.stdout + process.stderr)
                        else:
                            expected = {'header': 'cycle,rss_bytes,db_bytes\n',
                                        'daemon': '1,125952,456\n',
                                        'build': '1,123,456\n'}[name]
                            self.assertEqual(csv.read_text(), expected)


if __name__ == '__main__':
    unittest.main()
