"""Transient-failure retry in the coding-CLI executor.

A CLI that dies on a network/provider fault ("API Error: Connection closed
mid-response", 429/5xx, overloaded) says nothing about the task. Without a
retry the failure ends the task ``blocked``, burns a repair attempt on a
non-code problem, and shows up in the benchmark as a false negative.
"""

import subprocess

import pytest

from devcouncil.domain.task import PlannedFile, Task
from devcouncil.executors.coding_cli import CodingCliExecutor


def _task() -> Task:
    return Task(
        id="TASK-001",
        title="Coding CLI",
        description="Implement feature",
        planned_files=[
            PlannedFile(path="src/app.py", reason="logic", allowed_change="modify"),
        ],
    )


@pytest.fixture
def no_sleep(monkeypatch):
    slept = []
    monkeypatch.setattr("time.sleep", lambda seconds: slept.append(seconds))
    return slept


def _wire(monkeypatch, outcomes):
    """Patch which/run; ``outcomes`` is a list of (returncode, stderr) per call."""
    calls = []

    monkeypatch.setattr("shutil.which", lambda command: f"/usr/bin/{command}")

    def fake_run(cmd, **kwargs):
        index = min(len(calls), len(outcomes) - 1)
        returncode, stderr = outcomes[index]
        calls.append(cmd)
        return subprocess.CompletedProcess(cmd, returncode, stdout="", stderr=stderr)

    monkeypatch.setattr("subprocess.run", fake_run)
    return calls


def test_transient_failure_is_retried_and_recovers(tmp_path, monkeypatch, no_sleep):
    calls = _wire(monkeypatch, [
        (1, "API Error: Connection closed mid-response. The response above may be incomplete."),
        (0, ""),
    ])

    result = CodingCliExecutor(tmp_path, "codex").run_task(_task(), [])

    assert result.success
    assert len(calls) == 2  # one failure + one successful retry
    assert no_sleep  # backoff happened between attempts


def test_transient_retry_gives_up_after_limit(tmp_path, monkeypatch, no_sleep):
    calls = _wire(monkeypatch, [
        (1, "error: 502 Bad Gateway from provider"),
    ])

    result = CodingCliExecutor(tmp_path, "codex").run_task(_task(), [])

    assert not result.success
    # initial attempt + default retry limit (2)
    assert len(calls) == 3


def test_genuine_failure_is_not_retried(tmp_path, monkeypatch, no_sleep):
    calls = _wire(monkeypatch, [
        (1, "SyntaxError: invalid syntax in src/app.py"),
    ])

    result = CodingCliExecutor(tmp_path, "codex").run_task(_task(), [])

    assert not result.success
    assert len(calls) == 1  # no retry for a real agent failure
    assert not no_sleep


def test_transient_retry_can_be_disabled_via_config(tmp_path, monkeypatch, no_sleep):
    (tmp_path / ".devcouncil").mkdir(parents=True)
    (tmp_path / ".devcouncil" / "config.yaml").write_text(
        "execution:\n  transient_retry_attempts: 0\n", encoding="utf-8"
    )
    calls = _wire(monkeypatch, [
        (1, "API Error: Connection closed mid-response."),
    ])

    result = CodingCliExecutor(tmp_path, "codex").run_task(_task(), [])

    assert not result.success
    assert len(calls) == 1


def test_transient_marker_in_stdout_tail_counts(tmp_path, monkeypatch, no_sleep):
    monkeypatch.setattr("shutil.which", lambda command: f"/usr/bin/{command}")
    calls = []

    def fake_run(cmd, **kwargs):
        calls.append(cmd)
        if len(calls) == 1:
            return subprocess.CompletedProcess(
                cmd, 1, stdout="...lots of output...\nfetch failed: socket hang up", stderr=""
            )
        return subprocess.CompletedProcess(cmd, 0, stdout="", stderr="")

    monkeypatch.setattr("subprocess.run", fake_run)

    result = CodingCliExecutor(tmp_path, "codex").run_task(_task(), [])

    assert result.success
    assert len(calls) == 2
