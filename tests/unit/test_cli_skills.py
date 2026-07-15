from typer.testing import CliRunner

from devcouncil.cli.main import app
from devcouncil.skills.registry import Skill

runner = CliRunner()


def test_cli_skills_list(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    runner.invoke(app, ["init"])
    
    # Mock registry functions on the command module namespace
    mock_skill = Skill(
        name="test-skill",
        title="Test Skill",
        description="A test skill",
        always=True,
        body="This is the body of the test skill."
    )
    
    from devcouncil.cli.commands import skills as skills_cmd
    monkeypatch.setattr(skills_cmd, "load_skills", lambda project_root: [mock_skill])
    monkeypatch.setattr(skills_cmd, "select_skills", lambda goal, project_root: [mock_skill])
    
    res = runner.invoke(app, ["skills"])
    assert res.exit_code == 0
    assert "test-skill" in res.output
    assert "A test skill" in res.output


def test_cli_skills_show(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    runner.invoke(app, ["init"])
    
    mock_skill = Skill(
        name="test-skill",
        title="Test Skill",
        description="A test skill",
        body="This is the body of the test skill."
    )
    from devcouncil.cli.commands import skills as skills_cmd
    monkeypatch.setattr(skills_cmd, "get_skill", lambda name, project_root: mock_skill if name == "test-skill" else None)
    
    res = runner.invoke(app, ["skills", "show", "test-skill"])
    assert res.exit_code == 0
    assert "test-skill" in res.output
    assert "This is the body of the test skill." in res.output
    
    res_err = runner.invoke(app, ["skills", "show", "unknown-skill"])
    assert res_err.exit_code != 0
    assert "no skill named" in res_err.output.lower()


def test_cli_skills_scaffold(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    runner.invoke(app, ["init"])
    
    mock_skill = Skill(
        name="test-skill",
        title="Test Skill",
        description="A test skill",
        body="This is the body of the test skill."
    )
    
    from devcouncil.cli.commands import skills as skills_cmd
    monkeypatch.setattr(skills_cmd, "load_skills", lambda project_root: [mock_skill])
    monkeypatch.setattr(skills_cmd, "select_skills", lambda goal, project_root: [mock_skill])
    
    written_path = tmp_path / ".claude" / "skills" / "test-skill" / "SKILL.md"
    monkeypatch.setattr(skills_cmd, "scaffold_skills", lambda project_root, chosen: [written_path])
    
    res = runner.invoke(app, ["skills", "scaffold", "--all"])
    assert res.exit_code == 0
    assert "test-skill" in res.output.lower()
