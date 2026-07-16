import subprocess
import sys

import pytest

from devcouncil.domain.evidence import CommandResult
from devcouncil.verification.ad_hoc_check import run_working_tree_check
from devcouncil.verification.verifier import Verifier

pytest.importorskip("coverage")


def _git(args, cwd):
    subprocess.check_call(["git", *args], cwd=cwd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def test_no_changes_returns_passed(tmp_path):
    _git(["init"], tmp_path)
    result = run_working_tree_check(tmp_path)
    assert result.passed
    assert result.reason == "no_changes"
    assert result.changed_files == []


def test_check_verifies_live_paths_even_when_global_baseline_contains_them(tmp_path):
    _git(["init"], tmp_path)
    _git(["config", "user.email", "t@example.com"], tmp_path)
    _git(["config", "user.name", "Test"], tmp_path)
    (tmp_path / "calc.py").write_text("VALUE = 1\n", encoding="utf-8")
    _git(["add", "calc.py"], tmp_path)
    _git(["commit", "-m", "base"], tmp_path)
    (tmp_path / "calc.py").write_text("VALUE = 2\n", encoding="utf-8")
    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    (dev_dir / "baseline.json").write_text(
        '{"changed_files": ["calc.py"]}\n',
        encoding="utf-8",
    )

    verifier = Verifier(tmp_path)
    verifier._run_command = lambda command, task_id="verify": CommandResult(
        command=command,
        exit_code=0,
        stdout_path="",
        stderr_path="",
        summary="passed",
    )

    result = run_working_tree_check(
        tmp_path,
        "VALUE is updated",
        test_commands=['python -c "assert True"'],
        verifier=verifier,
    )

    assert result.passed
    assert result.changed_files == ["calc.py"]
    assert not [
        gap
        for gap in result.gaps
        if gap.gap_type == "planned_file_not_changed" and gap.file == "calc.py"
    ]


def test_flags_unexercised_change_with_next_action(tmp_path):
    _git(["init"], tmp_path)
    _git(["config", "user.email", "t@example.com"], tmp_path)
    _git(["config", "user.name", "Test"], tmp_path)
    (tmp_path / "calc.py").write_text("def add(a, b):\n    return a + b\n", encoding="utf-8")
    _git(["add", "calc.py"], tmp_path)
    _git(["commit", "-m", "base"], tmp_path)
    (tmp_path / "calc.py").write_text(
        "def add(a, b):\n    return a + b\n\n\ndef sub(a, b):\n    return a - b\n", encoding="utf-8"
    )
    # A passing test that never exercises the new function.
    (tmp_path / "test_calc.py").write_text("def test_trivial():\n    assert True\n", encoding="utf-8")

    verifier = Verifier(tmp_path)
    verifier._coverage_python = sys.executable
    verifier._run_command = lambda command, task_id="verify": CommandResult(
        command=command, exit_code=0, stdout_path="", stderr_path="", summary="passed",
    )

    result = run_working_tree_check(
        tmp_path,
        "sub returns a - b",
        test_commands=["python -m pytest test_calc.py -q"],
        enforce_coverage=True,
        verifier=verifier,
    )

    assert not result.passed
    assert "calc.py" in result.changed_files
    assert any(g.gap_type == "diff_not_exercised" and g.blocking for g in result.gaps)
    assert any(a.category == "add_test" for a in result.next_actions)
    assert result.diff_coverage is not None and result.diff_coverage.measured
