"""Unit tests for setup command helpers."""

import sys
from pathlib import Path
from unittest.mock import MagicMock

import pytest
import yaml

from devcouncil.cli.commands import setup as setup_cmd


def _init_config(tmp_path: Path) -> Path:
    dev = tmp_path / ".devcouncil"
    dev.mkdir()
    config = dev / "config.yaml"
    config.write_text(
        yaml.safe_dump(
            {
                "project": {"name": "test"},
                "models": {"provider": "openrouter", "roles": {}},
            }
        ),
        encoding="utf-8",
    )
    return config


def test_write_local_secret_rejects_newlines(tmp_path):
    _init_config(tmp_path)
    with pytest.raises(ValueError, match="newlines"):
        setup_cmd._write_local_secret(tmp_path, "API_KEY", "bad\nkey")


def test_write_local_secret_persists_sorted(tmp_path):
    _init_config(tmp_path)
    setup_cmd._write_local_secret(tmp_path, "B_KEY", "b")
    setup_cmd._write_local_secret(tmp_path, "A_KEY", "a")
    text = (tmp_path / ".devcouncil" / "secrets.env").read_text(encoding="utf-8")
    assert "A_KEY=a" in text
    assert "B_KEY=b" in text


def test_set_model_provider_same_provider_no_change_message(tmp_path, capsys):
    _init_config(tmp_path)
    setup_cmd._set_model_provider(tmp_path, "openrouter")
    captured = capsys.readouterr()
    assert "Updated model provider" not in captured.out


def test_set_model_roles_noop_when_empty(tmp_path):
    _init_config(tmp_path)
    before = (tmp_path / ".devcouncil" / "config.yaml").read_text(encoding="utf-8")
    setup_cmd._set_model_roles(tmp_path, model=None, role_models=None)
    after = (tmp_path / ".devcouncil" / "config.yaml").read_text(encoding="utf-8")
    assert before == after


def test_configure_api_key_ollama_skips(tmp_path, capsys):
    _init_config(tmp_path)
    (tmp_path / ".devcouncil" / "config.yaml").write_text(
        yaml.safe_dump({"project": {"name": "t"}, "models": {"provider": "ollama"}}),
        encoding="utf-8",
    )
    setup_cmd._configure_api_key(tmp_path, api_key=None, skip_api_key=False)
    assert "required" in capsys.readouterr().out


def test_configure_api_key_env_already_set(tmp_path, monkeypatch, capsys):
    _init_config(tmp_path)
    monkeypatch.setenv("OPENROUTER_API_KEY", "from-env")
    setup_cmd._configure_api_key(tmp_path, api_key=None, skip_api_key=False)
    assert "already set in the environment" in capsys.readouterr().out


def test_configure_api_key_local_secret_already_set(tmp_path, capsys):
    _init_config(tmp_path)
    setup_cmd._write_local_secret(tmp_path, "OPENROUTER_API_KEY", "local")
    setup_cmd._configure_api_key(tmp_path, api_key=None, skip_api_key=False)
    assert "already set in .devcouncil/secrets.env" in capsys.readouterr().out


def test_configure_api_key_writes_provided_key(tmp_path, capsys):
    _init_config(tmp_path)
    setup_cmd._configure_api_key(tmp_path, api_key="secret-key", skip_api_key=False)
    assert "Saved OPENROUTER_API_KEY" in capsys.readouterr().out
    assert "secret-key" in (tmp_path / ".devcouncil" / "secrets.env").read_text(encoding="utf-8")


def test_configure_api_key_skip_flag(tmp_path, capsys):
    _init_config(tmp_path)
    setup_cmd._configure_api_key(tmp_path, api_key=None, skip_api_key=True)
    assert "Skipped OPENROUTER_API_KEY setup" in capsys.readouterr().out


def test_configure_api_key_non_tty_warns(tmp_path, monkeypatch, capsys):
    _init_config(tmp_path)
    monkeypatch.setattr(setup_cmd.sys.stdin, "isatty", lambda: False)
    setup_cmd._configure_api_key(tmp_path, api_key=None, skip_api_key=False)
    assert "is not set" in capsys.readouterr().out


def test_configure_api_key_interactive_prompt(tmp_path, monkeypatch, capsys):
    _init_config(tmp_path)
    monkeypatch.setattr(setup_cmd.sys.stdin, "isatty", lambda: True)
    monkeypatch.setattr(setup_cmd.typer, "prompt", lambda *a, **k: "typed-key")
    setup_cmd._configure_api_key(tmp_path, api_key=None, skip_api_key=False)
    out = capsys.readouterr().out
    assert "Saved OPENROUTER_API_KEY" in out


def test_configure_api_key_interactive_skip_empty(tmp_path, monkeypatch, capsys):
    _init_config(tmp_path)
    monkeypatch.setattr(setup_cmd.sys.stdin, "isatty", lambda: True)
    monkeypatch.setattr(setup_cmd.typer, "prompt", lambda *a, **k: "")
    setup_cmd._configure_api_key(tmp_path, api_key=None, skip_api_key=False)
    assert "Skipped OPENROUTER_API_KEY setup" in capsys.readouterr().out


def test_configure_vertexai_settings_non_vertex_provider(tmp_path, capsys):
    _init_config(tmp_path)
    setup_cmd._configure_vertexai_settings(tmp_path, "openrouter", "proj", "us-central1")
    assert capsys.readouterr().out == ""


def test_configure_vertexai_settings_warns_missing_project(tmp_path, capsys):
    _init_config(tmp_path)
    (tmp_path / ".devcouncil" / "config.yaml").write_text(
        yaml.safe_dump({"project": {"name": "t"}, "models": {"provider": "vertexai"}}),
        encoding="utf-8",
    )
    setup_cmd._configure_vertexai_settings(tmp_path, "vertexai", None, None)
    assert "VERTEXAI_PROJECT is not set" in capsys.readouterr().out


def test_configure_vertexai_settings_writes_project_and_location(tmp_path, capsys):
    _init_config(tmp_path)
    setup_cmd._configure_vertexai_settings(tmp_path, "vertexai", "my-proj", "global")
    out = capsys.readouterr().out
    assert "VERTEXAI_PROJECT" in out
    assert "VERTEXAI_LOCATION" in out


def test_configure_coding_cli_integrations_apply_failure(tmp_path, monkeypatch, capsys):
    _init_config(tmp_path)
    monkeypatch.setattr(setup_cmd.shutil, "which", lambda cmd: "/usr/bin/codex")
    monkeypatch.setattr(setup_cmd, "_configure", lambda tool, command, apply: False)
    monkeypatch.setattr(setup_cmd, "_configure_cursor", lambda root, apply: True)
    monkeypatch.setattr(setup_cmd, "_configure_grok", lambda root, apply: True)
    monkeypatch.setattr(setup_cmd, "_configure_opencode", lambda root, apply: True)
    monkeypatch.setattr(setup_cmd, "_configure_antigravity", lambda root, apply: True)
    monkeypatch.setattr(setup_cmd, "_configure_warp", lambda root, apply: True)
    monkeypatch.setattr(setup_cmd, "_configure_native_hooks", lambda root, scope, apply: None)

    with pytest.raises((SystemExit, Exception)):
        setup_cmd._configure_coding_cli_integrations(tmp_path, apply=True, gemini_scope="project")


def test_prompt_for_first_run_integrations_non_tty_returns_false(tmp_path, monkeypatch):
    monkeypatch.setattr(setup_cmd, "_is_interactive_terminal", lambda: False)
    assert setup_cmd._prompt_for_first_run_integrations(tmp_path, apply=True, gemini_scope="project") is False


def test_prompt_for_first_run_integrations_user_declines(tmp_path, monkeypatch, capsys):
    monkeypatch.setattr(setup_cmd, "_is_interactive_terminal", lambda: True)
    monkeypatch.setattr(setup_cmd.typer, "confirm", lambda *a, **k: False)
    assert setup_cmd._prompt_for_first_run_integrations(tmp_path, apply=True, gemini_scope="project") is False
    assert "Skipped coding CLI integration setup" in capsys.readouterr().out
