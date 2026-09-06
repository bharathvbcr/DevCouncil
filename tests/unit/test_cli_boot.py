from typer.testing import CliRunner

from devcouncil.cli.main import app

runner = CliRunner()


def test_cli_boot_help():
    result = runner.invoke(app, ["boot", "--help"])
    assert result.exit_code == 0
    import re
    clean_output = re.sub(r'\x1b\[[0-9;]*[mK]', '', result.output)
    assert "Initialize the repo" in clean_output
    assert "--skip-integrations" in clean_output
    assert "--scaffold-ci-evidence" in clean_output
    assert "--executor" in clean_output



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


# --- `--json` contract: exactly one JSON object on stdout, diagnostics on stderr ---
#
# `dev boot` delegates its payload to the go flow, so a pre-flight failure exits before
# any object is produced. Asserting on `result.stdout`, never `result.output` — under
# Click 8.4 `.output` is the two streams merged and cannot detect the difference.


def test_boot_json_bad_gemini_scope_emits_one_object(tmp_path, monkeypatch):
    import json

    monkeypatch.chdir(tmp_path)

    result = runner.invoke(app, ["boot", "Goal", "--json", "--gemini-scope", "nonsense"])

    assert result.exit_code == 2
    data = json.loads(result.stdout)
    assert data["ok"] is False
    assert "--gemini-scope must be" in data["error"]
    assert "--gemini-scope must be" in result.stderr


def test_boot_json_bad_provider_emits_one_object(tmp_path, monkeypatch):
    import json

    monkeypatch.chdir(tmp_path)

    result = runner.invoke(app, ["boot", "Goal", "--json", "--provider", "nonsense-provider"])

    assert result.exit_code == 2
    data = json.loads(result.stdout)
    assert data["ok"] is False
    assert data["error"]
    assert result.stderr


def test_boot_json_setup_phase_keeps_stdout_empty(tmp_path, monkeypatch):
    """The whole setup phase ran onto the payload stream before this.

    `_run_setup_path` is the real one here, not a fake: the leak was never in boot.py
    itself but in what it calls — the `dev init` banner, `dev setup`'s API-key note, and
    the `dev doctor` table. The go phase is stubbed because it is covered separately and
    would otherwise try to reach a model provider.
    """
    import devcouncil.cli.commands.boot as boot_cmd

    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr(boot_cmd, "go_command", lambda *a, **k: None)

    class _NonTTYStdin:
        def isatty(self):
            return False

    monkeypatch.setattr(boot_cmd.sys, "stdin", _NonTTYStdin())

    result = runner.invoke(
        app,
        ["boot", "Goal", "--json", "--skip-map", "--skip-skills", "--skip-integrations"],
    )

    assert result.exit_code == 0
    # The go phase is stubbed, so it emits no payload — the point is that nothing
    # *else* did either. Every banner the setup phase produced is on stderr.
    assert result.stdout == ""
    assert "Initializing DevCouncil" in result.stderr
    assert "DevCouncil Doctor Check" in result.stderr


def test_boot_human_setup_phase_still_reports(tmp_path, monkeypatch):
    """Characterization: human mode still tells the user what happened."""
    import devcouncil.cli.commands.boot as boot_cmd

    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr(boot_cmd, "go_command", lambda *a, **k: None)

    class _NonTTYStdin:
        def isatty(self):
            return False

    monkeypatch.setattr(boot_cmd.sys, "stdin", _NonTTYStdin())

    result = runner.invoke(
        app, ["boot", "Goal", "--skip-map", "--skip-skills", "--skip-integrations"]
    )

    assert result.exit_code == 0
    assert "Initializing DevCouncil" in result.output
    assert "DevCouncil Doctor Check" in result.output
