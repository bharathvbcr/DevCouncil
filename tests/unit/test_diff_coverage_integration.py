"""End-to-end diff↔coverage: run real coverage over a tiny repo and assert the gate.

These exercise the full path — git diff -> changed-line parse -> ``coverage run`` ->
``coverage json`` -> intersection -> gap — using the project interpreter (which has
coverage + pytest). Section 4's command execution is stubbed to "pass" so the test
isolates the diff-coverage gate (section 5b), not pytest-in-a-stripped-env.
"""

import asyncio
import subprocess
import sys

import pytest

from devcouncil.domain.evidence import CommandResult, DiffCoverageEvidence
from devcouncil.domain.requirement import AcceptanceCriterion, Requirement
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.verification.verifier import Verifier

pytest.importorskip("coverage")


def _git(args, cwd):
    subprocess.check_call(["git", *args], cwd=cwd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def _init_repo(tmp_path, test_body: str):
    _git(["init"], tmp_path)
    _git(["config", "user.email", "t@example.com"], tmp_path)
    _git(["config", "user.name", "Test"], tmp_path)
    (tmp_path / "calc.py").write_text("def add(a, b):\n    return a + b\n", encoding="utf-8")
    _git(["add", "calc.py"], tmp_path)
    _git(["commit", "-m", "base"], tmp_path)
    # The change under verification: a brand-new function `sub`.
    (tmp_path / "calc.py").write_text(
        "def add(a, b):\n    return a + b\n\n\ndef sub(a, b):\n    return a - b\n",
        encoding="utf-8",
    )
    (tmp_path / "test_calc.py").write_text(test_body, encoding="utf-8")


def _task() -> Task:
    return Task(
        id="TASK-001",
        title="add subtract",
        description="implement sub",
        requirement_ids=["REQ-001"],
        acceptance_criterion_ids=["AC-001"],
        planned_files=[
            PlannedFile(path="calc.py", reason="logic", allowed_change="modify"),
            PlannedFile(path="test_calc.py", reason="test", allowed_change="create"),
        ],
        expected_tests=["python -m pytest test_calc.py -q"],
    )


def _req() -> Requirement:
    return Requirement(
        id="REQ-001",
        title="subtract",
        description="sub works",
        priority="high",
        source="user",
        acceptance_criteria=[
            AcceptanceCriterion(id="AC-001", description="sub returns a-b", verification_method="unit_test"),
        ],
    )


def _verifier(tmp_path) -> Verifier:
    verifier = Verifier(tmp_path)
    verifier._coverage_python = sys.executable
    # Make the section-4 command "pass" so any_passing is true and the diff-coverage
    # gate is what we're testing — coverage itself is run for real in section 5b.
    verifier._run_command = lambda command, task_id="verify": CommandResult(
        command=command, exit_code=0, stdout_path="", stderr_path="", summary="passed",
    )
    return verifier


def test_flags_change_that_tests_never_exercise(tmp_path):
    # The passing test never imports the changed module -> the new logic is unproven.
    _init_repo(tmp_path, "def test_trivial():\n    assert True\n")

    gaps, evidence = asyncio.run(_verifier(tmp_path).verify_task(_task(), [_req()]))

    cov = [ev for ev in evidence if isinstance(ev, DiffCoverageEvidence)]
    assert cov and cov[0].measured
    assert cov[0].covered_lines == 0
    # coverage --source=. reports the changed module as present-but-unexecuted.
    assert "calc.py" in cov[0].uncovered_by_file
    assert any(g.gap_type == "diff_not_exercised" for g in gaps)


def test_accepts_change_that_tests_exercise(tmp_path):
    # The passing test calls the new function -> the changed lines are executed.
    _init_repo(tmp_path, "import calc\n\ndef test_sub():\n    assert calc.sub(5, 2) == 3\n")

    gaps, evidence = asyncio.run(_verifier(tmp_path).verify_task(_task(), [_req()]))

    cov = [ev for ev in evidence if isinstance(ev, DiffCoverageEvidence)]
    assert cov and cov[0].measured
    assert cov[0].covered_lines > 0
    assert not any(g.gap_type == "diff_not_exercised" for g in gaps)


def test_inline_c_acceptance_check_is_instrumented(tmp_path):
    # An inline `python -c` acceptance check (what the acceptance compiler emits) that
    # calls the new function must register as exercising the changed lines.
    _init_repo(tmp_path, "def test_placeholder():\n    assert True\n")

    task = _task()
    task.expected_tests = ['python -c "import calc; assert calc.sub(5, 2) == 3"']

    gaps, evidence = asyncio.run(_verifier(tmp_path).verify_task(task, [_req()]))

    cov = [ev for ev in evidence if isinstance(ev, DiffCoverageEvidence)]
    assert cov and cov[0].measured
    assert cov[0].covered_lines > 0
    assert not any(g.gap_type == "diff_not_exercised" for g in gaps)


def test_enforce_with_min_ratio_blocks_partial_coverage(tmp_path):
    # Importing calc but never calling sub() leaves the body unexercised. With a
    # strict min_ratio and enforcement on, that partial coverage becomes blocking.
    _init_repo(tmp_path, "import calc\n\ndef test_add():\n    assert calc.add(1, 2) == 3\n")

    verifier = _verifier(tmp_path)
    verifier._diff_coverage_override = (True, True, 1.0)

    gaps, evidence = asyncio.run(verifier.verify_task(_task(), [_req()]))

    cov = [ev for ev in evidence if isinstance(ev, DiffCoverageEvidence)]
    assert cov and cov[0].measured
    assert 0 < cov[0].coverage_ratio < 1.0
    blocking = [g for g in gaps if g.gap_type == "diff_not_exercised" and g.blocking]
    assert blocking
    assert blocking[0].suggested_command == "python -m pytest test_calc.py -q"
