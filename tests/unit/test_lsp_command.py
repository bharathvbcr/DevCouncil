import json

from typer.testing import CliRunner

from devcouncil.cli.main import app


def test_lsp_inspect_json_compact(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    tmp_path.mkdir(parents=True, exist_ok=True)
    (tmp_path / "main.py").write_text("print('hi')\n", encoding="utf-8")
    runner = CliRunner()
    result = runner.invoke(app, ["lsp", "inspect", "--json", "--project-root", str(tmp_path)])
    assert result.exit_code == 0, result.output
    payload = json.loads(result.stdout)
    assert payload["mode"] == "detection-only"
    assert "servers_detected" in payload
    assert "note" in payload
    assert "does not run an LSP client" in payload["note"]
