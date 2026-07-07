"""run_git / git_output subprocess helpers.

Uses real temp git repos (many existing unit tests already shell out to git in
tmp_path). Error contract per source: run_git never raises on timeout (rc 124)
but propagates OSError and, with check=True, CalledProcessError; git_output
returns ``default`` on any failure when given, otherwise raises.
"""

import subprocess

import pytest

import devcouncil.utils.proc as proc
from devcouncil.utils.proc import git_output, run_git


@pytest.fixture
def git_repo(tmp_path):
    init = run_git(["init"], tmp_path)
    assert init.returncode == 0, init.stderr
    return tmp_path


def test_run_git_success_captures_text_stdout(git_repo):
    result = run_git(["rev-parse", "--is-inside-work-tree"], git_repo)

    assert result.returncode == 0
    assert isinstance(result.stdout, str)
    assert result.stdout.strip() == "true"


def test_run_git_does_not_double_prefix_explicit_git_argv(git_repo):
    result = run_git(["git", "rev-parse", "--is-inside-work-tree"], git_repo)

    assert result.returncode == 0
    assert result.stdout.strip() == "true"


def test_run_git_failure_returns_nonzero_without_raising(git_repo):
    result = run_git(["rev-parse", "--verify", "no-such-ref-xyz"], git_repo)

    assert result.returncode != 0


def test_run_git_check_true_raises_on_failure(git_repo):
    with pytest.raises(subprocess.CalledProcessError):
        run_git(["rev-parse", "--verify", "no-such-ref-xyz"], git_repo, check=True)


def test_run_git_propagates_os_error_for_missing_cwd(tmp_path):
    missing = tmp_path / "does-not-exist"

    with pytest.raises(OSError):
        run_git(["status"], missing)


def test_run_git_timeout_is_surfaced_as_returncode_124(tmp_path, monkeypatch):
    def fake_run(*args, **kwargs):
        raise subprocess.TimeoutExpired(cmd=["git", "status"], timeout=kwargs.get("timeout", 5))

    monkeypatch.setattr(proc.subprocess, "run", fake_run)

    result = run_git(["status"], tmp_path, timeout=5)

    assert result.returncode == 124
    assert result.stdout == ""
    assert "timed out" in result.stderr
    assert result.args == ["git", "status"]


def test_git_output_returns_stdout_on_success(git_repo):
    assert git_output(["rev-parse", "--is-inside-work-tree"], git_repo).strip() == "true"


def test_git_output_returns_default_on_failure(git_repo):
    result = git_output(["rev-parse", "--verify", "no-such-ref-xyz"], git_repo, default="fallback")

    assert result == "fallback"


def test_git_output_without_default_raises_on_failure(git_repo):
    with pytest.raises(subprocess.CalledProcessError):
        git_output(["rev-parse", "--verify", "no-such-ref-xyz"], git_repo)


def test_git_output_swallows_os_error_when_default_given(tmp_path):
    missing = tmp_path / "does-not-exist"

    assert git_output(["status"], missing, default="none") == "none"
