"""Wave-7: build_control and map_artifacts stable coverage."""

from __future__ import annotations

import json
import subprocess
from types import SimpleNamespace
from unittest.mock import MagicMock

import pytest

from devcouncil.codeintel.build_control import (
    BuildStatus,
    GraphBuildBusy,
    _write_status,
    graph_build_session,
    read_build_status,
    status_path,
)
from devcouncil.indexing.map_artifacts import (
    AGENT_GUIDE_MARKER,
    _important_surfaces,
    _wiki_index_rel,
    agent_guide_text,
    refresh_map_artifacts,
    write_agent_guides,
)
from devcouncil.indexing.repo_mapper import RepoMap, RepoSubsystem


# --- build_control ------------------------------------------------------------


def test_read_write_build_status(tmp_path):
    assert read_build_status(tmp_path).state == "idle"
    status = BuildStatus(build_id="b1", state="building", phase="x", completed=1, total=2)
    _write_status(tmp_path, status)
    assert status_path(tmp_path).is_file()
    loaded = read_build_status(tmp_path)
    assert loaded.build_id == "b1"
    assert loaded.completed == 1

    status_path(tmp_path).write_text("not-json", encoding="utf-8")
    assert read_build_status(tmp_path).state == "idle"


def test_graph_build_session_nested_and_busy(tmp_path):
    with graph_build_session(tmp_path):
        with graph_build_session(tmp_path):
            pass

    lease = MagicMock()
    # graph_build_session waits via acquire_with_retry (not a single acquire probe).
    lease.acquire_with_retry.return_value = False
    with pytest.raises(GraphBuildBusy):
        with graph_build_session(tmp_path, lease=lease):
            pass
    lease.acquire_with_retry.assert_called()


# --- build_worker -------------------------------------------------------------


# --- map_artifacts ------------------------------------------------------------


def _sample_map() -> RepoMap:
    return RepoMap(
        languages=["python"],
        frameworks=[],
        package_managers=[],
        test_commands=[],
        important_files=["src/a.py"],
        candidate_files=[],
        subsystems=[
            RepoSubsystem(
                area="src/pkg",
                summary="core",
                entry_points=["src/pkg/main.py"],
                critical_files=["src/pkg/main.py"],
            )
        ],
    )


def test_map_artifact_helpers(tmp_path):
    repo_map = _sample_map()
    surfaces = _important_surfaces(repo_map)
    assert "src/pkg" in surfaces[0]

    empty = RepoMap(
        languages=[],
        frameworks=[],
        package_managers=[],
        test_commands=[],
        important_files=[],
        candidate_files=[],
    )
    assert "repo_map.json" in _important_surfaces(empty)[0]

    empty.important_files = ["only.py"]
    assert "only.py" in _important_surfaces(empty)[0]

    assert _wiki_index_rel(tmp_path) is None

    text = agent_guide_text(tmp_path / ".devcouncil" / "repo_map.json", tmp_path, repo_map)
    assert AGENT_GUIDE_MARKER in text
    assert "Important surfaces" in text
    assert "DevCouncil loop" in text
    assert "devcouncil_checkout_task" in text
    assert ".cursor/skills/" in text

    write_agent_guides(tmp_path, tmp_path / ".devcouncil" / "repo_map.json", repo_map)
    assert (tmp_path / "AGENTS.md").is_file()
    write_agent_guides(tmp_path, tmp_path / ".devcouncil" / "repo_map.json", repo_map)
    (tmp_path / "CLAUDE.md").write_text("custom\n", encoding="utf-8")
    write_agent_guides(tmp_path, tmp_path / ".devcouncil" / "repo_map.json", repo_map)
    assert (tmp_path / "CLAUDE.md").read_text() == "custom\n"

    # A guide with invalid UTF-8 bytes must not crash map generation.
    (tmp_path / "CLAUDE.md").write_bytes(b"custom \xff guide\n")
    write_agent_guides(tmp_path, tmp_path / ".devcouncil" / "repo_map.json", repo_map)
    assert (tmp_path / "CLAUDE.md").read_bytes() == b"custom \xff guide\n"


def test_refresh_map_artifacts_fails_closed_without_a_kernel(tmp_path, monkeypatch):
    """There is no lean fallback any more.

    The Python engine used to write a fingerprint-stamped, `graph_degraded`
    map when the graph build failed. That map read fresh to every consumer that
    did not check the degraded flag. With the kernel as the only engine, a
    build that cannot run raises and leaves whatever was on disk untouched.
    """
    import pytest

    from devcouncil.cli.commands.init import initialize_project
    from devcouncil.devmap_engine import DevMapEngineError

    initialize_project(tmp_path, quiet=True, with_map=False, with_skills=False)
    out = tmp_path / ".devcouncil" / "repo_map.json"
    out.write_text('{"seed": true}', encoding="utf-8")

    monkeypatch.setattr(
        "devcouncil.devmap_engine.build_map",
        lambda *a, **k: (_ for _ in ()).throw(DevMapEngineError("no kernel")),
    )

    with pytest.raises(DevMapEngineError):
        refresh_map_artifacts(tmp_path, out, quiet=True)
    assert json.loads(out.read_text(encoding="utf-8")) == {"seed": True}


# --- doctor + map CLI branches -------------------------------------------------


def test_doctor_check_helpers_cover_new_rows(tmp_path, monkeypatch):
    from devcouncil.cli.commands import doctor as doctor_cmd
    from devcouncil.cli.commands.init import initialize_project

    initialize_project(tmp_path, quiet=True, with_map=False, with_skills=False)

    monkeypatch.setattr(
        doctor_cmd,
        "_repo_languages",
        lambda _r: {"python"},
    )
    monkeypatch.setattr(
        "devcouncil.codeintel.languages.grammar_status",
        lambda: {
            "languages": [{"language": "python", "missing_grammars": ["python"]}],
            "action": "install wheels",
        },
    )
    rows = doctor_cmd.check_grammar_coverage(tmp_path)
    assert rows and "Grammar coverage" in rows[0][0]

    monkeypatch.setattr(
        "devcouncil.codeintel.languages.grammar_status",
        lambda: {"languages": [{"language": "python", "missing_grammars": []}]},
    )
    ok_rows = doctor_cmd.check_grammar_coverage(tmp_path)
    assert ok_rows and "OK" in ok_rows[0][1]

    cand = SimpleNamespace(language="python", available=False)
    monkeypatch.setattr(
        "devcouncil.indexing.lsp.LspInspector",
        lambda _r: SimpleNamespace(server_candidates=lambda: [cand]),
    )
    lsp_rows = doctor_cmd.check_lsp_reference_confirmation(tmp_path)
    assert lsp_rows and "LSP servers" in lsp_rows[0][0]

    cfg_path = tmp_path / ".devcouncil" / "config.yaml"
    cfg_path.write_text("indexing:\n  not_a_real_key: 1\n", encoding="utf-8")
    unk = doctor_cmd.check_unknown_indexing_keys(tmp_path)
    assert isinstance(unk, list)

    floor = doctor_cmd.check_coverage_floor(tmp_path)
    assert floor

    mypy_rows = doctor_cmd.check_mypy_status(tmp_path)
    assert mypy_rows

    monitor = doctor_cmd.check_local_monitor_sampling(
        tmp_path,
        config=SimpleNamespace(
            models=SimpleNamespace(provider="ollama", roles={}),
            verification=SimpleNamespace(
                acceptance_checks=SimpleNamespace(
                    samples=1,
                    per_criterion=False,
                    resolved=lambda local: (1, 0, False),
                    unsafe_override_warnings=lambda local: ["warn"],
                )
            ),
        ),
    )
    assert isinstance(monitor, list)


def test_doctor_render_ollama_and_vertex_paths(tmp_path, monkeypatch):
    from typer.testing import CliRunner

    from devcouncil.cli.commands.init import initialize_project
    from devcouncil.cli.main import app

    initialize_project(tmp_path, quiet=True, with_map=False, with_skills=False)
    cfg = tmp_path / ".devcouncil" / "config.yaml"
    cfg.write_text(
        "models:\n  provider: ollama\n  roles:\n    planner_a:\n      model: tiny\n",
        encoding="utf-8",
    )

    monkeypatch.setattr(
        "devcouncil.cli.commands.doctor._probe_ollama",
        lambda _u: (True, "up"),
    )
    monkeypatch.setattr(
        "devcouncil.cli.commands.doctor._probe_ollama_models",
        lambda _u: (True, {"tiny"}),
    )
    monkeypatch.setenv("OLLAMA_NUM_CTX", "8192")
    runner = CliRunner()
    result = runner.invoke(app, ["doctor", "--project-root", str(tmp_path)])
    assert result.exit_code == 0

    cfg.write_text("models:\n  provider: vertexai\n  roles: {}\n", encoding="utf-8")
    monkeypatch.delenv("VERTEXAI_PROJECT", raising=False)
    monkeypatch.delenv("GOOGLE_CLOUD_PROJECT", raising=False)
    result2 = runner.invoke(app, ["doctor", "--project-root", str(tmp_path)])
    assert result2.exit_code == 0


def test_map_if_stale_skips_and_an_unbuildable_map_exits_one(tmp_path, monkeypatch):
    """`--if-stale` short-circuits on a fresh map; a build that cannot run exits 1.

    Replaces `test_map_if_stale_and_busy`, which patched
    `indexing.map_artifacts.refresh_map_artifacts` to raise `GraphBuildBusy`.
    The Rust kernel never calls that function, so the patch had no effect and
    the command exited 0.

    The "busy" half of the old contract is genuinely gone rather than moved: a
    concurrent build is no longer an error state. Measured after the temp-file
    races were fixed, 40 concurrent `dev map` runs against one repository all
    exit 0 with `PRAGMA integrity_check` clean. What must survive is the safety
    property underneath it — a map command that cannot produce a map must not
    exit 0 having quietly produced nothing.
    """
    from typer.testing import CliRunner

    import devcouncil.devmap_engine as engine
    from devcouncil.cli.commands.init import initialize_project
    from devcouncil.cli.main import app

    initialize_project(tmp_path, quiet=True, with_map=False, with_skills=False)
    out = tmp_path / ".devcouncil" / "repo_map.json"
    out.write_text("{}", encoding="utf-8")

    monkeypatch.setattr(
        "devcouncil.indexing.repo_mapper.RepoMapper.map_is_stale",
        lambda self, data: False,
    )
    runner = CliRunner()
    fresh = runner.invoke(
        app, ["map", "--if-stale", "--project-root", str(tmp_path), "-o", str(out)]
    )
    assert fresh.exit_code == 0
    assert "Map is fresh" in fresh.output

    def _unbuildable(*_args, **_kwargs):
        raise engine.DevMapEngineError("kernel unavailable")

    monkeypatch.setattr(engine, "build_map", _unbuildable)
    broken = runner.invoke(app, ["map", "--project-root", str(tmp_path), "-o", str(out)])
    assert broken.exit_code == 1


def test_doctor_ollama_missing_models_and_num_ctx_zero(tmp_path, monkeypatch):
    from typer.testing import CliRunner

    from devcouncil.cli.commands.init import initialize_project
    from devcouncil.cli.main import app

    initialize_project(tmp_path, quiet=True, with_map=False, with_skills=False)
    cfg = tmp_path / ".devcouncil" / "config.yaml"
    cfg.write_text(
        "models:\n  provider: ollama\n  roles:\n    planner_a:\n      model: missing-model\n",
        encoding="utf-8",
    )
    monkeypatch.setattr(
        "devcouncil.cli.commands.doctor._probe_ollama",
        lambda _u: (True, "up"),
    )
    monkeypatch.setattr(
        "devcouncil.cli.commands.doctor._probe_ollama_models",
        lambda _u: (True, set()),
    )
    monkeypatch.setenv("OLLAMA_NUM_CTX", "0")
    monkeypatch.setattr(
        "devcouncil.llm.provider.OllamaProvider._resolve_think",
        lambda: False,
    )
    runner = CliRunner()
    result = runner.invoke(app, ["doctor", "--project-root", str(tmp_path)])
    assert result.exit_code == 0

    monkeypatch.setattr(
        "devcouncil.cli.commands.doctor._probe_ollama_models",
        lambda _u: (False, set()),
    )
    result2 = runner.invoke(app, ["doctor", "--project-root", str(tmp_path)])
    assert result2.exit_code == 0


def test_map_pdg_and_html_flags(tmp_path, monkeypatch):
    from typer.testing import CliRunner

    from devcouncil.cli.commands.init import initialize_project
    from devcouncil.cli.main import app
    from devcouncil.indexing.graph.schema import CodeGraph
    from devcouncil.indexing.repo_mapper import RepoMap

    initialize_project(tmp_path, quiet=True, with_map=False, with_skills=False)
    out = tmp_path / ".devcouncil" / "repo_map.json"
    sample = RepoMap(
        languages=["python"],
        frameworks=[],
        package_managers=[],
        test_commands=[],
        important_files=[],
        candidate_files=[],
    )
    monkeypatch.setattr(
        "devcouncil.indexing.map_artifacts.refresh_map_artifacts",
        lambda *a, **k: SimpleNamespace(
            repo_map=sample,
            degraded=False,
            reason="",
            mode="full",
            generation=1,
            compatibility_export_degraded=False,
                build_incomplete=False,
        ),
    )
    graph_out = tmp_path / ".devcouncil" / "graph" / "code_graph.json"
    graph_out.parent.mkdir(parents=True, exist_ok=True)
    graph_out.write_text("{}", encoding="utf-8")
    monkeypatch.setattr(
        "devcouncil.indexing.graph.build.load_code_graph",
        lambda _r: CodeGraph(nodes=[], edges=[]),
    )
    monkeypatch.setattr(
        "devcouncil.indexing.graph.build.build_pdg_for_paths",
        lambda *a, **k: {},
    )
    monkeypatch.setattr(
        "devcouncil.indexing.graph.build.merge_pdg_into_graph",
        lambda g, layer: {},
    )
    monkeypatch.setattr(
        "devcouncil.indexing.graph.build.write_code_graph",
        lambda *a, **k: None,
    )
    monkeypatch.setattr(
        "devcouncil.app.config.load_config",
        lambda _r: SimpleNamespace(indexing=SimpleNamespace(write_graph_html=True, lsp_refs=False)),
    )
    monkeypatch.setattr(
        "devcouncil.indexing.viz.write_graph_html",
        lambda *a, **k: tmp_path / "g.html",
    )
    runner = CliRunner()
    result = runner.invoke(
        app,
        ["map", "--pdg", "--project-root", str(tmp_path), "-o", str(out)],
    )
    assert result.exit_code == 0


def test_doctor_probe_helpers_and_status_drift(tmp_path, monkeypatch):
    from devcouncil.cli.commands import doctor as doctor_cmd

    class Resp:
        status_code = 200

        def json(self):
            return {"version": "0.1", "models": [{"name": "tiny:latest"}]}

    monkeypatch.setattr("httpx.get", lambda *a, **k: Resp())
    ok, detail = doctor_cmd._probe_ollama("http://localhost:11434/v1")
    assert ok is True
    assert "Reachable" in detail
    q_ok, pulled = doctor_cmd._probe_ollama_models("http://localhost:11434/v1")
    assert q_ok is True
    assert doctor_cmd._ollama_model_present("tiny", pulled) is True
    assert doctor_cmd._ollama_model_present("other", pulled) is False

    monkeypatch.setattr("httpx.get", lambda *a, **k: (_ for _ in ()).throw(OSError("down")))
    bad, msg = doctor_cmd._probe_ollama("http://localhost:11434")
    assert bad is False
    assert "No Ollama" in msg

    drift = doctor_cmd.check_status_doc_drift(tmp_path)
    assert isinstance(drift, list)

    doctor_cmd._print_maturity_table()
    rows = doctor_cmd._subsystem_maturity_rows()
    assert rows


def test_doctor_mypy_and_status_doc_branches(tmp_path, monkeypatch):
    from devcouncil.cli.commands import doctor as doctor_cmd

    (tmp_path / "pyproject.toml").write_text("[project]\nname='t'\n", encoding="utf-8")
    (tmp_path / "src").mkdir()

    monkeypatch.setattr(
        doctor_cmd.subprocess,
        "run",
        lambda *a, **k: (_ for _ in ()).throw(FileNotFoundError()),
    )
    assert "unavailable" in doctor_cmd.check_mypy_status(tmp_path)[0][2]

    monkeypatch.setattr(
        doctor_cmd.subprocess,
        "run",
        lambda *a, **k: (_ for _ in ()).throw(subprocess.TimeoutExpired(cmd="mypy", timeout=1)),
    )
    assert "timed out" in doctor_cmd.check_mypy_status(tmp_path)[0][2]

    class Proc:
        returncode = 1
        stdout = "INTERNAL ERROR\n"
        stderr = ""

    monkeypatch.setattr(doctor_cmd.subprocess, "run", lambda *a, **k: Proc())
    assert "INTERNAL ERROR" in doctor_cmd.check_mypy_status(tmp_path)[0][2]

    Proc.stdout = "No module named mypy\n"
    assert "unavailable" in doctor_cmd.check_mypy_status(tmp_path)[0][2]

    Proc.returncode = 0
    Proc.stdout = "Success\n"
    assert "OK" in doctor_cmd.check_mypy_status(tmp_path)[0][1]

    docs = tmp_path / "docs"
    docs.mkdir()
    (docs / "project-status.md").write_text(
        "| Area | Status |\n| --- | --- |\n| CLI | Stable |\n",
        encoding="utf-8",
    )
    monkeypatch.setattr(
        doctor_cmd,
        "STATUS_DOC_UNIT_TEST_DIRS",
        [("CLI", "cli")],
    )
    drift = doctor_cmd.check_status_doc_drift(tmp_path)
    assert drift and "Status-doc drift" in drift[0][0]
