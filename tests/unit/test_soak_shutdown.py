"""Exercise the soak's actual shutdown block against owned real processes."""
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
SOURCE = Path(os.environ.get('SOAK_SCRIPT_UNDER_TEST', ROOT / 'rust/tools/soak.sh'))


class SoakShutdown(unittest.TestCase):
    def run_shutdown(self, mode, entry='normal', unidentified=False,
                     refuse_kill=False, exited_wrapper=False, delayed_session=False):
        source = SOURCE.read_text()
        helper_start = source.index('  # Every exit from here on')
        helper_end = source.index('\n  trap ', helper_start)
        shutdown_start = source.index('\n', source.index(
            'echo "SOAK FAIL: final daemon graph differs from baseline"'))
        shutdown_start = source.index('\n', shutdown_start + 1)
        shutdown_end = source.index('\n  if [ "$FAILS" -eq 0 ]; then', shutdown_start)
        helpers = source[helper_start:helper_end]
        shutdown = source[shutdown_start:shutdown_end]
        launch_start = source.find("perl -MPOSIX=setsid -e '")
        if launch_start >= 0:
            launch_end = source.index(' -- /usr/bin/time', launch_start)
            launcher = source[launch_start:launch_end] + ' --'
        else:
            # Isolate pre-fix children too, so a demonstrated hang cannot leak.
            launcher = "perl -MPOSIX=setsid -e 'setsid() == $$ or die; exec @ARGV; die $!;' --"
        if delayed_session:
            launcher = launcher.replace("-e '", "-e 'sleep 3; ", 1)
        with tempfile.TemporaryDirectory(prefix='soak-shutdown-') as directory:
            daemon = Path(directory) / 'daemon.py'
            daemon.write_text('''import os, signal, sys
exit_code = 7 if sys.argv[2] == 'failure' else 0
signal.signal(signal.SIGTERM, signal.SIG_IGN if sys.argv[2] == 'ignore' else lambda *_: sys.exit(exit_code))
with open(sys.argv[1], 'w') as stream:
    stream.write(str(os.getpid()))
while True:
    signal.pause()
''')
            program = helpers + '''
set -u
SOAK_SHUTDOWN_SECONDS=1
DAEMON_STOPPED=0
DAEMON_STOP_STATUS=0
ENDPOINT="$TEST_DIRECTORY/socket"
ORIG="$TEST_DIRECTORY/original"
touch "$ENDPOINT" "$ORIG"
@SESSION_LAUNCHER@ /usr/bin/time -p "$TEST_PYTHON" "$TEST_DIRECTORY/daemon.py" "$TEST_DIRECTORY/pid" "$TEST_MODE" >/dev/null 2>"$TEST_DIRECTORY/time.log" &
TIME_PID=$!
printf '%s' "$TIME_PID" > "$TEST_DIRECTORY/group"
SERVE_PID=""
if [ "$TEST_DELAYED_SESSION" != 1 ]; then
  for ((tick=0; tick<100; tick++)); do
    [ ! -s "$TEST_DIRECTORY/pid" ] || break
    sleep 0.02
  done
  SERVE_PID=$(cat "$TEST_DIRECTORY/pid") || exit 90
fi
[ "$TEST_UNIDENTIFIED" != 1 ] || SERVE_PID=""
FAILS=0
'''
            program = program.replace('@SESSION_LAUNCHER@', launcher)
            if exited_wrapper:
                program += 'kill -KILL "$TIME_PID"; wait "$TIME_PID" 2>/dev/null || true\n'
            if refuse_kill:
                # Model an OS refusal while keeping real, still-running children.
                program += 'kill() { [ "$1" != "-KILL" ] || return 1; builtin kill "$@"; }\n'
            if entry == 'normal':
                program += shutdown + '\nexit "$FAILS"\n'
            else:
                program += '''
daemon_cleanup
first=$?
daemon_cleanup
second=$?
[ "$first" -eq "$second" ] || exit 91
[ ! -e "$ENDPOINT" ] && [ ! -e "$ORIG" ] || exit 92
exit "$first"
'''
            sentinel = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(30)'])
            process = subprocess.Popen(
                ['bash', '-c', program], start_new_session=True,
                env={**os.environ, 'TEST_DIRECTORY': directory,
                     'TEST_PYTHON': sys.executable, 'TEST_MODE': mode,
                     'TEST_DELAYED_SESSION': str(int(delayed_session)),
                     'TEST_UNIDENTIFIED': str(int(unidentified))},
                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
            )
            try:
                try:
                    stdout, stderr = process.communicate(timeout=6)
                except subprocess.TimeoutExpired:
                    self.fail('the real soak shutdown exceeded 6 seconds for a 1 second grace period')
                self.assertIsNone(sentinel.poll(), 'shutdown killed an unrelated process')
                owned_pid = int((Path(directory) / ('group' if delayed_session else 'pid')).read_text())
                state = subprocess.run(
                    ['ps', '-o', 'stat=', '-p', str(owned_pid)],
                    capture_output=True, text=True, timeout=2,
                ).stdout.strip()
                if refuse_kill:
                    self.assertTrue(state and not state.startswith('Z'))
                else:
                    self.assertTrue(not state or state.startswith('Z'),
                                    f'shutdown left owned process {owned_pid} running: {state}')
                return process.returncode, stdout + stderr, (Path(directory) / 'time.log').read_text()
            finally:
                # Also reap the pre-fix implementation after its expected timeout.
                group_file = Path(directory) / 'group'
                if group_file.exists():
                    try:
                        os.killpg(int(group_file.read_text()), signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                process.communicate(timeout=2)
                sentinel.terminate()
                sentinel.wait(timeout=2)

    def test_graceful_shutdown_retains_the_time_receipt(self):
        status, output, receipt = self.run_shutdown('graceful')
        self.assertEqual(status, 0, output)
        self.assertIn('real ', receipt)

    def test_sigterm_ignoring_daemon_is_bounded_and_fails_the_soak(self):
        status, output, _ = self.run_shutdown('ignore')
        self.assertEqual(status, 1, output)
        self.assertIn('SOAK FAIL: daemon shutdown', output)

    def test_exit_cleanup_escalates_and_is_idempotent(self):
        status, output, _ = self.run_shutdown('ignore', entry='cleanup')
        self.assertEqual(status, 1, output)

    def test_startup_failure_cleans_up_before_daemon_identification(self):
        status, output, _ = self.run_shutdown('ignore', entry='cleanup', unidentified=True)
        self.assertEqual(status, 1, output)

    def test_unconfirmed_forced_exit_is_bounded_and_reported(self):
        status, output, _ = self.run_shutdown('ignore', refuse_kill=True)
        self.assertEqual(status, 1, output)
        self.assertIn('could not confirm termination after SIGKILL', output)

    def test_failed_daemon_exit_cannot_count_as_a_successful_soak(self):
        status, output, _ = self.run_shutdown('failure')
        self.assertEqual(status, 1, output)
        self.assertIn('exited with status 7', output)

    def test_exited_wrapper_cannot_hide_a_reparented_daemon(self):
        status, output, _ = self.run_shutdown(
            'ignore', entry='cleanup', unidentified=True, exited_wrapper=True)
        self.assertEqual(status, 1, output)

    def test_cleanup_before_session_creation_terminates_the_launcher(self):
        status, output, _ = self.run_shutdown(
            'ignore', entry='cleanup', unidentified=True, delayed_session=True)
        self.assertEqual(status, 1, output)


if __name__ == '__main__':
    unittest.main()
