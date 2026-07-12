from pathlib import Path
from typer.testing import CliRunner

from devcouncil.cli.main import app

runner = CliRunner()


def _setup_doctor_repo(tmp_path: Path, monkeypatch) -> Path:
    monkeypatch.chdir(tmp_path)
    runner.invoke(app, ["init"])
    return tmp_path


def test_cli_doctor_default(tmp_path, monkeypatch):
    _setup_doctor_repo(tmp_path, monkeypatch)
    
    res = runner.invoke(app, ["doctor"])
    assert res.exit_code == 0
    assert "DevCouncil Doctor Check" in res.output
    assert "Git" in res.output
    assert "uv" in res.output


def test_cli_doctor_custom_config(tmp_path, monkeypatch):
    _setup_doctor_repo(tmp_path, monkeypatch)
    
    # Write custom config.yaml with ollama provider
    config_file = tmp_path / ".devcouncil" / "config.yaml"
    config_file.write_text("""
models:
  provider: ollama
  roles:
    orchestrator:
      model: qwen2.5-coder
""", encoding="utf-8")
    
    # Mock network calls to ollama
    from devcouncil.cli.commands import doctor
    monkeypatch.setattr(doctor, "_probe_ollama", lambda url: (True, "Reachable mockup"))
    monkeypatch.setattr(doctor, "_probe_ollama_models", lambda url: (True, {"qwen2.5-coder"}))
    
    res = runner.invoke(app, ["doctor"])
    assert res.exit_code == 0
    assert "OLLAMA" in res.output
    assert "Reachable mockup" in res.output
