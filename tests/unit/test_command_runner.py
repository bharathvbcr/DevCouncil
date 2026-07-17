import subprocess
import sys
from pathlib import Path

from devcouncil.verification import command_runner


def test_python_launchers_use_active_project_interpreter(tmp_path, monkeypatch):
    invocations = []

    def capture(argv, **kwargs):
        invocations.append(argv)
        return subprocess.CompletedProcess(argv, 0, stdout="ok", stderr="")

    monkeypatch.setattr(command_runner.subprocess, "run", capture)

    for launcher in ("python", "python3"):
        result = command_runner.run_verification_command(
            tmp_path,
            f'{launcher} -c "print(1)"',
        )
        assert result.exit_code == 0

    assert [argv[0] for argv in invocations] == [sys.executable, sys.executable]


def test_run_verification_command_classifies_timeout_and_saves_partial_output(
    tmp_path, monkeypatch
):
    def raise_timeout(argv, **kwargs):
        raise subprocess.TimeoutExpired(
            cmd=argv,
            timeout=kwargs["timeout"],
            output=b"partial stdout",
            stderr=b"partial stderr",
        )

    monkeypatch.setattr(command_runner.subprocess, "run", raise_timeout)

    result = command_runner.run_verification_command(
        tmp_path,
        'python -c "pass"',
        timeout=7,
    )

    assert result.exit_code == 124
    assert result.timed_out is True
    assert "timed out after 7 seconds" in result.summary
    assert "partial stdout" in result.summary
    assert "partial stderr" in result.summary
    assert "partial stdout" in Path(result.stdout_path).read_text(encoding="utf-8")
    assert "partial stderr" in Path(result.stderr_path).read_text(encoding="utf-8")
