from pathlib import Path
import subprocess
from typer.testing import CliRunner

from devcouncil.cli.main import app
from devcouncil.storage.db import reset_db_cache

runner = CliRunner()


def _setup_go_repo(tmp_path: Path, monkeypatch) -> Path:
    reset_db_cache()
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
    
    # Run dev init
    runner.invoke(app, ["init"])
    return tmp_path


def test_cli_go_manual_executor_fails(tmp_path, monkeypatch):
    _setup_go_repo(tmp_path, monkeypatch)
    
    res = runner.invoke(app, ["go", "implement something", "--executor", "manual"])
    assert res.exit_code != 0
    assert "requires an automated executor" in res.output


def test_cli_go_dry_run_no_tasks(tmp_path, monkeypatch):
    _setup_go_repo(tmp_path, monkeypatch)
    
    # Mock planning flow to return empty list
    from devcouncil.cli.commands import plan as plan_command
    async def fake_run_plan_flow(*args, **kwargs):
        return []
    monkeypatch.setattr(plan_command, "run_plan_flow", fake_run_plan_flow)
    
    res = runner.invoke(app, ["go", "implement something", "--executor", "native", "--dry-run"])
    assert res.exit_code != 0
    assert "Planning did not produce any approved tasks" in res.output
