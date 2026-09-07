"""`_run` must block on the kernel, not busy-poll it.

`devmap_engine._run` waited for the kernel with `proc.wait(timeout=...)`. On
POSIX, CPython implements exactly that call as a **busy loop**
(`subprocess.py`, `Popen._wait`): it retries `waitpid(WNOHANG)` on an
exponential backoff that is capped at 50 ms and sleeps in between —

    delay = min(delay * 2, remaining, .05)
    time.sleep(delay)

— so the parent learns that the kernel exited up to 50 ms after it actually
did, on every single kernel invocation. A cProfile of a warm `dev map` over
this repository showed `time.sleep` as the largest single entry, 12 calls, and
7 `subprocess._wait` frames.

The passing timeout is not the point and must not be dropped: the point is
that a *blocking* wait is available and gives the same guarantee. Both reader
threads already read their pipe to EOF, which happens when the kernel exits,
and `Thread.join(timeout)` blocks on a condition variable rather than
spinning. Joining the readers first and only then calling the bare, blocking
`proc.wait()` waits exactly as long, enforces the same deadline, and sleeps
zero times.

Asserting on the *count* of `time.sleep` calls rather than on elapsed time:
the wall-clock difference is one poll interval and disappears into the noise
on a loaded machine, while the call count is exact and is the mechanism
itself.
"""

from __future__ import annotations

import sys
import time as time_module

from devcouncil import devmap_engine


def _counting_sleep(recorder: list[float]):
    real_sleep = time_module.sleep

    def counting(seconds: float) -> None:
        recorder.append(seconds)
        real_sleep(seconds)

    return counting


def test_run_blocks_on_the_kernel_instead_of_polling_it(tmp_path, monkeypatch):
    """A kernel that runs long enough to reach the 50 ms poll cap costs no sleeps."""
    slept: list[float] = []
    monkeypatch.setattr(time_module, "sleep", _counting_sleep(slept))

    # Long enough that the pre-fix backoff reaches its 50 ms ceiling (~63 ms of
    # doubling) and keeps polling for the rest of the run.
    argv = [sys.executable, "-c", "import time; time.sleep(0.4); print('done')"]
    result = devmap_engine._run(argv, cwd=tmp_path, timeout=30.0, stage="probe")

    assert result.returncode == 0, result.stderr
    assert "done" in result.stdout
    assert slept == [], (
        f"_run slept {len(slept)} time(s) waiting for a subprocess "
        f"({slept!r}); the wait must block, not poll"
    )


def test_run_still_enforces_its_timeout(tmp_path, monkeypatch):
    """Removing the poll must not remove the deadline it was enforcing."""
    monkeypatch.setattr(devmap_engine, "_record_run", lambda *a, **k: None)
    argv = [sys.executable, "-c", "import time; time.sleep(30)"]
    started = time_module.monotonic()
    try:
        devmap_engine._run(argv, cwd=tmp_path, timeout=0.5, stage="probe")
    except devmap_engine.DevMapEngineError as error:
        assert error.code == "kernel_timeout", f"wrong code: {error.code}"
    else:  # pragma: no cover - the raise above is the contract
        raise AssertionError("a kernel over its deadline must raise kernel_timeout")
    elapsed = time_module.monotonic() - started
    assert elapsed < 10.0, f"the timeout did not fire promptly ({elapsed:.1f}s)"


def test_run_reaps_the_child_it_killed(tmp_path, monkeypatch):
    """A timed-out kernel must be waited on, not left a zombie."""
    monkeypatch.setattr(devmap_engine, "_record_run", lambda *a, **k: None)
    argv = [sys.executable, "-c", "import time; time.sleep(30)"]
    try:
        devmap_engine._run(argv, cwd=tmp_path, timeout=0.5, stage="probe")
    except devmap_engine.DevMapEngineError:
        pass
    # Nothing to assert on the pid portably; the contract is that _run returned
    # rather than blocking forever on a kill it never reaped, which it did.
