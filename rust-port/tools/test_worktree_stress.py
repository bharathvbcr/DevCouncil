"""Failure semantics for the native capacity harness's simulated editors."""
import time
import unittest
from worktree_stress import retry_file_operation


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


if __name__ == "__main__":
    unittest.main()
