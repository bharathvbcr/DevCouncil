"""Verification & next-actions feedback hardening.

Covers four behaviors:
1. A genuinely failing test whose traceback mentions ImportError stays a blocking
   ``test_failed`` gap (not downgraded to non-blocking invalid_verification_command).
2. Real failure evidence (stdout/stderr log paths, traceback file:line) is threaded
   into gaps and next actions.
3. ``missing_evidence`` and unproven-AC routing are concrete (carry the criterion +
   expected verification method, attach only the AC's own checks).
4. ``build_correction_manifest`` picks root_cause by severity then gap-type priority.
"""

import asyncio

from devcouncil.domain.evidence import CommandResult
from devcouncil.domain.requirement import AcceptanceCriterion, Requirement
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.verification.next_actions import next_action_for
from devcouncil.verification.verifier import Verifier


def _requirement() -> Requirement:
    return Requirement(
        id="REQ-001",
        title="Password reset",
        description="Reset tokens are single use",
        priority="high",
        source="user",
        acceptance_criteria=[
            AcceptanceCriterion(
                id="AC-001",
                description="Token reuse is rejected",
                verification_method="unit_test",
            )
        ],
    )


def _task() -> Task:
    return Task(
        id="TASK-001",
        title="Implement reset token",
        description="Implement reset token rules",
        requirement_ids=["REQ-001"],
        acceptance_criterion_ids=["AC-001"],
        planned_files=[
            PlannedFile(path="src/auth.py", reason="token logic", allowed_change="modify"),
        ],
        allowed_commands=["pytest tests/test_auth.py"],
    )


# --- Fix #1: real failure whose traceback mentions ImportError stays blocking ----------


def test_failing_test_with_importerror_in_traceback_stays_blocking_test_failed(tmp_path):
    # A genuine failing test: pytest ran (exit 1), and the captured stdout shows a
    # real traceback that happens to mention ImportError. This must NOT be downgraded
    # to a non-blocking invalid_verification_command (which would let verify PASS).
    task = _task()
    task.expected_tests = ["python -m pytest tests/test_auth.py -q"]
    logdir = tmp_path / ".devcouncil" / "logs"
    logdir.mkdir(parents=True)
    stdout_log = logdir / "fail-stdout.log"
    stdout_log.write_text(
        "=== FAILURES ===\n"
        "____ test_token ____\n"
        '  File "tests/test_auth.py", line 12, in test_token\n'
        "    handler.reset()\n"
        '  File "src/auth.py", line 40, in reset\n'
        "    raise ImportError('boom')\n"
        "ImportError: boom\n"
        "1 failed in 0.10s\n",
        encoding="utf-8",
    )
    stderr_log = logdir / "fail-stderr.log"
    stderr_log.write_text("", encoding="utf-8")

    verifier = Verifier(tmp_path)
    verifier.get_changed_files = lambda: ["src/auth.py"]
    verifier.get_diff = lambda: "diff --git a/src/auth.py b/src/auth.py\n+token logic"
    verifier._run_command = lambda command, task_id="verify": CommandResult(
        command=command,
        exit_code=1,
        stdout_path=str(stdout_log),
        stderr_path=str(stderr_log),
        summary="Exit code 1. stderr: (empty). stdout: ImportError: boom | 1 failed",
    )

    gaps, _ = asyncio.run(verifier.verify_task(task, [_requirement()]))

    test_failed = [g for g in gaps if g.gap_type == "test_failed"]
    assert test_failed and test_failed[0].blocking
    assert not [g for g in gaps if g.gap_type == "invalid_verification_command"]


def test_missing_pytest_module_stays_unrunnable(tmp_path):
    # The launcher failed before pytest ran: ModuleNotFoundError straight from the
    # interpreter, no traceback frame -> unrunnable (non-blocking), not a code defect.
    task = _task()
    task.expected_tests = ["python -m pytest tests/test_auth.py -q"]
    logdir = tmp_path / ".devcouncil" / "logs"
    logdir.mkdir(parents=True)
    stderr_log = logdir / "missing-stderr.log"
    stderr_log.write_text(
        "/usr/bin/python: No module named pytest\n", encoding="utf-8"
    )
    stdout_log = logdir / "missing-stdout.log"
    stdout_log.write_text("", encoding="utf-8")

    verifier = Verifier(tmp_path)
    verifier.get_changed_files = lambda: ["src/auth.py"]
    verifier.get_diff = lambda: "diff --git a/src/auth.py b/src/auth.py\n+token logic"
    verifier._run_command = lambda command, task_id="verify": CommandResult(
        command=command,
        exit_code=1,
        stdout_path=str(stdout_log),
        stderr_path=str(stderr_log),
        summary="Exit code 1. stderr: No module named pytest. stdout: (empty)",
    )

    gaps, _ = asyncio.run(verifier.verify_task(task, [_requirement()]))

    assert not [g for g in gaps if g.gap_type == "test_failed"]
    assert [g for g in gaps if g.gap_type == "invalid_verification_command"]


def test_malformed_signature_precedes_traceback_helper(tmp_path):
    v = Verifier(tmp_path)
    # No traceback frame, signature present -> unrunnable.
    assert v._malformed_signature_precedes_traceback("ModuleNotFoundError: No module named flake8")
    # Traceback frame BEFORE the signature -> real defect, not unrunnable.
    assert not v._malformed_signature_precedes_traceback(
        'Traceback:\n  File "src/x.py", line 3, in f\n    import nope\nImportError: no module named nope'
    )
    # SyntaxError prints File "<string>" but is a compile failure -> unrunnable.
    assert v._malformed_signature_precedes_traceback(
        '  File "<string>", line 1\n    bad syntax\nSyntaxError: invalid syntax'
    )


# --- Fix #2: failure evidence (log paths + file:line) threaded into gap/next-action ----


def test_test_failed_gap_carries_log_paths_and_traceback_location(tmp_path):
    task = _task()
    task.expected_tests = ["python -m pytest tests/test_auth.py -q"]
    logdir = tmp_path / ".devcouncil" / "logs"
    logdir.mkdir(parents=True)
    stdout_log = logdir / "tf-stdout.log"
    stdout_log.write_text(
        '  File "tests/test_auth.py", line 7, in test_token\n'
        '  File "src/auth.py", line 41, in reset\n'
        "AssertionError: token reused\n",
        encoding="utf-8",
    )
    stderr_log = logdir / "tf-stderr.log"
    stderr_log.write_text("", encoding="utf-8")

    verifier = Verifier(tmp_path)
    verifier.get_changed_files = lambda: ["src/auth.py"]
    verifier.get_diff = lambda: "diff --git a/src/auth.py b/src/auth.py\n+token logic"
    verifier._run_command = lambda command, task_id="verify": CommandResult(
        command=command,
        exit_code=1,
        stdout_path=str(stdout_log),
        stderr_path=str(stderr_log),
        summary="Exit code 1. stdout: AssertionError: token reused",
    )

    gaps, _ = asyncio.run(verifier.verify_task(task, [_requirement()]))

    tf = [g for g in gaps if g.gap_type == "test_failed"]
    assert tf
    gap = tf[0]
    # Deepest real frame is src/auth.py:41.
    assert gap.file == "src/auth.py"
    assert gap.line == 41
    assert gap.stdout_path == str(stdout_log)
    assert gap.stderr_path == str(stderr_log)

    action = next_action_for(gap)
    assert action.file == "src/auth.py"
    assert action.line == 41
    assert action.stdout_path == str(stdout_log)
    assert action.stderr_path == str(stderr_log)


def test_failure_location_ignores_python_dash_c_string_frame(tmp_path):
    v = Verifier(tmp_path)
    result = CommandResult(
        command='python -c "..."',
        exit_code=1,
        stdout_path="",
        stderr_path="",
        summary='  File "<string>", line 1\nAssertionError',
    )
    file, line = v._failure_location(result)
    assert file is None  # "<string>" is not a real source file


# --- Fix #3: concrete unproven-AC routing -------------------------------------------


def test_unproven_ac_gap_is_concrete_and_scoped(tmp_path):
    # No compiled checks, no passing commands -> blocking unproven AC. The gap must
    # carry the expected verification method and an explicit "no check compiled"
    # marker, NOT a dump of every command summary.
    task = _task()
    task.allowed_commands = []
    verifier = Verifier(tmp_path)
    verifier.get_changed_files = lambda: ["src/auth.py"]
    verifier.get_diff = lambda: "diff --git a/src/auth.py b/src/auth.py\n+token logic"
    verifier._load_commands = lambda: {"test": [], "lint": [], "typecheck": []}

    gaps, _ = asyncio.run(verifier.verify_task(task, [_requirement()]))

    ac = [g for g in gaps if g.gap_type == "acceptance_criteria_unproven" and g.blocking]
    assert ac
    gap = ac[0]
    assert gap.acceptance_criterion_id == "AC-001"
    assert gap.expected_verification_method == "unit_test"
    assert any("no DevCouncil check compiled" in e for e in gap.evidence)

    action = next_action_for(gap)
    assert action.expected_verification_method == "unit_test"
    assert "AC-001" in action.missing_evidence
    assert "unit_test" in action.missing_evidence


def test_unproven_ac_attaches_only_its_own_compiled_check(tmp_path):
    # A compiled check targeted AC-001 and failed in a way that is unrunnable, so the
    # AC stays unproven. The gap must attach ONLY that AC's compiled command(s), not
    # all command summaries.
    class FakeCompiler:
        async def compile(self, task, requirements, code_context):
            return {"AC-001": ["python -m pytest tests/missing.py -q"]}

    task = _task()
    task.expected_tests = []
    task.allowed_commands = []
    verifier = Verifier(tmp_path)
    verifier.acceptance_compiler = FakeCompiler()
    verifier.get_changed_files = lambda: ["src/auth.py"]
    verifier.get_diff = lambda: "diff --git a/src/auth.py b/src/auth.py\n+token logic"
    verifier._commands_for_task = lambda task: {}
    verifier._run_command = lambda command, task_id="verify": CommandResult(
        command=command, exit_code=4, stdout_path="", stderr_path="",
        summary="ERROR: file or directory not found: tests/missing.py",
    )

    gaps, _ = asyncio.run(verifier.verify_task(task, [_requirement()]))

    ac = [g for g in gaps if g.gap_type == "acceptance_criteria_unproven"]
    assert ac
    gap = ac[0]
    assert any("pytest tests/missing.py" in e for e in gap.evidence)
    assert gap.suggested_command == "python -m pytest tests/missing.py -q"
