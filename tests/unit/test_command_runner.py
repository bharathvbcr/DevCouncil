import subprocess
from pathlib import Path

from devcouncil.verification import command_runner


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
