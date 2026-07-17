import json
import webbrowser
from pathlib import Path

from typer.testing import CliRunner

from devcouncil.cli.commands import map as map_cmd
from devcouncil.cli.main import app
from devcouncil.indexing.repo_mapper import RepoMap, RepoSubsystem
from devcouncil.indexing.viz import (
    graph_path,
    render_graph_html,
    sample_demo_graph,
    write_graph_demo,
)
from devcouncil.integrations.code_review_graph import CodeReviewGraphContext


runner = CliRunner()


def _repo_map(
    *,
    important_files: list[str] | None = None,
    subsystems: list[RepoSubsystem] | None = None,
) -> RepoMap:
    return RepoMap(
        languages=["python"],
        frameworks=[],
        package_managers=[],
        test_commands=["pytest"],
        important_files=important_files if important_files is not None else [],
        candidate_files=[],
        subsystems=subsystems if subsystems is not None else [],
    )


def test_important_surfaces_prefers_subsystems_and_caps_at_six():
    repo_map = _repo_map(
        important_files=["README.md"],
        subsystems=[
            RepoSubsystem(
                area=f"area{index}",
                summary=f"Subsystem {index}",
                entry_points=[],
                critical_files=[],
            )
            for index in range(1, 8)
        ],
    )

    surfaces = map_cmd._important_surfaces(repo_map)

    assert len(surfaces) == 6
    assert surfaces[0] == "1. `area1/` — Subsystem 1"
    assert surfaces[-1] == "6. `area6/` — Subsystem 6"


def test_important_surfaces_falls_back_to_files_and_then_map_hint():
    files_only = _repo_map(important_files=[f"src/file{index}.py" for index in range(1, 8)])
    empty = _repo_map()

    file_surfaces = map_cmd._important_surfaces(files_only)
    empty_surfaces = map_cmd._important_surfaces(empty)

    assert file_surfaces == [
        "1. `src/file1.py`",
        "2. `src/file2.py`",
        "3. `src/file3.py`",
        "4. `src/file4.py`",
        "5. `src/file5.py`",
        "6. `src/file6.py`",
    ]
    assert empty_surfaces == ["1. See `.devcouncil/repo_map.json` for the file index."]


def test_agent_guide_text_includes_relative_map_path_and_surfaces(tmp_path):
    repo_map = _repo_map(
        subsystems=[
            RepoSubsystem(
                area="src",
                summary="Application source",
                entry_points=["src/app.py"],
                critical_files=["src/app.py"],
            )
        ]
    )

    text = map_cmd._agent_guide_text(tmp_path / ".devcouncil" / "repo_map.json", tmp_path, repo_map)

    assert text.startswith(map_cmd.AGENT_GUIDE_MARKER)
    assert "Repo map: `.devcouncil/repo_map.json`" in text
    assert "1. `src/` — Application source" in text
    assert "trust the source and regenerate the map" in text


def test_agent_guide_text_keeps_absolute_map_path_outside_repo(tmp_path):
    outside_map = tmp_path.parent / "outside-repo-map.json"

    text = map_cmd._agent_guide_text(outside_map, tmp_path / "repo", _repo_map())

    assert f"Repo map: `{outside_map}`" in text


def test_write_agent_guides_preserves_unmanaged_files_and_updates_managed(tmp_path):
    repo_map = _repo_map(important_files=["src/app.py"])
    unmanaged = tmp_path / "AGENTS.md"
    managed = tmp_path / "CLAUDE.md"
    unmanaged.write_text("# Human notes\n", encoding="utf-8")
    managed.write_text(f"{map_cmd.AGENT_GUIDE_MARKER}\nold\n", encoding="utf-8")

    map_cmd._write_agent_guides(tmp_path, tmp_path / ".devcouncil" / "repo_map.json", repo_map)

    assert unmanaged.read_text(encoding="utf-8") == "# Human notes\n"
    managed_text = managed.read_text(encoding="utf-8")
    assert managed_text.startswith(map_cmd.AGENT_GUIDE_MARKER)
    assert "1. `src/app.py`" in managed_text
    assert managed_text.endswith("\n")


def test_write_agent_guides_creates_missing_files(tmp_path):
    repo_map = _repo_map(important_files=["src/app.py"])

    map_cmd._write_agent_guides(tmp_path, tmp_path / ".devcouncil" / "repo_map.json", repo_map)

    assert (tmp_path / "AGENTS.md").exists()
    assert (tmp_path / "CLAUDE.md").exists()


def test_generate_map_artifacts_writes_repo_map_and_guides_without_graph_context(tmp_path, monkeypatch):
    repo_map = _repo_map(important_files=["src/app.py"])
    captured = {}

    class FakeRepoMapper:
        def __init__(self, root: Path):
            captured["root"] = root

        def map_repo(self, goal: str, *, scan_dependencies: bool = False) -> RepoMap:
            captured["goal"] = goal
            captured["scan_dependencies"] = scan_dependencies
            return repo_map

    class FakeGraphAdapter:
        def __init__(self, root: Path):
            captured["graph_root"] = root

        def get_context(self) -> CodeReviewGraphContext:
            return CodeReviewGraphContext(available=False, summary="disabled")

    monkeypatch.setattr(map_cmd, "RepoMapper", FakeRepoMapper)
    monkeypatch.setattr(map_cmd, "CodeReviewGraphAdapter", FakeGraphAdapter)

    output = tmp_path / ".devcouncil" / "repo_map.json"
    result = map_cmd.generate_map_artifacts(tmp_path, output, "ship auth", scan_dependencies=True)

    assert result == repo_map
    assert captured == {
        "root": tmp_path,
        "goal": "ship auth",
        "scan_dependencies": True,
        "graph_root": tmp_path,
    }
    assert json.loads(output.read_text(encoding="utf-8"))["languages"] == ["python"]
    assert (tmp_path / "AGENTS.md").exists()
    assert not (tmp_path / ".devcouncil" / "code_review_graph_context.json").exists()


def test_generate_map_artifacts_writes_available_graph_context(tmp_path, monkeypatch):
    repo_map = _repo_map()
    context = CodeReviewGraphContext(
        available=True,
        summary="graph ready",
        changed_files=["src/app.py"],
        impacted_files=["src/routes.py"],
        related_tests=["tests/test_app.py"],
    )

    class FakeRepoMapper:
        def __init__(self, root: Path):
            self.root = root

        def map_repo(self, goal: str, *, scan_dependencies: bool = False) -> RepoMap:
            return repo_map

    class FakeGraphAdapter:
        def __init__(self, root: Path):
            self.root = root

        def get_context(self) -> CodeReviewGraphContext:
            return context

    monkeypatch.setattr(map_cmd, "RepoMapper", FakeRepoMapper)
    monkeypatch.setattr(map_cmd, "CodeReviewGraphAdapter", FakeGraphAdapter)

    output = tmp_path / "nested" / "repo_map.json"
    map_cmd.generate_map_artifacts(tmp_path, output)

    graph_output = tmp_path / "nested" / "code_review_graph_context.json"
    graph_data = json.loads(graph_output.read_text(encoding="utf-8"))
    assert graph_data["available"] is True
    assert graph_data["impacted_files"] == ["src/routes.py"]


def test_map_cli_entry_initializes_and_prints_json(tmp_path, monkeypatch):
    repo_map = _repo_map(important_files=["src/app.py"])
    captured = {}

    def fake_initialize_project(root: Path, *, quiet: bool, with_map: bool):
        captured["initialize"] = (root, quiet, with_map)

    def fake_generate_map_artifacts(
        root: Path,
        output: Path,
        goal: str = "",
        *,
        scan_dependencies: bool = False,
    ) -> RepoMap:
        captured["generate"] = (root, output, goal, scan_dependencies)
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(repo_map.model_dump_json(indent=2), encoding="utf-8")
        return repo_map

    monkeypatch.setattr(map_cmd, "initialize_project", fake_initialize_project)
    monkeypatch.setattr(map_cmd, "get_db", lambda root: object())
    monkeypatch.setattr(map_cmd, "generate_map_artifacts", fake_generate_map_artifacts)

    result = runner.invoke(
        app,
        [
            "map",
            "ship auth",
            "--project-root",
            str(tmp_path),
            "--output",
            "out/repo_map.json",
            "--scan-deps",
        ],
    )

    assert result.exit_code == 0
    assert json.loads(result.stdout)["important_files"] == ["src/app.py"]
    assert captured["initialize"] == (tmp_path.resolve(), True, False)
    assert captured["generate"] == (
        tmp_path.resolve(),
        tmp_path.resolve() / "out" / "repo_map.json",
        "ship auth",
        True,
    )


def test_map_cli_exits_when_db_initialization_fails(tmp_path, monkeypatch):
    monkeypatch.setattr(map_cmd, "initialize_project", lambda *args, **kwargs: None)
    monkeypatch.setattr(map_cmd, "get_db", lambda root: None)

    result = runner.invoke(app, ["map", "--project-root", str(tmp_path)])

    assert result.exit_code == 1


def test_viz_helpers_render_and_write_demo_without_opening_browser(tmp_path):
    demo = sample_demo_graph()
    html = render_graph_html(demo, file_level=True)

    output = write_graph_demo(tmp_path, open_browser=False)

    assert graph_path(tmp_path) == tmp_path / ".devcouncil" / "graph" / "code-graph.json"
    assert demo["nodes"][0]["id"] == "src/app/main.py"
    assert "<!DOCTYPE html>" in html
    assert '"source": "src/app/main.py"' in html
    assert output == tmp_path / ".devcouncil" / "graph" / "demo.html"
    assert "ForceGraph()" in output.read_text(encoding="utf-8")


def test_write_graph_demo_opens_browser_when_requested(tmp_path, monkeypatch):
    opened = []
    monkeypatch.setattr(webbrowser, "open", lambda uri: opened.append(uri))

    output = write_graph_demo(tmp_path, open_browser=True)

    assert opened == [output.resolve().as_uri()]


def test_graph_demo_cli_writes_demo_path(tmp_path, monkeypatch):
    monkeypatch.setattr(webbrowser, "open", lambda uri: None)

    result = runner.invoke(app, ["graph", "demo", "--no-open", "--project-root", str(tmp_path)])

    assert result.exit_code == 0
    assert str(tmp_path / ".devcouncil" / "graph" / "demo.html") in result.stdout
