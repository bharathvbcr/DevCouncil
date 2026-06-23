import yaml
from typer.testing import CliRunner

from devcouncil.cli.commands.init import initialize_project
from devcouncil.cli.main import app
from devcouncil.repo.ci_scaffold import detect_stacks, render_workflow, scaffold_ci

runner = CliRunner()


def _init(tmp_path):
    initialize_project(tmp_path, quiet=True, with_map=False, with_skills=False)


def test_detect_stacks(tmp_path):
    (tmp_path / "pyproject.toml").write_text("[project]\n", encoding="utf-8")
    assert detect_stacks(tmp_path) == {"python"}
    (tmp_path / "package.json").write_text("{}", encoding="utf-8")
    assert detect_stacks(tmp_path) == {"node", "python"}


def test_render_filters_commands_to_python_stack(tmp_path):
    (tmp_path / "pyproject.toml").write_text("[project]\n", encoding="utf-8")
    _init(tmp_path)
    doc = yaml.safe_load(render_workflow(tmp_path))
    names = [s.get("name", "") for s in doc["jobs"]["checks"]["steps"]]
    assert any("Set up Python" in n for n in names)
    assert not any("Set up Node" in n for n in names)
    # stack-aware init writes only Python commands (ruff/mypy/pytest); no node tools
    assert any("ruff" in n for n in names)
    assert not any("eslint" in n for n in names)
    assert not any("tsc" in n for n in names)


def test_render_node_stack(tmp_path):
    (tmp_path / "package.json").write_text("{}", encoding="utf-8")
    _init(tmp_path)
    doc = yaml.safe_load(render_workflow(tmp_path))
    names = [s.get("name", "") for s in doc["jobs"]["checks"]["steps"]]
    assert any("Set up Node" in n for n in names)
    assert any("eslint" in n for n in names)
    assert not any("flake8" in n for n in names)


def test_scaffold_ci_writes_and_does_not_clobber(tmp_path):
    (tmp_path / "pyproject.toml").write_text("[project]\n", encoding="utf-8")
    _init(tmp_path)
    target = scaffold_ci(tmp_path)
    assert target is not None and target.exists()
    # Valid YAML
    yaml.safe_load(target.read_text(encoding="utf-8"))
    # Does not overwrite without force
    assert scaffold_ci(tmp_path) is None
    # Force overwrites
    assert scaffold_ci(tmp_path, force=True) == target


def test_cli_scaffold_ci_requires_init(tmp_path):
    result = runner.invoke(app, ["scaffold-ci", "--project-root", str(tmp_path)])
    assert result.exit_code == 1


def test_cli_scaffold_ci_writes(tmp_path):
    (tmp_path / "pyproject.toml").write_text("[project]\n", encoding="utf-8")
    _init(tmp_path)
    result = runner.invoke(app, ["scaffold-ci", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert (tmp_path / ".github" / "workflows" / "devcouncil.yml").exists()
