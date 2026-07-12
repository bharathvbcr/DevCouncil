import json
from pathlib import Path
from typer.testing import CliRunner

from devcouncil.cli.main import app

runner = CliRunner()


def _setup_wiki_env(tmp_path: Path, monkeypatch) -> Path:
    monkeypatch.chdir(tmp_path)
    runner.invoke(app, ["init"])
    return tmp_path


def test_cli_wiki_install_action(tmp_path, monkeypatch):
    root = _setup_wiki_env(tmp_path, monkeypatch)
    
    # Create an empty .gitignore to verify update
    (root / ".gitignore").write_text(".devcouncil/\n", encoding="utf-8")

    res = runner.invoke(app, ["wiki", "install-action"])
    assert res.exit_code == 0
    assert "Wrote" in res.output
    
    wf = root / ".github" / "workflows" / "devcouncil-wiki-update.yml"
    assert wf.is_file()
    assert "DevCouncil codebase wiki" in wf.read_text(encoding="utf-8")
    
    gitignore_content = (root / ".gitignore").read_text(encoding="utf-8")
    assert "!.devcouncil/knowledge/okf/wiki/" in gitignore_content


def test_cli_wiki_status_missing(tmp_path, monkeypatch):
    _setup_wiki_env(tmp_path, monkeypatch)
    
    res = runner.invoke(app, ["wiki", "status"])
    assert res.exit_code != 0
    assert "No wiki found" in res.output


def test_cli_wiki_status_json_missing(tmp_path, monkeypatch):
    _setup_wiki_env(tmp_path, monkeypatch)
    
    res = runner.invoke(app, ["wiki", "status", "--json"])
    assert res.exit_code != 0
    data = json.loads(res.output)
    assert data["exists"] is False


def test_cli_wiki_update_and_read(tmp_path, monkeypatch):
    root = _setup_wiki_env(tmp_path, monkeypatch)
    
    # Create sample files to build a map
    (root / "pyproject.toml").write_text("[project]\nname='test'\n", encoding="utf-8")
    (root / "src").mkdir()
    (root / "src" / "app.py").write_text("def run(): pass\n", encoding="utf-8")
    
    # Run dev map to ensure map exists
    runner.invoke(app, ["map", "test"])
    
    # Update wiki
    res = runner.invoke(app, ["wiki", "update", "--no-llm"])
    assert res.exit_code == 0
    assert "Wiki updated" in res.output or "Wiki is up to date" in res.output
    
    # Check wiki status
    res_status = runner.invoke(app, ["wiki", "status", "--json"])
    assert res_status.exit_code == 0
    data_status = json.loads(res_status.output)
    assert data_status["exists"] is True
    assert data_status["pages"] > 0
    
    # Read wiki list
    res_list = runner.invoke(app, ["wiki", "read"])
    assert res_list.exit_code == 0
    assert "index.md" in res_list.output
    
    # Read specific wiki page
    res_page = runner.invoke(app, ["wiki", "read", "--page", "index.md"])
    assert res_page.exit_code == 0
    assert "codebase wiki" in res_page.output.lower()
    
    # Query wiki pages
    res_query = runner.invoke(app, ["wiki", "read", "--query", "app"])
    assert res_query.exit_code == 0
