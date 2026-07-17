import json
from pathlib import Path

from typer.testing import CliRunner

from devcouncil.cli.main import app

runner = CliRunner()


def test_cli_boot_help():
    result = runner.invoke(app, ["boot", "--help"])
    assert result.exit_code == 0
    assert "Initialize the repo" in result.output
    assert "--skip-integrations" in result.output
    assert "--scaffold-ci-evidence" in result.output
    assert "--executor" in result.output


def test_cli_boot_runs_setup_integrate_and_go(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)

    setup_calls = {"integrations": 0}
    go_calls: list[dict] = []

    import devcouncil.cli.commands.boot as boot_cmd

    def fake_setup_path(root, **kwargs):
        setup_calls["integrations"] += 0 if kwargs["skip_integrations"] else 1
        setup_calls["skip_api_key"] = kwargs["skip_api_key"]

    def fake_go(ctx, goal, **kwargs):
        go_calls.append({"goal": goal, **kwargs})

    monkeypatch.setattr(boot_cmd, "_run_setup_path", fake_setup_path)
    monkeypatch.setattr(boot_cmd, "go_command", fake_go)

    class _NonTTYStdin:
        def isatty(self):
            return False

    monkeypatch.setattr(boot_cmd.sys, "stdin", _NonTTYStdin())

    result = runner.invoke(
        app,
        ["boot", "Ship feature X", "--executor", "codex", "--quick"],
    )

    assert result.exit_code == 0
    assert setup_calls["integrations"] == 1
    assert setup_calls["skip_api_key"] is True
    assert len(go_calls) == 1
    assert go_calls[0]["goal"] == "Ship feature X"
    assert go_calls[0]["executor"] == "codex"
    assert go_calls[0]["quick"] is True


def test_cli_boot_skip_integrations(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)

    setup_calls: dict = {}

    import devcouncil.cli.commands.boot as boot_cmd

    def fake_setup_path(root, **kwargs):
        setup_calls.update(kwargs)

    monkeypatch.setattr(boot_cmd, "_run_setup_path", fake_setup_path)
    monkeypatch.setattr(boot_cmd, "go_command", lambda *a, **k: None)

    result = runner.invoke(app, ["boot", "Goal", "--skip-integrations"])
    assert result.exit_code == 0
    assert setup_calls["skip_integrations"] is True
