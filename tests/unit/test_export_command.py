import json

from devcouncil.cli.commands.init import initialize_project
from typer.testing import CliRunner

from devcouncil.cli.main import app


def test_export_writes_state_json(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    initialize_project(tmp_path, quiet=True)
    runner = CliRunner()
    result = runner.invoke(app, ["export"])
    assert result.exit_code == 0, result.output
    out_path = tmp_path / ".devcouncil" / "export" / "state.json"
    assert out_path.is_file()
    payload = json.loads(out_path.read_text(encoding="utf-8"))
    assert payload["initialized"] is True
    assert "requirements" in payload
    assert "tasks" in payload
    assert "gaps" in payload


def test_export_json_stdout(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    initialize_project(tmp_path, quiet=True)
    runner = CliRunner()
    result = runner.invoke(app, ["export", "--json"])
    assert result.exit_code == 0, result.output
    payload = json.loads(result.stdout)
    assert payload["initialized"] is True
