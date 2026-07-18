"""Unit tests for claim check helpers (stable/deterministic)."""

from __future__ import annotations

import subprocess
from pathlib import Path
from unittest.mock import patch

from devcouncil.verification.claims.checks import (
    ClaimCheckBudget,
    CommandOutcome,
    _argv_to_shell,
    _command_not_found,
    _normalize,
    _tail,
    git_changed_files,
    git_toplevel,
    last_commit_paths,
    run_command,
)
from devcouncil.verification.claims.models import Assertion, Kind, Status


def test_tail_and_argv_helpers():
    assert "b" in _tail("a\n" * 60 + "b")
    assert _tail(None, b"hello") == "hello"
    assert _tail("  ") == ""
    assert _argv_to_shell("echo hi") == "echo hi"
    assert "pytest" in _argv_to_shell(["python", "-m", "pytest"])


def test_run_command_success_timeout_and_oserror(tmp_path: Path):
    ok = run_command("python -c \"print('x')\"", tmp_path, timeout=30)
    assert ok.exit_code == 0
    assert "x" in ok.output_tail

    timed = run_command("python -c \"import time; time.sleep(5)\"", tmp_path, timeout=1)
    assert timed.timed_out is True

    with patch("subprocess.run", side_effect=OSError("boom")):
        failed = run_command("nope", tmp_path, timeout=5)
        assert failed.exit_code is None
        assert "failed to launch" in failed.output_tail


def test_command_not_found_markers():
    assert _command_not_found(CommandOutcome(127, "", 0.1, False))
    assert _command_not_found(CommandOutcome(1, "command not found", 0.1, False))
    assert not _command_not_found(CommandOutcome(0, "", 0.1, False))


def test_git_helpers(tmp_path: Path):
    assert git_toplevel(tmp_path) is None
    assert last_commit_paths(tmp_path) == set()
    assert git_changed_files(tmp_path) is None

    subprocess.run(["git", "init"], cwd=tmp_path, check=True, capture_output=True)
    subprocess.run(["git", "config", "user.email", "t@t"], cwd=tmp_path, check=True, capture_output=True)
    subprocess.run(["git", "config", "user.name", "t"], cwd=tmp_path, check=True, capture_output=True)
    (tmp_path / "a.py").write_text("1\n", encoding="utf-8")
    subprocess.run(["git", "add", "a.py"], cwd=tmp_path, check=True, capture_output=True)
    subprocess.run(["git", "commit", "-m", "init"], cwd=tmp_path, check=True, capture_output=True)

    top = git_toplevel(tmp_path)
    assert top is not None
    assert "a.py" in last_commit_paths(tmp_path)
    (tmp_path / "b.py").write_text("2\n", encoding="utf-8")
    changed = git_changed_files(tmp_path)
    assert changed is not None
    assert any("b.py" in p for p in changed)
    assert _normalize("Foo/Bar") == "foo/bar"


def test_command_result_paths(tmp_path: Path):
    from devcouncil.verification.claims.checks import _Runner, _command_result

    assertion = Assertion(kind=Kind.TESTS_PASS, source_text="tests pass")
    runner = _Runner(ClaimCheckBudget(per_command_timeout=10, total_timeout=20), tmp_path)
    missing = _command_result(assertion, None, "test", runner)
    assert missing.status == Status.UNVERIFIABLE

    ok = _command_result(assertion, "python -c \"print(1)\"", "test", runner)
    assert ok.status == Status.PASS
    again = _command_result(assertion, "python -c \"print(1)\"", "test", runner)
    assert again.status == Status.PASS

    fail = _command_result(assertion, "python -c \"raise SystemExit(2)\"", "test", runner)
    assert fail.status == Status.FAIL


def test_execute_checks_and_file_result(tmp_path: Path):
    from types import SimpleNamespace

    from devcouncil.verification.claims.checks import (
        ResolvedCommands,
        execute_checks,
        resolve_commands_from_config,
        _file_result,
        _git_info,
    )

    cfg = SimpleNamespace(
        test=["python -c \"print('ok')\""],
        typecheck="python -c \"print('tc')\"",
        lint=["python", "-c", "print('lint')"],
    )
    resolved = resolve_commands_from_config(cfg)
    assert resolved.test
    assert resolved.build
    assert resolved.lint

    (tmp_path / "created.py").write_text("x\n", encoding="utf-8")
    results = execute_checks(
        [
            Assertion(kind=Kind.TESTS_PASS),
            Assertion(kind=Kind.BUILD_SUCCEEDS),
            Assertion(kind=Kind.LINT_CLEAN),
            Assertion(kind=Kind.FILE_CREATED, target="created.py"),
            Assertion(kind=Kind.FILE_UPDATED, target="missing.py"),
            Assertion(kind=Kind.COMMAND_SUCCEEDED, target="python -c \"print('ok')\""),
            Assertion(kind=Kind.COMMAND_SUCCEEDED, target="not-a-real-command"),
            Assertion(kind=Kind.GENERIC_DONE),
        ],
        cwd=tmp_path,
        commands=resolved,
    )
    assert len(results) == 8
    assert results[0].status == Status.PASS
    assert results[4].status == Status.FAIL
    assert results[6].status == Status.UNVERIFIABLE

    outside = _file_result(
        Assertion(kind=Kind.FILE_CREATED, target="/etc/passwd"),
        tmp_path,
        None,
    )
    assert outside.status == Status.UNVERIFIABLE

    exists = _file_result(
        Assertion(kind=Kind.FILE_CREATED, target="created.py"),
        tmp_path,
        None,
    )
    assert exists.status == Status.PASS

    subprocess.run(["git", "init"], cwd=tmp_path, check=True, capture_output=True)
    subprocess.run(["git", "config", "user.email", "t@t"], cwd=tmp_path, check=True, capture_output=True)
    subprocess.run(["git", "config", "user.name", "t"], cwd=tmp_path, check=True, capture_output=True)
    subprocess.run(["git", "add", "created.py"], cwd=tmp_path, check=True, capture_output=True)
    subprocess.run(["git", "commit", "-m", "c"], cwd=tmp_path, check=True, capture_output=True)
    git = _git_info(tmp_path)
    assert git is not None
    committed = _file_result(
        Assertion(kind=Kind.FILE_UPDATED, target="created.py"),
        tmp_path,
        git,
    )
    assert committed.status in {Status.PASS, Status.UNVERIFIABLE}
