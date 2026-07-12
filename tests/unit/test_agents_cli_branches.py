"""Additional CLI branch coverage for `dev agents`."""

import json
import subprocess
from types import SimpleNamespace

import yaml
from typer.testing import CliRunner

from devcouncil.cli.main import app
from devcouncil.optimization.gepa_agent import GepaUnavailableError

runner = CliRunner()


def test_cli_agents_add_rejects_invalid_input_mode(tmp_path):
    result = runner.invoke(
        app,
        ["agents", "add", "custom", "--command", "custom", "--input-mode", "bad", "--project-root", str(tmp_path)],
    )
    assert result.exit_code == 2
    assert "--input-mode must be one of" in result.output


def test_cli_agents_add_rejects_blank_name(tmp_path):
    result = runner.invoke(
        app,
        ["agents", "add", " ", "--command", "custom", "--project-root", str(tmp_path)],
    )
    assert result.exit_code == 2
    assert "Agent name cannot be empty" in result.output


def test_cli_agents_help_unknown_agent(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    result = runner.invoke(app, ["agents", "help", "missing-agent"])
    assert result.exit_code == 1
    assert "Unknown agent" in result.output


def test_cli_agents_help_executable_missing(tmp_path, monkeypatch):
    import yaml as yaml_mod

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    config_path = tmp_path / ".devcouncil" / "config.yaml"
    raw = yaml_mod.safe_load(config_path.read_text(encoding="utf-8"))
    raw.setdefault("integrations", {}).setdefault("cli_agents", {}).setdefault("agents", {})["custombot"] = {
        "command": "custombot",
        "args": [],
        "input_mode": "stdin",
    }
    config_path.write_text(yaml_mod.safe_dump(raw), encoding="utf-8")
    monkeypatch.setattr("devcouncil.cli.commands.agents._which", lambda command: None)

    result = runner.invoke(app, ["agents", "help", "custombot"])
    assert result.exit_code == 1
    assert "not installed or not on PATH" in result.output


def test_cli_agents_help_runs_underlying_cli(tmp_path, monkeypatch):
    import yaml as yaml_mod

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    config_path = tmp_path / ".devcouncil" / "config.yaml"
    raw = yaml_mod.safe_load(config_path.read_text(encoding="utf-8"))
    raw.setdefault("integrations", {}).setdefault("cli_agents", {}).setdefault("agents", {})["custombot"] = {
        "command": "custombot",
        "args": ["--help"],
        "input_mode": "stdin",
        "help_command": ["custombot", "--help"],
    }
    config_path.write_text(yaml_mod.safe_dump(raw), encoding="utf-8")
    monkeypatch.setattr("devcouncil.cli.commands.agents._which", lambda command: "/usr/bin/custombot")

    class _Result:
        returncode = 0
        stdout = "custom help"
        stderr = ""

    monkeypatch.setattr(subprocess, "run", lambda *a, **k: _Result())

    result = runner.invoke(app, ["agents", "help", "custombot"])
    assert result.exit_code == 0
    assert "custom help" in result.output


def test_cli_agents_help_subprocess_failure(tmp_path, monkeypatch):
    import yaml as yaml_mod

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    config_path = tmp_path / ".devcouncil" / "config.yaml"
    raw = yaml_mod.safe_load(config_path.read_text(encoding="utf-8"))
    raw.setdefault("integrations", {}).setdefault("cli_agents", {}).setdefault("agents", {})["custombot"] = {
        "command": "custombot",
        "input_mode": "stdin",
    }
    config_path.write_text(yaml_mod.safe_dump(raw), encoding="utf-8")
    monkeypatch.setattr("devcouncil.cli.commands.agents._which", lambda command: "/usr/bin/custombot")

    def _boom(*args, **kwargs):
        raise subprocess.TimeoutExpired(cmd="custombot", timeout=15)

    monkeypatch.setattr(subprocess, "run", _boom)

    result = runner.invoke(app, ["agents", "help", "custombot"])
    assert result.exit_code == 1
    assert "Failed to run" in result.output


def test_cli_agents_optimize_gepa_unavailable(tmp_path, monkeypatch):
    project = tmp_path / "project"
    config_path = project / ".devcouncil" / "config.yaml"
    config_path.parent.mkdir(parents=True)
    config_path.write_text(yaml.safe_dump({"integrations": {"cli_agents": {"profiles": {}}}}), encoding="utf-8")
    evals = project / "evals.jsonl"
    evals.write_text(json.dumps({"id": "e1"}) + "\n", encoding="utf-8")

    def _raise(**kwargs):
        raise GepaUnavailableError("GEPA not installed")

    monkeypatch.setattr("devcouncil.cli.commands.agents.optimize_agent_profile", _raise)

    result = runner.invoke(
        app,
        ["agents", "optimize", "--agent", "codex", "--evals", str(evals), "--project-root", str(project)],
    )
    assert result.exit_code == 1
    assert "GEPA not installed" in result.output


def test_cli_agents_optimize_value_error(tmp_path, monkeypatch):
    project = tmp_path / "project"
    config_path = project / ".devcouncil" / "config.yaml"
    config_path.parent.mkdir(parents=True)
    config_path.write_text(yaml.safe_dump({"integrations": {"cli_agents": {"profiles": {}}}}), encoding="utf-8")
    evals = project / "evals.jsonl"
    evals.write_text(json.dumps({"id": "e1"}) + "\n", encoding="utf-8")

    def _raise(**kwargs):
        raise ValueError("bad evals")

    monkeypatch.setattr("devcouncil.cli.commands.agents.optimize_agent_profile", _raise)

    result = runner.invoke(
        app,
        ["agents", "optimize", "--agent", "codex", "--evals", str(evals), "--project-root", str(project)],
    )
    assert result.exit_code == 2
    assert "bad evals" in result.output


def test_cli_agents_doctor_no_coding_cli_on_path(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    monkeypatch.setattr("devcouncil.cli.commands.agents._which", lambda command: None)
    monkeypatch.setattr("devcouncil.cli.commands.agents.detect_available_coding_cli", lambda root: [])

    result = runner.invoke(app, ["agents", "doctor"])
    assert result.exit_code == 0
    assert "No built-in coding CLI on PATH" in result.output


def test_cli_agents_doctor_help_ok_for_custom(tmp_path, monkeypatch):
    import yaml as yaml_mod

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    config_path = tmp_path / ".devcouncil" / "config.yaml"
    raw = yaml_mod.safe_load(config_path.read_text(encoding="utf-8"))
    raw.setdefault("integrations", {}).setdefault("cli_agents", {}).setdefault("agents", {})["custombot"] = {
        "command": "custombot",
        "input_mode": "stdin",
        "help_command": ["custombot", "--help"],
    }
    config_path.write_text(yaml_mod.safe_dump(raw), encoding="utf-8")
    monkeypatch.setattr("devcouncil.cli.commands.agents._which", lambda command: "/usr/bin/custombot")

    class _Result:
        returncode = 0
        stdout = ""
        stderr = ""

    monkeypatch.setattr(subprocess, "run", lambda *a, **k: _Result())

    result = runner.invoke(app, ["agents", "doctor"])
    assert result.exit_code == 0
    assert "help command OK" in result.output


def test_cli_agents_optimize_apply_mode(tmp_path, monkeypatch):
    project = tmp_path / "project"
    config_path = project / ".devcouncil" / "config.yaml"
    config_path.parent.mkdir(parents=True)
    config_path.write_text(
        yaml.safe_dump({"integrations": {"cli_agents": {"profiles": {"default": {"prompt_preamble": "old"}}}}}),
        encoding="utf-8",
    )
    evals = project / "evals.jsonl"
    evals.write_text(json.dumps({"id": "e1"}) + "\n", encoding="utf-8")

    monkeypatch.setattr(
        "devcouncil.cli.commands.agents.optimize_agent_profile",
        lambda **kwargs: SimpleNamespace(
            agent="codex",
            profile_name="default",
            best_score=0.9,
            best_preamble="new preamble",
            artifact_path=project / "artifact.json",
            applied=kwargs["apply"],
        ),
    )

    result = runner.invoke(
        app,
        ["agents", "optimize", "--agent", "codex", "--evals", str(evals), "--apply", "--project-root", str(project)],
    )
    assert result.exit_code == 0
    assert "applied" in result.output
