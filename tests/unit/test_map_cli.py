import json
from pathlib import Path
import subprocess
from typer.testing import CliRunner

from devcouncil.cli.main import app

runner = CliRunner()


def _setup_map_repo(tmp_path: Path, monkeypatch) -> Path:
    monkeypatch.chdir(tmp_path)
    
    # Init git repo
    subprocess.run(["git", "init"], cwd=tmp_path, capture_output=True)
    subprocess.run(["git", "config", "user.email", "t@example.com"], cwd=tmp_path, capture_output=True)
    subprocess.run(["git", "config", "user.name", "Test"], cwd=tmp_path, capture_output=True)
    
    # Create simple file and commit
    a_py = tmp_path / "src" / "a.py"
    a_py.parent.mkdir(parents=True, exist_ok=True)
    a_py.write_text("def hello():\n    pass\n", encoding="utf-8")
    
    subprocess.run(["git", "add", "src/a.py"], cwd=tmp_path, capture_output=True)
    subprocess.run(["git", "commit", "-m", "init"], cwd=tmp_path, capture_output=True)
    
    # Pre-create AGENTS.md with marker
    agents_md = tmp_path / "AGENTS.md"
    agents_md.write_text("<!-- Managed by dev map: keep this file in sync with .devcouncil/repo_map.json. -->\n", encoding="utf-8")
    
    # Run dev init
    runner.invoke(app, ["init"])
    return tmp_path


def test_cli_map_default(tmp_path, monkeypatch):
    _setup_map_repo(tmp_path, monkeypatch)
    
    res = runner.invoke(app, ["map"])
    assert res.exit_code == 0
    assert "map completed" in res.output or "wrote" in res.output.lower()
    
    # Verify repo_map.json exists
    map_file = tmp_path / ".devcouncil" / "repo_map.json"
    assert map_file.exists()
    data = json.loads(map_file.read_text(encoding="utf-8"))
    assert any(f["path"] == "src/a.py" for f in data["files"])


def test_cli_map_with_goal_and_output(tmp_path, monkeypatch):
    _setup_map_repo(tmp_path, monkeypatch)
    
    custom_output = "custom_map.json"
    res = runner.invoke(app, ["map", "My task goal", "--output", custom_output])
    assert res.exit_code == 0
    assert (tmp_path / custom_output).exists()


def test_cli_map_no_liveness(tmp_path, monkeypatch):
    _setup_map_repo(tmp_path, monkeypatch)
    
    res = runner.invoke(app, ["map", "--no-liveness"])
    assert res.exit_code == 0
    
    map_file = tmp_path / ".devcouncil" / "repo_map.json"
    data = json.loads(map_file.read_text(encoding="utf-8"))
    # entry_roots and dead_symbol_candidates should be empty or not computed
    assert len(data.get("entry_roots", [])) == 0
