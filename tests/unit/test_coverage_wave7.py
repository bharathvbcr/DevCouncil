"""Wave-7: build_control, build_worker, and map_artifacts stable coverage."""

from __future__ import annotations

import json
import subprocess
from types import SimpleNamespace
from unittest.mock import MagicMock

import pytest

from devcouncil.codeintel.build_control import (
    BuildStatus,
    GraphBuildBusy,
    GraphBuildFailed,
    GraphBuildTimeout,
    _terminate_worker,
    _write_status,
    graph_build_session,
    read_build_status,
    run_isolated_full_build,
    status_path,
)
from devcouncil.indexing.graph.schema import CodeGraph
from devcouncil.indexing.map_artifacts import (
    AGENT_GUIDE_MARKER,
    _important_surfaces,
    _wiki_index_rel,
    agent_guide_text,
    generate_map_artifacts,
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


def test_terminate_worker_paths(monkeypatch):
    done = MagicMock()
    done.poll.return_value = 0
    _terminate_worker(done)

    alive = MagicMock()
    alive.poll.side_effect = [None, None]
    alive.pid = 4242
    alive.wait.side_effect = subprocess.TimeoutExpired(cmd="x", timeout=1)
    kills = []

    monkeypatch.setattr("devcouncil.codeintel.build_control.os.name", "posix")
    monkeypatch.setattr(
        "devcouncil.codeintel.build_control.os.killpg",
        lambda pid, sig: kills.append((pid, sig)),
    )
    _terminate_worker(alive)
    assert kills


def test_run_isolated_full_build_success(tmp_path, monkeypatch):
    from devcouncil.cli.commands.init import initialize_project

    initialize_project(tmp_path, quiet=True, with_map=False, with_skills=False)

    graph = CodeGraph(nodes=[], edges=[])
    service = SimpleNamespace(
        store=SimpleNamespace(current_generation=lambda: 1),
        load=lambda: graph,
    )
    monkeypatch.setattr(
        "devcouncil.codeintel.build_control.get_codeintel_service",
        lambda _r: service,
    )
    monkeypatch.setattr(
        "devcouncil.app.config.load_config",
        lambda _r: SimpleNamespace(
            indexing=SimpleNamespace(
                build_stall_timeout_seconds=30.0,
                build_total_timeout_seconds=60.0,
            )
        ),
    )

    lines = [
        json.dumps(
            {
                "phase": "extract",
                "completed": 1,
                "total": 2,
                "compatibility_export": "healthy",
            }
        )
        + "\n",
        "not-json\n",
        json.dumps(
            {
                "state": "complete",
                "phase": "complete",
                "completed": 2,
                "total": 2,
                "compatibility_export": "healthy",
            }
        )
        + "\n",
    ]

    class FakeStdout:
        def __iter__(self):
            return iter(lines)

    class FakeProc:
        def __init__(self):
            self.pid = 99
            self.stdout = FakeStdout()
            self.stderr = MagicMock(read=lambda: "")
            self.returncode = 0
            self._polls = 0

        def poll(self):
            self._polls += 1
            return 0 if self._polls > 2 else None

        def wait(self, timeout=None):
            return 0

        def terminate(self):
            return None

        def kill(self):
            return None

    monkeypatch.setattr(
        "devcouncil.codeintel.build_control.subprocess.Popen",
        lambda *a, **k: FakeProc(),
    )
    gens = {"n": 0}

    def gen():
        gens["n"] += 1
        return 1 if gens["n"] == 1 else 2

    service.store.current_generation = gen
    result = run_isolated_full_build(tmp_path, changed_paths={"a.py"}, liveness=False)
    assert result.graph is graph
    assert result.status.state == "complete"


def test_run_isolated_full_build_timeout_and_fail(tmp_path, monkeypatch):
    from devcouncil.cli.commands.init import initialize_project

    initialize_project(tmp_path, quiet=True, with_map=False, with_skills=False)
    service = SimpleNamespace(
        store=SimpleNamespace(current_generation=lambda: 1),
        load=lambda: None,
    )
    monkeypatch.setattr(
        "devcouncil.codeintel.build_control.get_codeintel_service",
        lambda _r: service,
    )
    monkeypatch.setattr(
        "devcouncil.app.config.load_config",
        lambda _r: SimpleNamespace(
            indexing=SimpleNamespace(
                build_stall_timeout_seconds=0.01,
                build_total_timeout_seconds=0.01,
            )
        ),
    )

    class FakeProc:
        def __init__(self):
            self.pid = 1
            self.stdout = MagicMock(__iter__=lambda self: iter([]))
            self.stderr = MagicMock(read=lambda: "boom")
            self.returncode = 1

        def poll(self):
            return None

        def wait(self, timeout=None):
            raise subprocess.TimeoutExpired(cmd="x", timeout=1)

        def terminate(self):
            return None

        def kill(self):
            return None

    monkeypatch.setattr(
        "devcouncil.codeintel.build_control.subprocess.Popen",
        lambda *a, **k: FakeProc(),
    )
    monkeypatch.setattr("devcouncil.codeintel.build_control.os.name", "posix")
    monkeypatch.setattr("devcouncil.codeintel.build_control.os.killpg", lambda *a, **k: None)

    with pytest.raises(GraphBuildTimeout):
        run_isolated_full_build(tmp_path)

    class FailProc(FakeProc):
        def poll(self):
            return 1

        def wait(self, timeout=None):
            return 1

    monkeypatch.setattr(
        "devcouncil.codeintel.build_control.subprocess.Popen",
        lambda *a, **k: FailProc(),
    )
    monkeypatch.setattr(
        "devcouncil.app.config.load_config",
        lambda _r: SimpleNamespace(
            indexing=SimpleNamespace(
                build_stall_timeout_seconds=30.0,
                build_total_timeout_seconds=60.0,
            )
        ),
    )
    with pytest.raises(GraphBuildFailed):
        run_isolated_full_build(tmp_path)


# --- build_worker -------------------------------------------------------------


def test_build_worker_main_complete_and_degraded(monkeypatch, tmp_path):
    from devcouncil.codeintel import build_worker as bw

    graph = CodeGraph(nodes=[], edges=[])
    monkeypatch.setattr(
        "devcouncil.indexing.graph.build.build_code_graph",
        lambda *a, **k: graph,
    )
    monkeypatch.setattr(
        "devcouncil.indexing.graph.build.write_code_graph",
        lambda *a, **k: None,
    )
    monkeypatch.setattr(
        "sys.argv",
        ["build_worker", "--root", str(tmp_path), "--build-id", "id1", "--changed-path", "a.py"],
    )
    assert bw.main() == 0

    from devcouncil.indexing.graph.build import CompatibilityGraphTooLarge

    monkeypatch.setattr(
        "devcouncil.indexing.graph.build.write_code_graph",
        lambda *a, **k: (_ for _ in ()).throw(CompatibilityGraphTooLarge("too big")),
    )
    monkeypatch.setattr(
        "sys.argv",
        ["build_worker", "--root", str(tmp_path), "--build-id", "id2", "--no-liveness"],
    )
    assert bw.main() == 0


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


def test_refresh_map_artifacts_lean_fallback(tmp_path, monkeypatch):
    from contextlib import nullcontext

    from devcouncil.cli.commands.init import initialize_project

    initialize_project(tmp_path, quiet=True, with_map=False, with_skills=False)
    out = tmp_path / ".devcouncil" / "repo_map.json"

    monkeypatch.setattr(
        "devcouncil.codeintel.build_control.graph_build_session",
        lambda root, **k: nullcontext(),
    )
    monkeypatch.setattr(
        "devcouncil.codeintel.build_control.run_isolated_full_build",
        lambda *a, **k: (_ for _ in ()).throw(RuntimeError("no graph")),
    )
    monkeypatch.setattr(
        "devcouncil.indexing.map_artifacts.CodeReviewGraphAdapter",
        lambda _r: SimpleNamespace(get_context=lambda: SimpleNamespace(available=False)),
    )
    monkeypatch.setattr(
        "devcouncil.codeintel.get_codeintel_service",
        lambda _r: SimpleNamespace(store=SimpleNamespace(current_generation=lambda: None)),
    )

    result = refresh_map_artifacts(tmp_path, out, quiet=True)
    assert result.degraded is True
    assert result.mode == "lean"
    assert out.is_file()
    data = json.loads(out.read_text(encoding="utf-8"))
    assert data.get("graph_degraded") is True
    from devcouncil.indexing.repo_mapper import RepoMapper

    assert RepoMapper(tmp_path).map_is_stale(data) is True

    again = generate_map_artifacts(tmp_path, out, quiet=True)
    assert again.languages is not None


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


def test_map_if_stale_and_busy(tmp_path, monkeypatch):
    from typer.testing import CliRunner

    from devcouncil.cli.commands.init import initialize_project
    from devcouncil.cli.main import app
    from devcouncil.codeintel.build_control import GraphBuildBusy
    from devcouncil.indexing.repo_mapper import RepoMap

    initialize_project(tmp_path, quiet=True, with_map=False, with_skills=False)
    out = tmp_path / ".devcouncil" / "repo_map.json"
    out.write_text("{}", encoding="utf-8")

    monkeypatch.setattr(
        "devcouncil.indexing.repo_mapper.RepoMapper.map_is_stale",
        lambda self, data: False,
    )
    runner = CliRunner()
    fresh = runner.invoke(
        app,
        ["map", "--if-stale", "--project-root", str(tmp_path), "-o", str(out)],
    )
    assert fresh.exit_code == 0

    monkeypatch.setattr(
        "devcouncil.indexing.map_artifacts.refresh_map_artifacts",
        lambda *a, **k: (_ for _ in ()).throw(GraphBuildBusy("busy")),
    )
    busy = runner.invoke(app, ["map", "--project-root", str(tmp_path), "-o", str(out)])
    assert busy.exit_code == 1

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
        ),
    )
    monkeypatch.setattr(
        "devcouncil.indexing.graph.build.export_code_graph_json",
        lambda _r: None,
    )
    mapped = runner.invoke(app, ["map", "--project-root", str(tmp_path), "-o", str(out)])
    assert mapped.exit_code == 0


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
