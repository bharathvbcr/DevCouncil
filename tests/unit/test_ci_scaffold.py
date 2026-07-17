import yaml
from typer.testing import CliRunner

from devcouncil.cli.commands.init import initialize_project
from devcouncil.cli.main import app
from devcouncil.repo.ci_scaffold import detect_stacks, render_workflow, scaffold_ci

runner = CliRunner()


def _init(tmp_path):
    initialize_project(tmp_path, quiet=True, with_map=False, with_skills=False)


def _set_commands(tmp_path, commands: dict[str, list[str]]) -> None:
    config_path = tmp_path / ".devcouncil" / "config.yaml"
    config = yaml.safe_load(config_path.read_text(encoding="utf-8"))
    config["commands"] = commands
    config_path.write_text(yaml.dump(config, sort_keys=False), encoding="utf-8")


def test_detect_stacks(tmp_path):
    (tmp_path / "pyproject.toml").write_text("[project]\n", encoding="utf-8")
    assert detect_stacks(tmp_path) == {"python"}
    (tmp_path / "package.json").write_text("{}", encoding="utf-8")
    assert detect_stacks(tmp_path) == {"node", "python"}
    (tmp_path / "go.mod").write_text("module example.com/app\n\ngo 1.21\n", encoding="utf-8")
    assert detect_stacks(tmp_path) == {"go", "node", "python"}
    (tmp_path / "Cargo.toml").write_text("[package]\nname = 'x'\n", encoding="utf-8")
    assert detect_stacks(tmp_path) == {"go", "node", "python", "rust"}
    # uv.lock alone implies a Python stack for scaffolding purposes
    only_uv = tmp_path / "uv-only"
    only_uv.mkdir()
    (only_uv / "uv.lock").write_text("", encoding="utf-8")
    assert detect_stacks(only_uv) == {"python"}


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


def test_render_go_stack(tmp_path):
    (tmp_path / "go.mod").write_text("module example.com/app\n\ngo 1.21\n", encoding="utf-8")
    _init(tmp_path)
    _set_commands(
        tmp_path,
        {"test": ["go test ./..."], "lint": ["go vet ./..."], "typecheck": []},
    )
    doc = yaml.safe_load(render_workflow(tmp_path))
    steps = doc["jobs"]["checks"]["steps"]
    names = [s.get("name", "") for s in steps]
    runs = [s.get("run", "") for s in steps]
    assert any("Set up Go" in n for n in names)
    assert "go test ./..." in runs
    assert "go vet ./..." in runs
    assert any("govulncheck" in n for n in names)
    assert not any("Set up Python" in n for n in names)


def test_render_rust_stack(tmp_path):
    (tmp_path / "Cargo.toml").write_text("[package]\nname = 'x'\n", encoding="utf-8")
    _init(tmp_path)
    _set_commands(
        tmp_path,
        {"test": ["cargo test"], "lint": ["cargo clippy"], "typecheck": []},
    )
    doc = yaml.safe_load(render_workflow(tmp_path))
    steps = doc["jobs"]["checks"]["steps"]
    names = [s.get("name", "") for s in steps]
    runs = [s.get("run", "") for s in steps]
    assert any("Set up Rust" in n for n in names)
    assert "cargo test" in runs
    assert "cargo clippy" in runs
    assert any("cargo audit" in n for n in names)
    assert not any("Set up Python" in n for n in names)


def test_render_filters_go_commands_on_python_stack(tmp_path):
    (tmp_path / "pyproject.toml").write_text("[project]\n", encoding="utf-8")
    _init(tmp_path)
    _set_commands(
        tmp_path,
        {"test": ["pytest", "go test ./..."], "lint": ["ruff check ."], "typecheck": []},
    )
    doc = yaml.safe_load(render_workflow(tmp_path))
    names = [s.get("name", "") for s in doc["jobs"]["checks"]["steps"]]
    assert any("pytest" in n for n in names)
    assert not any("go test" in n for n in names)


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
