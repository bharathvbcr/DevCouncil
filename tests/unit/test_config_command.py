"""CLI coverage for `dev config show` / `dev config set` / `dev config models`."""

import yaml
from typer.testing import CliRunner

from devcouncil.cli.main import app

runner = CliRunner()


def _raw(root):
    return yaml.safe_load((root / ".devcouncil" / "config.yaml").read_text(encoding="utf-8"))


# --- show -------------------------------------------------------------------------


def test_config_show_prints_settings(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    result = runner.invoke(app, ["config", "show"])
    assert result.exit_code == 0
    assert "DevCouncil Settings" in result.output
    assert "execution.default_executor" in result.output
    assert "semantic_layer.enabled" in result.output


def test_config_show_missing_config_errors(tmp_path, monkeypatch):
    project = tmp_path / "empty"
    project.mkdir()
    monkeypatch.chdir(tmp_path)

    result = runner.invoke(app, ["config", "show", "--project-root", str(project)])
    assert result.exit_code == 1


def test_config_show_expands_semantic_layer_when_enabled(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    raw = _raw(tmp_path)
    raw.setdefault("semantic_layer", {})["enabled"] = True
    (tmp_path / ".devcouncil" / "config.yaml").write_text(yaml.dump(raw), encoding="utf-8")

    result = runner.invoke(app, ["config", "show"])
    assert result.exit_code == 0
    assert "semantic_layer.cache.enabled" in result.output
    assert "semantic_layer.embedding.model_name" in result.output


# --- set --------------------------------------------------------------------------


def test_config_set_int_value(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    result = runner.invoke(app, ["config", "set", "execution.command_timeout", "120"])
    assert result.exit_code == 0
    assert "Updated execution.command_timeout = 120" in result.output
    assert _raw(tmp_path)["execution"]["command_timeout"] == 120


def test_config_set_bool_value_nested(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    result = runner.invoke(app, ["config", "set", "verification.diff_coverage.enforce", "true"])
    assert result.exit_code == 0
    assert _raw(tmp_path)["verification"]["diff_coverage"]["enforce"] is True


def test_config_set_string_value(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    result = runner.invoke(app, ["config", "set", "execution.default_executor", "claude"])
    assert result.exit_code == 0
    assert _raw(tmp_path)["execution"]["default_executor"] == "claude"


def test_config_set_unsupported_key(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    result = runner.invoke(app, ["config", "set", "made.up.key", "1"])
    assert result.exit_code == 2
    assert "Unsupported key" in result.output


def test_config_set_invalid_bool_value(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    result = runner.invoke(app, ["config", "set", "execution.verify_on_post_task", "maybe"])
    assert result.exit_code == 2
    assert "Expected boolean" in result.output


def test_config_set_missing_config(tmp_path, monkeypatch):
    project = tmp_path / "empty"
    project.mkdir()
    monkeypatch.chdir(tmp_path)

    result = runner.invoke(
        app, ["config", "set", "execution.command_timeout", "5", "--project-root", str(project)]
    )
    assert result.exit_code == 1
    assert "Config not found" in result.output


# --- models: role-specific branches -----------------------------------------------


def test_config_models_show_single_role(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    result = runner.invoke(app, ["config", "models", "--role", "arbiter"])
    assert result.exit_code == 0
    assert "arbiter" in result.output


def test_config_models_show_unknown_role(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    result = runner.invoke(app, ["config", "models", "--role", "does_not_exist"])
    assert result.exit_code == 0
    assert "not found" in result.output


def test_config_models_set_single_role(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    result = runner.invoke(
        app, ["config", "models", "--role", "arbiter", "--model", "openai/custom"]
    )
    assert result.exit_code == 0
    assert "Updated 'arbiter' to use model 'openai/custom'" in result.output
    assert _raw(tmp_path)["models"]["roles"]["arbiter"]["model"] == "openai/custom"


def test_config_models_missing_config(tmp_path, monkeypatch):
    project = tmp_path / "empty"
    project.mkdir()
    monkeypatch.chdir(tmp_path)

    result = runner.invoke(app, ["config", "models", "--project-root", str(project)])
    assert result.exit_code == 0  # returns after printing the FileNotFoundError message
