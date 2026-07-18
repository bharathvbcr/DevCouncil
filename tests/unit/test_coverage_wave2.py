"""Stable unit coverage for CLI helpers, corpus wiring, stop-gate cache, MCP resources."""

from __future__ import annotations

import json
import sys
import time
from types import SimpleNamespace

import pytest
from typer.testing import CliRunner

from devcouncil.cli.main import app
from devcouncil.domain.gap import Gap
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.indexing import wiring
from devcouncil.indexing.wiring import CorpusGraph


runner = CliRunner()


# ---------------------------------------------------------------------------
# stop_gate_verify_cache
# ---------------------------------------------------------------------------


def test_verify_cache_record_load_and_expiry(tmp_path):
    from devcouncil.execution.stop_gate_verify_cache import (
        load_verify_cache,
        record_verify_cache,
    )

    record_verify_cache(
        tmp_path,
        task_id="TASK-1",
        status="verified",
        blocking_gaps=0,
        next_actions=[{"action": "done"}],
    )
    hit = load_verify_cache(tmp_path, task_id="TASK-1", ttl_minutes=5)
    assert hit is not None
    assert hit["passed"] is True
    assert hit["task_id"] == "TASK-1"

    assert load_verify_cache(tmp_path, task_id="TASK-2", ttl_minutes=5) is None
    assert load_verify_cache(tmp_path, task_id="TASK-1", ttl_minutes=0) is None

    path = tmp_path / ".devcouncil" / "cache" / "stop_gate_verify.json"
    stale = json.loads(path.read_text(encoding="utf-8"))
    stale["updated_at"] = time.time() - 10_000
    path.write_text(json.dumps(stale), encoding="utf-8")
    assert load_verify_cache(tmp_path, task_id="TASK-1", ttl_minutes=1) is None

    path.write_text("[]", encoding="utf-8")
    assert load_verify_cache(tmp_path, task_id="TASK-1") is None

    path.write_text(json.dumps({"task_id": "TASK-1", "updated_at": "bad"}), encoding="utf-8")
    assert load_verify_cache(tmp_path, task_id="TASK-1") is None

    record_verify_cache(
        tmp_path,
        task_id="TASK-9",
        status="blocked",
        blocking_gaps=2,
        passed=False,
    )
    broken = tmp_path / "broken_root"
    broken.mkdir()
    (broken / ".devcouncil").write_text("not-a-dir", encoding="utf-8")
    record_verify_cache(broken, task_id="T", status="x", blocking_gaps=0)


# ---------------------------------------------------------------------------
# reporting.mcp_resources
# ---------------------------------------------------------------------------


def test_mcp_resources_read_and_list(tmp_path, monkeypatch):
    from devcouncil.cli.commands.init import initialize_project
    from devcouncil.reporting import mcp_resources as mr

    initialize_project(tmp_path, quiet=True, with_map=False, with_skills=False)

    report = mr.read_mcp_resource(tmp_path, "devcouncil://report")
    assert len(report) > 0
    assert '"tasks"' in mr.read_mcp_resource(tmp_path, "devcouncil://tasks")
    assert '"gaps"' in mr.read_mcp_resource(tmp_path, "devcouncil://gaps")
    cards = mr.read_mcp_resource(tmp_path, "devcouncil://cards")
    assert cards.startswith("{") or "card" in cards.lower()

    missing = json.loads(mr.read_mcp_resource(tmp_path, "devcouncil://task/TASK-MISSING"))
    assert missing["ok"] is False

    monkeypatch.setattr(mr, "get_db", lambda _root: None)
    assert "not initialized" in mr.read_mcp_resource(tmp_path, "devcouncil://report").lower()
    assert json.loads(mr.read_mcp_resource(tmp_path, "devcouncil://tasks")) == {"tasks": []}
    assert json.loads(mr.read_mcp_resource(tmp_path, "devcouncil://gaps")) == {"gaps": []}
    assert "not initialized" in mr.read_mcp_resource(tmp_path, "devcouncil://task/T1")

    class Src:
        def __init__(self, kind, name, description=""):
            self.kind = kind
            self.name = name
            self.description = description
            self.body = f"# {name}"

        def render(self):
            return self.body

    sources = [Src("design", "arch", "Architecture"), Src("okf", "bundle", "OKF")]
    monkeypatch.setattr(mr, "discover_knowledge_sources", lambda _root: sources)
    monkeypatch.setattr(mr, "knowledge_source_uri", lambda kind, name: f"devcouncil://knowledge/{kind}/{name}")

    index = mr.read_mcp_resource(tmp_path, "devcouncil://knowledge")
    assert "Architecture" in index and "OKF" in index
    assert mr.read_mcp_resource(tmp_path, "devcouncil://knowledge/design/arch") == "# arch"
    assert "not found" in mr.read_mcp_resource(tmp_path, "devcouncil://knowledge/design/nope")

    with pytest.raises(ValueError):
        mr.read_mcp_resource(tmp_path, "devcouncil://unknown")

    uris = mr.list_mcp_resource_uris(tmp_path)
    kinds = {u["uri"] for u in uris}
    assert "devcouncil://report" in kinds
    assert "devcouncil://knowledge" in kinds
    assert "devcouncil://knowledge/design/arch" in kinds


# ---------------------------------------------------------------------------
# boot / gaps / shell CLI helpers
# ---------------------------------------------------------------------------


def test_boot_run_setup_path_branches(tmp_path, monkeypatch):
    import devcouncil.cli.commands.boot as boot_cmd

    calls = {"evidence": 0, "integrations": 0}

    monkeypatch.setattr(boot_cmd, "initialize_project", lambda *a, **k: False)
    monkeypatch.setattr(boot_cmd, "_set_model_provider", lambda *a, **k: None)
    monkeypatch.setattr(boot_cmd, "_set_model_roles", lambda *a, **k: None)
    monkeypatch.setattr(
        boot_cmd,
        "load_config",
        lambda _r: SimpleNamespace(models=SimpleNamespace(provider="openrouter")),
    )
    monkeypatch.setattr(boot_cmd, "_configure_vertexai_settings", lambda *a, **k: None)
    monkeypatch.setattr(boot_cmd, "_configure_api_key", lambda *a, **k: None)
    monkeypatch.setattr(boot_cmd, "render_doctor_check", lambda *a, **k: None)
    monkeypatch.setattr(boot_cmd, "scaffold_ci", lambda *_: None)
    monkeypatch.setattr(
        boot_cmd,
        "scaffold_evidence_ci",
        lambda root: (calls.__setitem__("evidence", 1) or (root / "ev.yml")),
    )
    monkeypatch.setattr(
        boot_cmd,
        "_configure_coding_cli_integrations",
        lambda *a, **k: calls.__setitem__("integrations", 1),
    )

    boot_cmd._run_setup_path(
        tmp_path,
        name="demo",
        provider="openrouter",
        model=None,
        role_models={},
        api_key=None,
        skip_api_key=True,
        skip_integrations=False,
        skip_map=True,
        skip_skills=True,
        scaffold_ci_flag=True,
        scaffold_ci_evidence=True,
        gemini_scope="project",
    )
    assert calls["integrations"] == 1
    assert calls["evidence"] == 1

    written = tmp_path / ".github" / "workflows" / "ci.yml"
    written.parent.mkdir(parents=True, exist_ok=True)
    written.write_text("name: x\n", encoding="utf-8")
    monkeypatch.setattr(boot_cmd, "scaffold_ci", lambda root: written)
    monkeypatch.setattr(boot_cmd, "scaffold_evidence_ci", lambda _r: None)
    boot_cmd._run_setup_path(
        tmp_path,
        name=None,
        provider=None,
        model=None,
        role_models={},
        api_key=None,
        skip_api_key=True,
        skip_integrations=True,
        skip_map=True,
        skip_skills=True,
        scaffold_ci_flag=True,
        scaffold_ci_evidence=True,
        gemini_scope="user",
    )


def test_boot_rejects_bad_gemini_scope(tmp_path, monkeypatch):
    import devcouncil.cli.commands.boot as boot_cmd

    monkeypatch.setattr(boot_cmd, "_run_setup_path", lambda *a, **k: None)
    monkeypatch.setattr(boot_cmd, "go_command", lambda *a, **k: None)
    result = runner.invoke(app, ["boot", "goal", "--gemini-scope", "bad", "--project-root", str(tmp_path)])
    assert result.exit_code == 2


def test_boot_rejects_bad_provider(tmp_path, monkeypatch):
    import devcouncil.cli.commands.boot as boot_cmd

    monkeypatch.setattr(boot_cmd, "_run_setup_path", lambda *a, **k: None)
    monkeypatch.setattr(boot_cmd, "go_command", lambda *a, **k: None)
    result = runner.invoke(
        app,
        ["boot", "goal", "--provider", "not-a-real-provider", "--project-root", str(tmp_path)],
    )
    assert result.exit_code == 2


def test_gaps_payload_and_cli(tmp_path):
    from devcouncil.cli.commands import gaps as gaps_cmd
    from devcouncil.cli.commands.init import initialize_project
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import GapRepository, TaskRepository

    initialize_project(tmp_path, quiet=True, with_map=False, with_skills=False)
    db = get_db(tmp_path)
    assert db
    with db.get_session() as session:
        TaskRepository(session).save(
            Task(
                id="TASK-G",
                title="g",
                description="d",
                status="planned",
                planned_files=[],
            )
        )
        GapRepository(session).save(
            Gap(
                id="GAP-1",
                task_id="TASK-G",
                gap_type="missing_test",
                description="need tests",
                recommended_fix="add tests",
                blocking=True,
                severity="high",
            )
        )
        GapRepository(session).save(
            Gap(
                id="GAP-2",
                task_id="TASK-G",
                gap_type="architecture_drift",
                description="docs",
                recommended_fix="update docs",
                blocking=False,
                severity="low",
            )
        )

    all_payload = gaps_cmd._gaps_payload(tmp_path)
    assert all_payload["blocking_count"] == 1
    assert all_payload["advisory_count"] == 1

    task_payload = gaps_cmd._gaps_payload(tmp_path, task_id="TASK-G", blocking_only=True)
    assert task_payload["task_id"] == "TASK-G"
    assert len(task_payload["gaps"]) == 1

    next_payload = gaps_cmd._next_actions_payload(tmp_path, "TASK-G")
    assert next_payload["ok"] is True
    assert "next_actions" in next_payload

    empty = tmp_path / "noinit"
    empty.mkdir()
    # uninitialized: get_db returns None after quiet init may still create state —
    # exercise via monkeypatch inside the module.
    import devcouncil.cli.commands.gaps as gmod

    real_get_db = gmod.get_db
    gmod.get_db = lambda _r: None  # type: ignore[assignment]
    try:
        assert gaps_cmd._gaps_payload(empty)["initialized"] is False
        assert gaps_cmd._next_actions_payload(empty, "TASK-G")["ok"] is False
    finally:
        gmod.get_db = real_get_db  # type: ignore[assignment]

    result = runner.invoke(
        app,
        ["gaps", "--json", "--task-id", "TASK-G", "--blocking-only", "--project-root", str(tmp_path)],
    )
    assert result.exit_code == 0
    assert "GAP-1" in result.output

    bad = runner.invoke(app, ["gaps", "--next-actions", "--json", "--project-root", str(tmp_path)])
    assert bad.exit_code == 1

    next_cli = runner.invoke(
        app,
        ["gaps", "--next-actions", "--task-id", "TASK-G", "--json", "--project-root", str(tmp_path)],
    )
    assert next_cli.exit_code == 0

    fail = runner.invoke(
        app,
        ["gaps", "--fail-on-blocking", "--json", "--project-root", str(tmp_path)],
    )
    assert fail.exit_code == 1


def test_shell_error_paths(tmp_path, monkeypatch):
    from devcouncil.cli.commands.init import initialize_project
    import devcouncil.cli.commands.shell as shell_cmd
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import TaskRepository

    monkeypatch.setattr(shell_cmd, "get_db", lambda _r: None)
    result = runner.invoke(app, ["shell", "TASK-1", "--project-root", str(tmp_path)])
    assert result.exit_code == 1

    monkeypatch.setattr(shell_cmd, "get_db", get_db)
    initialize_project(tmp_path, quiet=True, with_map=False, with_skills=False)
    missing = runner.invoke(app, ["shell", "TASK-MISSING", "--project-root", str(tmp_path)])
    assert missing.exit_code == 1

    db = get_db(tmp_path)
    with db.get_session() as session:
        TaskRepository(session).save(
            Task(id="TASK-S", title="s", description="d", status="planned", planned_files=[])
        )

    monkeypatch.setattr(
        shell_cmd,
        "GuardedShellSession",
        lambda *a, **k: (_ for _ in ()).throw(ValueError("bad shell")),
    )
    bad_shell = runner.invoke(app, ["shell", "TASK-S", "--project-root", str(tmp_path)])
    assert bad_shell.exit_code == 2

    class Sess:
        def start(self, force=False):
            raise ValueError("leased")

        def finish(self):
            pass

        def run_one(self, cmd):
            return 0

    monkeypatch.setattr(shell_cmd, "GuardedShellSession", lambda *a, **k: Sess())
    leased = runner.invoke(app, ["shell", "TASK-S", "--command", "echo hi", "--project-root", str(tmp_path)])
    assert leased.exit_code == 2

    class OkSess:
        def start(self, force=False):
            return None

        def finish(self):
            return None

        def run_one(self, cmd):
            return 7

    monkeypatch.setattr(shell_cmd, "GuardedShellSession", lambda *a, **k: OkSess())
    one = runner.invoke(
        app,
        ["shell", "TASK-S", "--command", "false", "--project-root", str(tmp_path)],
    )
    assert one.exit_code == 7


# ---------------------------------------------------------------------------
# wiring corpus extractors
# ---------------------------------------------------------------------------


def test_wiring_corpus_pdf_image_html_and_query_edges(tmp_path, monkeypatch):
    docs = tmp_path / "docs"
    docs.mkdir()
    (docs / "guide.md").write_text("# Title\n\nBody alpha\n", encoding="utf-8")
    (docs / "pic.png").write_bytes(b"\x89PNG\r\n\x1a\n" + b"\x00" * 16)
    (docs / "notes.rst").write_text("Notes\n=====\n\nrst body\n", encoding="utf-8")
    (docs / "paper.pdf").write_bytes(b"%PDF-1.4 fake")

    class FakePage:
        def extract_text(self):
            return "See [site](https://example.com/docs) for more."

    class FakeReader:
        def __init__(self, *_a, **_k):
            self.pages = [FakePage()]

    monkeypatch.setitem(sys.modules, "pypdf", SimpleNamespace(PdfReader=FakeReader))

    (tmp_path / ".devcouncil").mkdir(exist_ok=True)
    (tmp_path / ".devcouncil" / "config.yaml").write_text(
        "indexing:\n  corpus:\n    write_html: true\n    vision_captions: true\n    paths:\n      - docs\n",
        encoding="utf-8",
    )

    graph = wiring.build_corpus(tmp_path)
    assert graph.nodes
    assert wiring.corpus_html_path(tmp_path).is_file()

    img_nodes, _ = wiring._extract_image(
        "docs/pic.png",
        docs / "pic.png",
        vision_captions=True,
        project_root=tmp_path,
    )
    assert img_nodes[0].kind == "image"
    assert wiring._optional_vision_caption(tmp_path, docs / "pic.png") is None

    pdf_nodes, pdf_edges = wiring._extract_pdf("docs/paper.pdf", docs / "paper.pdf")
    assert any(n.kind == "pdf_page" for n in pdf_nodes)
    assert any(e.kind == "contains" for e in pdf_edges)

    empty_q = wiring.query_corpus(tmp_path, "   ")
    assert empty_q.get("error")
    none_q = wiring.query_corpus(tmp_path / "missing", "x")
    assert none_q.get("error")

    enriched = wiring._optional_llm_enrich(tmp_path, graph)
    assert isinstance(enriched, CorpusGraph)

    list(wiring._iter_corpus_files(tmp_path, ["../outside"], [".md"]))


def test_wiring_extract_pdf_without_pypdf(tmp_path, monkeypatch):
    pdf = tmp_path / "a.pdf"
    pdf.write_bytes(b"%PDF")
    monkeypatch.setitem(sys.modules, "pypdf", None)

    import builtins

    real_import = builtins.__import__

    def fake_import(name, *a, **k):
        if name == "pypdf" or name.startswith("pypdf."):
            raise ImportError("no pypdf")
        return real_import(name, *a, **k)

    monkeypatch.setattr(builtins, "__import__", fake_import)
    nodes, edges = wiring._extract_pdf("a.pdf", pdf)
    assert nodes[0].kind == "pdf"
    assert edges == []


# ---------------------------------------------------------------------------
# campaign orchestrator pure helpers
# ---------------------------------------------------------------------------


def test_campaign_result_and_wave_selection(tmp_path):
    from devcouncil.campaign.orchestrator import Campaign, CampaignResult, TaskOutcome

    result = CampaignResult(
        goal="g",
        outcomes=[
            TaskOutcome("T1", "a", "w1", "remember", True, True, "verified"),
            TaskOutcome("T2", "b", "w1", "remember", True, False, "blocked"),
            TaskOutcome("T3", "c", "-", "remember", False, False, "skipped"),
        ],
        dashboard_path=None,
        halted=False,
    )
    assert result.verified == ["T1"]
    assert "T2" in result.blocked
    assert result.skipped == ["T3"]
    assert result.success is False
    assert "verified" in result.summary_line()

    halted = CampaignResult(goal="g", outcomes=[], dashboard_path=None, halted=True, halt_reason="budget")
    assert halted.success is False

    t1 = Task(
        id="A",
        title="a",
        description="d",
        status="planned",
        planned_files=[PlannedFile(path="src/a.py", allowed_change="modify", reason="x")],
    )
    t2 = Task(
        id="B",
        title="b",
        description="d",
        status="planned",
        planned_files=[PlannedFile(path="src/a.py", allowed_change="modify", reason="x")],
    )
    t3 = Task(
        id="C",
        title="c",
        description="d",
        status="planned",
        planned_files=[PlannedFile(path="src/c.py", allowed_change="modify", reason="x")],
    )
    camp = Campaign(
        tmp_path,
        goal="g",
        tasks=[t1, t2, t3],
        executor_factory=lambda _owner: SimpleNamespace(run_task=lambda *a, **k: None),
        verify_fn=lambda *_: (True, []),
        max_parallel=2,
    )
    wave = camp._select_wave([t1, t2, t3])
    assert t1 in wave
    assert t2 not in wave
    assert t3 in wave

    single = Campaign(
        tmp_path,
        goal="g",
        tasks=[t1, t2],
        executor_factory=lambda _owner: SimpleNamespace(run_task=lambda *a, **k: None),
        verify_fn=lambda *_: (True, []),
        max_parallel=1,
    )
    assert single._select_wave([t1, t2]) == [t1]
    assert single._writable_planned_paths(t1) == {"src/a.py"}


def test_campaign_run_covers_dispatch(tmp_path):
    from devcouncil.campaign.orchestrator import Campaign

    class Exec:
        def run_task(self, task, requirements):
            return SimpleNamespace(success=True, message="ok")

    parent = Task(id="P", title="p", description="d", status="planned", planned_files=[])
    child = Task(
        id="C",
        title="c",
        description="d",
        status="planned",
        planned_files=[],
        depends_on=["MISSING"],
    )
    camp = Campaign(
        tmp_path,
        goal="g",
        tasks=[parent, child],
        executor_factory=lambda _owner: Exec(),
        verify_fn=lambda *_: (True, []),
        max_parallel=1,
        on_event=lambda _m: None,
    )
    result = camp.run()
    assert "P" in result.verified
    assert "C" in result.skipped


# ---------------------------------------------------------------------------
# stop_gate briefing helpers
# ---------------------------------------------------------------------------


def test_session_briefing_and_phase_helpers(tmp_path, monkeypatch):
    from devcouncil.execution import stop_gate as sg
    from devcouncil.execution.stop_gate_history import append_event

    root = tmp_path
    (root / ".devcouncil").mkdir()
    (root / ".devcouncil" / "config.yaml").write_text(
        "project:\n  name: t\nexecution:\n  stop_gate:\n    mode: assist\n",
        encoding="utf-8",
    )

    append_event(
        root,
        {"decision": "assist", "claim": "ship it", "blocking_gaps": 2, "session_id": "s"},
    )
    transcript = root / "transcript.jsonl"
    transcript.write_text(
        json.dumps({"role": "assistant", "content": "Should we continue?"}) + "\n",
        encoding="utf-8",
    )
    monkeypatch.setattr(sg, "last_assistant_sentence", lambda _p: "Should we continue?")
    monkeypatch.setattr(sg, "ends_on_open_question", lambda _p: True)
    monkeypatch.setattr(sg, "active_task_id", lambda _p: None)

    brief = sg.session_briefing(root, {"transcript_path": str(transcript)})
    assert brief and "Where you left off" in brief
    assert "open question" in brief

    assert sg._project_phase(root) is None or isinstance(sg._project_phase(root), str)
    assert sg._task_blocking_summary(root, None) == (0, [])
    assert sg._task_blocking_summary(root, "TASK-X")[0] == 0


# ---------------------------------------------------------------------------
# graph_cmd remaining branches via mocks
# ---------------------------------------------------------------------------


def test_graph_cmd_corpus_and_pdg_text_paths(tmp_path, monkeypatch):
    from devcouncil.cli.commands import graph_cmd as gc

    mapped = tmp_path
    (mapped / ".devcouncil").mkdir()

    monkeypatch.setattr(
        "devcouncil.indexing.wiring.build_corpus",
        lambda root, path=None: SimpleNamespace(nodes=[], edges=[]),
    )
    monkeypatch.setattr(
        "devcouncil.indexing.wiring.corpus_status",
        lambda root: {
            "enabled": True,
            "node_count": 1,
            "edge_count": 0,
            "graph_path": ".devcouncil/corpus/graph.json",
            "built_at": "now",
        },
    )
    monkeypatch.setattr(
        "devcouncil.indexing.wiring.query_corpus",
        lambda root, query, limit=20: {
            "matches": [{"label": "L", "kind": "document", "path": "docs/a.md", "score": 1}],
            "count": 1,
        },
    )

    r = runner.invoke(gc.corpus_app, ["build", "--project-root", str(mapped)])
    assert r.exit_code == 0, r.output
    r2 = runner.invoke(gc.corpus_app, ["query", "alpha", "--project-root", str(mapped)])
    assert r2.exit_code == 0, r2.output
    r3 = runner.invoke(gc.corpus_app, ["status", "--project-root", str(mapped)])
    assert r3.exit_code == 0, r3.output

    monkeypatch.setattr(
        "devcouncil.indexing.wiring.query_corpus",
        lambda *a, **k: {"error": "missing", "matches": []},
    )
    err = runner.invoke(gc.corpus_app, ["query", "x", "--project-root", str(mapped)])
    assert err.exit_code == 1
    empty = runner.invoke(
        gc.corpus_app,
        ["query", "x", "--json", "--project-root", str(mapped)],
    )
    assert empty.exit_code == 1

    monkeypatch.setattr(
        "devcouncil.indexing.wiring.query_corpus",
        lambda *a, **k: {"matches": [], "count": 0},
    )
    none = runner.invoke(gc.corpus_app, ["query", "zzz", "--project-root", str(mapped)])
    assert none.exit_code == 0
    assert "No matches" in none.output

    monkeypatch.setattr(gc, "_require_graph", lambda root: SimpleNamespace(meta={"pdg": {"stats": {}}}, nodes=[], edges=[]))
    monkeypatch.setattr(
        "devcouncil.indexing.graph.build.build_pdg_for_paths",
        lambda *a, **k: SimpleNamespace(files={}),
    )
    monkeypatch.setattr(
        "devcouncil.indexing.graph.build.merge_pdg_into_graph",
        lambda graph, layer: {},
    )
    monkeypatch.setattr(
        "devcouncil.indexing.graph.build.write_code_graph",
        lambda *a, **k: None,
    )
    pdg = runner.invoke(gc.app, ["pdg", "build", "--project-root", str(mapped)])
    assert pdg.exit_code == 0, pdg.output

    monkeypatch.setattr(
        "devcouncil.indexing.graph.query.explain_pdg_taint",
        lambda *a, **k: {"ok": True, "findings": []},
    )
    expl = runner.invoke(gc.app, ["explain", "--project-root", str(mapped)])
    assert expl.exit_code == 0
    assert "No taint" in expl.output

    monkeypatch.setattr(
        "devcouncil.indexing.graph.query.explain_pdg_taint",
        lambda *a, **k: {
            "ok": True,
            "findings": [
                {
                    "path": "a.py",
                    "sink_line": 1,
                    "category": "sql",
                    "function": "f",
                    "source_expr": "x",
                    "sink_expr": "y",
                }
            ],
        },
    )
    expl2 = runner.invoke(gc.app, ["explain", "--project-root", str(mapped)])
    assert expl2.exit_code == 0

    monkeypatch.setattr(
        "devcouncil.indexing.graph.query.query_pdg_controls",
        lambda *a, **k: {
            "ok": True,
            "functions": [{"qualname": "f", "path": "a.py", "cdg": [{"a": 1}]}],
        },
    )
    ctrl = runner.invoke(
        gc.app,
        ["pdg-query", "--mode", "controls", "--target", "f", "--project-root", str(mapped)],
    )
    assert ctrl.exit_code == 0

    bad_mode = runner.invoke(
        gc.app,
        ["pdg-query", "--mode", "nope", "--target", "f", "--project-root", str(mapped)],
    )
    assert bad_mode.exit_code == 2

    monkeypatch.setattr(
        "devcouncil.indexing.graph.embeddings.semantic_search",
        lambda *a, **k: {"ok": False},
    )

    class Engine:
        def search(self, q, limit=50):
            return {
                "matches": [
                    {"path": "a.py", "id": "n1", "kind": "function", "label": "run", "score": 0.9}
                ]
            }

    monkeypatch.setattr("devcouncil.codeintel.query.CodeIntelQueryEngine", lambda root: Engine())
    sem = runner.invoke(
        gc.app,
        ["search", "run", "--semantic", "--project-root", str(mapped)],
    )
    assert sem.exit_code == 0, sem.output
