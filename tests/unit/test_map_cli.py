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


def test_cli_map_rejects_missing_project_root(tmp_path, monkeypatch):
    """Missing --project-root must exit non-zero without mkdir'ing an empty project."""
    missing = tmp_path / "does-not-exist"
    assert not missing.exists()
    res = runner.invoke(app, ["map", "--project-root", str(missing), "--no-wiki"])
    assert res.exit_code != 0
    assert not missing.exists()
    combined = (res.stdout or "") + (res.stderr or "")
    assert "does not exist" in combined.lower() or "does not exist" in str(res)


def test_cli_map_survives_broken_stdout_pipe(tmp_path, monkeypatch):
    """A consumer closing stdout early (dev map | head) must not fail the map."""
    _setup_map_repo(tmp_path, monkeypatch)
    import typer as typer_mod

    real_echo = typer_mod.echo
    def _epipe_echo(*args, **kwargs):
        raise BrokenPipeError(32, "Broken pipe")
    monkeypatch.setattr("devcouncil.cli.commands.map.typer.echo", _epipe_echo)
    try:
        res = runner.invoke(app, ["map", "--no-wiki"])
    finally:
        monkeypatch.setattr("devcouncil.cli.commands.map.typer.echo", real_echo)
    assert res.exit_code == 0, res.output
    assert (tmp_path / ".devcouncil" / "repo_map.json").is_file()


def test_run_entry_exits_quietly_on_broken_pipe(monkeypatch):
    """The console-script wrapper converts BrokenPipeError into exit 141."""
    import os

    import pytest

    from devcouncil.cli import main as main_mod

    def _epipe_app():
        raise BrokenPipeError(32, "Broken pipe")

    monkeypatch.setattr(main_mod, "app", _epipe_app)
    # Neutralize the devnull fd swap — under pytest's capture it would clobber
    # the captured fd 1; in a real console it is the documented SIGPIPE recipe.
    monkeypatch.setattr(os, "open", lambda *a, **k: 99)
    monkeypatch.setattr(os, "dup2", lambda *a, **k: None)
    with pytest.raises(SystemExit) as exc:
        main_mod.run_cli()
    assert exc.value.code == 141


def test_cli_map_warns_when_goal_is_a_directory(tmp_path, monkeypatch):
    """`dev map /other/repo` maps CWD with the path as goal — warn about intent."""
    repo = tmp_path / "repo"
    repo.mkdir()
    _setup_map_repo(repo, monkeypatch)
    other = tmp_path / "other"
    other.mkdir()
    res = runner.invoke(app, ["map", str(other), "--no-wiki"])
    assert res.exit_code == 0, res.output
    combined = res.output + str(getattr(res, "stderr", ""))
    assert "--project-root" in combined
