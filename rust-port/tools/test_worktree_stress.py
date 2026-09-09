"""Failure semantics for the native capacity harness's simulated editors."""
import time
import unittest
import threading
from concurrent.futures import ThreadPoolExecutor
from worktree_stress import ProbeAdmission, retry_file_operation


def windows_error(code):
    error = PermissionError("simulated native denial")
    error.winerror = code
    return error


class EditorRetryTests(unittest.TestCase):
    def test_transient_sharing_retries_without_dropping_the_edit(self):
        pending = [windows_error(32), windows_error(5), "saved"]
        retries = []
        def save():
            result = pending.pop(0)
            if isinstance(result, OSError):
                raise result
            return result
        self.assertEqual(retry_file_operation(save, platform_name="nt", on_retry=lambda: retries.append(1)), "saved")
        self.assertEqual(len(retries), 2)
        self.assertFalse(pending)

    def test_permanent_denial_is_bounded_and_propagated(self):
        error = windows_error(5)
        calls = []
        def denied():
            calls.append(1)
            raise error
        start = time.monotonic()
        with self.assertRaises(PermissionError) as caught:
            retry_file_operation(denied, platform_name="nt", deadline_seconds=.02)
        self.assertIs(caught.exception, error)
        self.assertGreater(len(calls), 1)
        self.assertLess(time.monotonic() - start, 1)

    def test_zero_budget_and_other_errors_are_never_retried(self):
        for platform, error, budget in [
            ("posix", windows_error(5), 5),
            ("nt", windows_error(2), 5),
            ("nt", OSError(5, "I/O failure, not WinError 5"), 5),
            ("nt", windows_error(33), 0),
        ]:
            calls = []
            def denied():
                calls.append(1)
                raise error
            with self.assertRaises(OSError) as caught:
                retry_file_operation(denied, platform_name=platform, deadline_seconds=budget)
            self.assertIs(caught.exception, error)
            self.assertEqual(len(calls), 1)

    def test_the_budget_cannot_be_disabled_or_made_unbounded(self):
        for budget in [-1, 6, float("inf"), float("nan")]:
            with self.assertRaises(ValueError):
                retry_file_operation(lambda: None, deadline_seconds=budget)


class ProbeAdmissionTests(unittest.TestCase):
    def test_parallel_editors_cannot_create_unbounded_probe_processes(self):
        admission = ProbeAdmission(4)
        active = 0
        peak = 0
        lock = threading.Lock()
        def probe():
            nonlocal active, peak
            with lock:
                active += 1
                peak = max(peak, active)
            time.sleep(.002)
            with lock:
                active -= 1
            return 1
        with ThreadPoolExecutor(max_workers=64) as pool:
            results = list(pool.map(lambda _: admission.run(probe), range(256)))
        self.assertEqual(sum(results), 256)
        self.assertLessEqual(peak, 4)
        self.assertEqual(admission.peak, peak)
        self.assertEqual(active, 0)

    def test_timeout_and_failure_do_not_leak_or_create_slots(self):
        admission = ProbeAdmission(1, timeout=0)
        error = OSError("probe creation refused")
        def fail():
            with self.assertRaises(TimeoutError):
                admission.run(lambda: self.fail("over-admitted"))
            raise error
        with self.assertRaises(OSError) as caught:
            admission.run(fail)
        self.assertIs(caught.exception, error)
        self.assertEqual(admission.run(lambda: 7), 7)
        self.assertEqual(admission.peak, 1)

    def test_unbounded_or_invalid_admission_is_refused(self):
        for workers in [0, 33, True, 1.5]:
            with self.assertRaises(ValueError):
                ProbeAdmission(workers)
        for timeout in [-1, 31, float("inf"), float("nan")]:
            with self.assertRaises(ValueError):
                ProbeAdmission(1, timeout=timeout)


if __name__ == "__main__":
    unittest.main()
