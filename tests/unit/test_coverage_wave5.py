"""Wave-5 stable coverage: doctor mapping/containment, api_routes helpers, stop_gate
internals, hook context emitters, build fingerprints, liveness ratchet symbols,
prompt enhancer helpers, and small CLI branches."""

from __future__ import annotations

import json
import subprocess
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import AsyncMock

from typer.testing import CliRunner

from devcouncil.cli.main import app
from devcouncil.domain.task import Task
from devcouncil.indexing.graph.schema import CodeGraph, GraphEdge, GraphNode, NodeKind
from devcouncil.verification.claims.models import Assertion, CheckResult, Kind, Status

runner = CliRunner()


def _init_repo(tmp_path: Path, yaml_body: str) -> Path:
    subprocess.run(["git", "init"], cwd=tmp_path, capture_output=True)
    dev = tmp_path / ".devcouncil"
    dev.mkdir(parents=True, exist_ok=True)
    (dev / "config.yaml").write_text(yaml_body, encoding="utf-8")
    return tmp_path


# --- doctor.py ------------------------------------------------------------------


def test_knowledge_dir_uses_config_and_default(tmp_path, monkeypatch):
    from devcouncil.cli.commands import doctor as doctor_cmd

    cfg = SimpleNamespace(knowledge=SimpleNamespace(directory="custom/knowledge"))
    assert doctor_cmd._knowledge_dir(tmp_path, config=cfg) == "custom/knowledge"

    import devcouncil.app.config as config_mod

    monkeypatch.setattr(config_mod, "load_config", lambda _r: (_ for _ in ()).throw(RuntimeError("boom")))
    assert doctor_cmd._knowledge_dir(tmp_path) == ".devcouncil/knowledge"


def test_check_liveness_reliability_ok_when_roots_present(tmp_path):
    from devcouncil.cli.commands import doctor as doctor_cmd

    map_path = tmp_path / ".devcouncil" / "repo_map.json"
    map_path.parent.mkdir(parents=True)
    map_path.write_text(
        json.dumps({"entry_roots": ["src/main.py"], "liveness_unreachable_unreliable": False}),
        encoding="utf-8",
    )
    rows = doctor_cmd.check_liveness_reliability(tmp_path)
    assert rows[0][0] == "Map liveness"
    assert "OK" in rows[0][1]
    assert "1 production entry root" in rows[0][2]


def test_check_mapping_stack_legacy_graphify_and_missing_graph(tmp_path):
    from devcouncil.cli.commands import doctor as doctor_cmd

    legacy = tmp_path / ".devcouncil" / "graphify.yaml"
    legacy.parent.mkdir(parents=True, exist_ok=True)
    legacy.write_text("corpus: {}\n", encoding="utf-8")

    rows = doctor_cmd.check_mapping_stack(tmp_path)
    labels = [row[0] for row in rows]
    assert "Legacy graphify.yaml" in labels
    assert "Code graph" in labels
    assert any("Missing" in row[2] for row in rows if row[0] == "Code graph")


def test_check_mapping_stack_loadable_graph(tmp_path, monkeypatch):
    from devcouncil.cli.commands import doctor as doctor_cmd

    graph_path = tmp_path / ".devcouncil" / "graph" / "code_graph.json"
    graph_path.parent.mkdir(parents=True, exist_ok=True)
    graph_path.write_text("{}", encoding="utf-8")

    monkeypatch.setattr(
        "devcouncil.indexing.graph.build.load_code_graph",
        lambda _root: CodeGraph(nodes=[], edges=[]),
    )
    rows = doctor_cmd.check_mapping_stack(tmp_path)
    graph_rows = [row for row in rows if row[0] == "Code graph"]
    assert graph_rows and "OK" in graph_rows[0][1]


def test_check_execution_containment_rows(tmp_path, monkeypatch):
    from devcouncil.cli.commands import doctor as doctor_cmd

    cfg = SimpleNamespace(
        execution=SimpleNamespace(enforce_file_scope_pre_verify=True),
        integrations=SimpleNamespace(claude=SimpleNamespace(write_gate=True)),
    )
    profile = SimpleNamespace(
        permission_mode="bypassPermissions",
        extra_args=["--permission-mode", "bypassPermissions"],
    )
    monkeypatch.setattr(
        "devcouncil.executors.agent_registry.load_agent_profiles",
        lambda _root: {"agent": profile},
    )
    rows = doctor_cmd.check_execution_containment(tmp_path, config=cfg)
    labels = {row[0] for row in rows}
    assert "Pre-verify scope gate" in labels
    assert "Claude write-gate" in labels
    assert any(row[0].startswith("Profile agent") for row in rows)


def test_check_repo_map_freshness_ok_path(tmp_path, monkeypatch):
    from devcouncil.cli.commands import doctor as doctor_cmd
    from devcouncil.indexing.repo_mapper import RepoMapper

    map_path = tmp_path / ".devcouncil" / "repo_map.json"
    map_path.parent.mkdir(parents=True, exist_ok=True)
    map_path.write_text(json.dumps({"generated_head": "abc", "indexed_hash": "fp"}), encoding="utf-8")
    monkeypatch.setattr(RepoMapper, "map_is_stale", lambda self, data: False)
    rows = doctor_cmd.check_repo_map_freshness(tmp_path)
    assert rows[0][1] == "[green]OK[/green]"


# --- api_routes.py --------------------------------------------------------------


def test_api_routes_normalize_and_match_variants():
    from devcouncil.indexing.graph import api_routes as ar

    assert ar.normalize_route_path("api/items") == "/api/items"
    assert ar.normalize_route_path("\\api\\items\\:id") == "/api/items/*"
    assert ar.paths_match("/a/:id", "/a/{id}") is True
    assert ar.paths_match("/a/b", "/a/c") is False
    assert ar.paths_match("/a", "/a/b") is False


def test_api_routes_verbs_compatible():
    from devcouncil.indexing.graph import api_routes as ar

    assert ar._verbs_compatible("ANY", "POST") is True
    assert ar._verbs_compatible("GET", "GET") is True
    assert ar._verbs_compatible("HEAD", "GET") is True
    assert ar._verbs_compatible("POST", "GET") is False


def test_api_routes_consumer_keys_and_risk():
    from devcouncil.indexing.graph import api_routes as ar

    window = [
        "const resp = await fetch('/x');",
        "const { id, name } = await resp.json();",
        "console.log(resp.token);",
    ]
    keys = ar._consumer_keys(window, "resp")
    assert {"id", "name", "token"} <= keys

    assert ar._risk_level(consumers=[], mismatches=[]) == "none"
    assert ar._risk_level(consumers=[{}], mismatches=[{}]) == "medium"
    assert ar._risk_level(consumers=[{}, {}], mismatches=[{}]) == "high"
    assert ar._risk_level(consumers=[{}, {}], mismatches=[]) == "low"


def test_api_routes_handler_return_keys_python(tmp_path):
    from devcouncil.indexing.graph import api_routes as ar

    src = tmp_path / "handlers.py"
    src.write_text(
        "def list_items():\n"
        "    return {'id': 1, 'name': 'x', 'price': 9}\n",
        encoding="utf-8",
    )
    node = GraphNode(
        id="handlers.py::list_items",
        kind=NodeKind.FUNCTION,
        path="handlers.py",
        name="list_items",
        line=1,
    )
    keys = ar.handler_return_keys(tmp_path, node)
    assert {"id", "name", "price"} <= keys


def test_api_routes_route_matches_filter():
    from devcouncil.indexing.graph import api_routes as ar

    route = {"path": "/api/items/{id}", "id": "route-1", "normalized_path": "/api/items/*"}
    assert ar._route_matches_filter(route, "/api/items/{id}") is True
    assert ar._route_matches_filter(route, "route-1") is True
    assert ar._route_matches_filter(route, "/api/items/42") is True
    assert ar._route_matches_filter(route, "/other") is False


def test_api_routes_resolve_handlers_fallback(tmp_path):
    from devcouncil.indexing.graph import api_routes as ar

    route = GraphNode(id="app.py::route_get", kind=NodeKind.ROUTE, path="app.py", name="get_items", line=1)
    handler = GraphNode(id="app.py::list_items", kind=NodeKind.FUNCTION, path="app.py", name="list_items", line=3)
    graph = CodeGraph(
        nodes=[route, handler],
        edges=[GraphEdge(source=route.id, target=route.id, kind="routes_to")],
    )
    nodes = {n.id: n for n in graph.nodes}
    resolved = ar._resolve_route_handlers(route, [route.id], nodes, graph)
    assert resolved == [handler.id]


# --- stop_gate.py ---------------------------------------------------------------


def test_resolve_mode_env_override(monkeypatch):
    from devcouncil.execution import stop_gate as sg

    monkeypatch.setenv("DEVCOUNCIL_STOP_GATE", "assist")
    assert sg._resolve_mode("block") == "assist"
    monkeypatch.delenv("DEVCOUNCIL_STOP_GATE", raising=False)
    assert sg._resolve_mode("BLOCK") == "block"
    assert sg._resolve_mode("bogus") == "off"


def test_merge_corrective_and_system_message():
    from devcouncil.execution import stop_gate as sg

    merged = sg._merge_corrective(
        "claim failed",
        "TASK-1",
        2,
        ["fix tests", "rerun verify"],
    )
    assert "claim failed" in merged
    assert "TASK-1" in merged
    assert "fix tests" in merged

    results = [
        CheckResult(
            assertion=Assertion(kind=Kind.TESTS_PASS, target=None, source_text="tests pass"),
            status=Status.PASS,
            detail="ok",
        ),
        CheckResult(
            assertion=Assertion(kind=Kind.LINT_CLEAN, target=None, source_text="lint clean"),
            status=Status.FAIL,
            detail="bad",
        ),
    ]
    msg = sg._system_message(
        claim_results=results,
        blocking_gaps=1,
        decision="block",
        notify_on_pass=True,
    )
    assert msg and "task blocked" in msg

    pass_msg = sg._system_message(
        claim_results=results,
        blocking_gaps=0,
        decision="pass",
        notify_on_pass=True,
    )
    assert pass_msg and "task ✓" in pass_msg


def test_evaluate_stop_non_dict_payload_and_no_claim(tmp_path):
    from devcouncil.execution import stop_gate as sg

    root = _init_repo(
        tmp_path,
        "project:\n  name: t\nexecution:\n  stop_gate:\n    mode: block\n    check_claims: false\n    verify_active_task: false\n",
    )
    result = sg.evaluate_stop(root, "not-a-dict")
    assert result.decision == "pass"

    result2 = sg.evaluate_stop(root, {})
    assert result2.decision == "pass"


def test_evaluate_stop_env_mode_off(tmp_path, monkeypatch):
    from devcouncil.execution import stop_gate as sg

    root = _init_repo(
        tmp_path,
        "project:\n  name: t\nexecution:\n  stop_gate:\n    mode: block\n",
    )
    monkeypatch.setenv("DEVCOUNCIL_STOP_GATE", "off")
    result = sg.evaluate_stop(root, {"claim_text": "All tests pass."})
    assert result.decision == "pass"
    assert result.mode == "off"


def test_statusline_tally_with_events(tmp_path):
    from devcouncil.execution import stop_gate as sg
    from devcouncil.execution.stop_gate_history import append_event

    root = _init_repo(tmp_path, "project:\n  name: t\nexecution:\n  stop_gate:\n    mode: assist\n")
    append_event(root, {"decision": "pass", "session_id": "sess-1"})
    append_event(root, {"decision": "block", "session_id": "sess-1"})
    tally = sg.statusline_tally(root, "sess-1")
    assert tally and "🛡" in tally and "✓" in tally and "✗" in tally


def test_run_claim_pass_empty_when_no_assertions(tmp_path):
    from devcouncil.execution import stop_gate as sg

    root = _init_repo(tmp_path, "project:\n  name: t\n")
    results = sg._run_claim_pass(
        root,
        "just chatting, no verifiable claims here",
        commands_cfg={},
        per_command_timeout=30,
        total_timeout=60,
    )
    assert results == []


# --- hook.py --------------------------------------------------------------------


def test_emit_additional_context_and_system_message(capsys):
    from devcouncil.cli.commands import hook as hook_cmd

    hook_cmd._emit_additional_context("SessionStart", None)
    assert capsys.readouterr().out == ""

    hook_cmd._emit_additional_context("SessionStart", "hello context")
    payload = json.loads(capsys.readouterr().out)
    assert payload["hookSpecificOutput"]["additionalContext"] == "hello context"

    hook_cmd._emit_system_message("")
    assert capsys.readouterr().out == ""

    hook_cmd._emit_system_message("toast")
    payload = json.loads(capsys.readouterr().out)
    assert payload["hookSpecificOutput"]["systemMessage"] == "toast"



def test_session_start_context_compact_branch(tmp_path, monkeypatch):
    from devcouncil.cli.commands import hook as hook_cmd

    monkeypatch.setattr(
        "devcouncil.execution.stop_gate.compact_briefing",
        lambda root, payload: "compact brief",
    )
    monkeypatch.setattr(
        "devcouncil.execution.stop_gate.record_compact_brief",
        lambda root, session_id=None: None,
    )
    ctx = hook_cmd._session_start_context(tmp_path, {"source": "compact", "session_id": "s1"})
    assert ctx == "compact brief"


def test_session_start_context_merges_status_and_briefing(tmp_path, monkeypatch):
    from devcouncil.cli.commands import hook as hook_cmd

    monkeypatch.setattr(hook_cmd, "_status_line", lambda root: "status line")
    monkeypatch.setattr(
        "devcouncil.execution.stop_gate.session_briefing",
        lambda root, payload: "extra brief",
    )
    ctx = hook_cmd._session_start_context(tmp_path, {})
    assert ctx == "status line\nextra brief"


def test_handle_unified_stop_gate_error_fail_open_gemini(tmp_path, monkeypatch, capsys):
    from devcouncil.cli.commands import hook as hook_cmd

    monkeypatch.setattr(hook_cmd, "write_signal", lambda *a, **k: tmp_path / "sig.json")
    monkeypatch.setattr(hook_cmd, "TraceLogger", lambda root: SimpleNamespace(log_event=lambda *a, **k: None))

    def boom(*a, **k):
        raise RuntimeError("gate down")

    monkeypatch.setattr("devcouncil.execution.stop_gate.evaluate_stop", boom)
    hook_cmd._handle_unified_stop("{}", client="gemini", project_root=tmp_path, hook_kind="stop")
    out = capsys.readouterr()
    assert "fail-open" in out.err
    assert "suppressOutput" in out.out


def test_post_tool_use_gemini_emits_allow(tmp_path, capsys):
    from devcouncil.cli.commands import hook as hook_cmd

    hook_cmd.post_tool_use("{}", client="gemini", project_root=tmp_path)
    payload = json.loads(capsys.readouterr().out)
    assert payload["decision"] == "allow"
    assert payload["suppressOutput"] is True


def test_emit_decision_gemini_warn(capsys):
    from devcouncil.cli.commands.hook import _emit_decision

    _emit_decision("gemini", "warn", "careful")
    payload = json.loads(capsys.readouterr().out)
    assert payload["decision"] == "allow"
    assert payload["systemMessage"] == "DevCouncil Warning: careful"


# --- build.py -------------------------------------------------------------------


def test_build_graph_path_and_fingerprints(tmp_path):
    from devcouncil.indexing.graph import build as graph_build

    assert graph_build.graph_path(tmp_path) == tmp_path / ".devcouncil" / "graph" / "code_graph.json"

    rel = "src/a.py"
    path = tmp_path / rel
    path.parent.mkdir(parents=True)
    path.write_text("x = 1\n", encoding="utf-8")
    fp = graph_build.content_fingerprint(tmp_path, [rel])
    assert len(fp) == 40

    path.write_text("x = 2\n", encoding="utf-8")
    assert graph_build.content_fingerprint(tmp_path, [rel]) != fp

    assert graph_build._files_fingerprint(["b.py", "a.py"]) == graph_build._files_fingerprint(["a.py", "b.py"])


def test_build_code_files_filters_vendored(tmp_path):
    from devcouncil.indexing.graph import build as graph_build

    files = [
        "src/app.py",
        "vendor/lib.py",
        "node_modules/pkg/index.js",
        "README.md",
    ]
    out = graph_build._code_files(files)
    assert "src/app.py" in out
    assert "README.md" not in out


def test_build_graph_json_indent_honors_config(tmp_path, monkeypatch):
    from devcouncil.indexing.graph import build as graph_build

    cfg = SimpleNamespace(indexing=SimpleNamespace(compact_graph_json=True))
    monkeypatch.setattr("devcouncil.app.config.load_config", lambda _r: cfg)
    assert graph_build._graph_json_indent(tmp_path) is None

    cfg.indexing.compact_graph_json = False
    assert graph_build._graph_json_indent(tmp_path) == 2

    monkeypatch.setattr(
        "devcouncil.app.config.load_config",
        lambda _r: (_ for _ in ()).throw(RuntimeError("no cfg")),
    )
    assert graph_build._graph_json_indent(tmp_path) is None


# --- liveness_ratchet.py --------------------------------------------------------


def test_liveness_ratchet_symbol_helpers_and_baseline_gate():
    from devcouncil.verification.checks import liveness_ratchet as lr

    assert lr.baseline_is_complete({"complete": True}) is True
    assert lr.baseline_is_complete({"complete": False}) is False
    assert lr.baseline_is_complete(None) is False

    assert lr._norm("./pkg/a.py") == "pkg/a.py"
    assert lr._symbol_key("pkg/a.py:12 helper_fn") == "pkg/a.py::helper_fn"
    assert lr._symbol_key("pkg/a.py:10:20:helper_fn") == "pkg/a.py::helper_fn"

    path, line, name = lr._symbol_display("pkg/a.py:12 helper_fn")
    assert path == "pkg/a.py" and line == 12 and name == "helper_fn"


def test_liveness_ratchet_skips_unreachable_when_roots_unreliable():
    from devcouncil.verification.checks.liveness_ratchet import detect_liveness_regressions

    baseline = {
        "complete": True,
        "unwired_candidates": [],
        "unreachable_files": [],
        "dead_symbol_candidates": [],
        "symbol_index": [],
        "entry_roots": [],
        "liveness_unreachable_unreliable": True,
    }
    current = {
        "unwired_candidates": [],
        "unreachable_files": ["pkg/stranded.py"],
        "dead_symbol_candidates": [],
        "entry_roots": [],
        "liveness_unreachable_unreliable": True,
    }
    gaps = detect_liveness_regressions(
        baseline,
        current,
        set(),
        task=Task(id="TASK-1", title="t", description="d"),
    )
    assert gaps == []


def test_liveness_baseline_load_and_delete(tmp_path):
    from devcouncil.verification.checks.liveness_ratchet import (
        delete_liveness_baseline,
        load_liveness_baseline,
    )

    assert load_liveness_baseline(tmp_path, "TASK-1") is None

    base_dir = tmp_path / ".devcouncil" / "liveness_baseline"
    base_dir.mkdir(parents=True)
    path = base_dir / "TASK-1.json"
    path.write_text(json.dumps({"complete": False}), encoding="utf-8")
    assert load_liveness_baseline(tmp_path, "TASK-1") is None

    path.write_text(json.dumps({"complete": True, "unwired_candidates": []}), encoding="utf-8")
    loaded = load_liveness_baseline(tmp_path, "TASK-1")
    assert loaded and loaded["complete"] is True
    assert delete_liveness_baseline(tmp_path, "TASK-1") is True
    assert not path.exists()


# --- prompt_enhancer_service.py -------------------------------------------------


def test_prompt_enhancement_normalized_and_debate_prompt():
    from devcouncil.planning.prompt_enhancer_service import PromptEnhancement, _clean_items

    raw = PromptEnhancement(
        original_goal="",
        enhanced_goal="  build auth  ",
        codebase_context=["  ctx  ", ""],
        debate_focus=["focus"],
        constraints=[" keep scope "],
        skills_brief="- **web** — modern stack",
        knowledge_brief="- **design** (design) — tokens",
    )
    norm = raw.normalized("add login")
    assert norm.original_goal == "add login"
    assert norm.enhanced_goal == "build auth"
    assert norm.codebase_context == ["ctx"]
    assert _clean_items([" a ", "", "b"]) == ["a", "b"]

    prompt = norm.debate_prompt()
    assert "Enhanced Planning Prompt" in prompt
    assert "Domain engineering intake" in prompt
    assert "Project knowledge" in prompt


def test_prompt_enhancer_helper_intake_and_brief():
    from devcouncil.planning.prompt_enhancer_service import (
        _compact_brief,
        _full_intake,
        _knowledge_brief,
        _knowledge_intake,
        _select_knowledge,
    )

    skill = SimpleNamespace(name="web", description="Modern web", body="Use React.")
    assert "web" in _compact_brief([skill])
    assert "React" in _full_intake([skill])

    source = SimpleNamespace(name="design", description="tokens", body="primary: blue", kind="design")
    assert "design" in _knowledge_brief([source])
    assert "primary" in _knowledge_intake([source])

    assert _select_knowledge("goal", None) == []


def test_save_and_load_active_prompt_enhancement(tmp_path):
    from devcouncil.planning.prompt_enhancer_service import (
        PromptEnhancement,
        load_latest_prompt_enhancement,
        save_active_prompt_enhancement,
    )

    enhancement = PromptEnhancement(original_goal="g", enhanced_goal="enhanced g")
    save_active_prompt_enhancement(tmp_path, enhancement)
    loaded = load_latest_prompt_enhancement(tmp_path)
    assert loaded and loaded.enhanced_goal == "enhanced g"

    runs = tmp_path / ".devcouncil" / "runs" / "run-a"
    runs.mkdir(parents=True)
    (runs / "prompt_enhancement.json").write_text(
        enhancement.model_dump_json(),
        encoding="utf-8",
    )
    active = tmp_path / ".devcouncil" / "active_prompt_enhancement.json"
    active.unlink()
    fallback = load_latest_prompt_enhancement(tmp_path)
    assert fallback and fallback.original_goal == "g"


def test_prompt_enhancer_service_stamps_skills(tmp_path, monkeypatch):
    import asyncio

    from devcouncil.planning.prompt_enhancer_service import PromptEnhancement, PromptEnhancerService

    skill = SimpleNamespace(name="ios", description="Swift UI", body="Use SwiftUI.")
    monkeypatch.setattr(
        "devcouncil.planning.prompt_enhancer_service._select_skills",
        lambda goal, root: [skill],
    )
    monkeypatch.setattr(
        "devcouncil.planning.prompt_enhancer_service._select_knowledge",
        lambda goal, root: [],
    )

    router = SimpleNamespace(
        complete_structured=AsyncMock(
            return_value=PromptEnhancement(original_goal="ship", enhanced_goal="ship fast")
        )
    )
    service = PromptEnhancerService(router)
    out = asyncio.run(service.enhance_prompt("ship", "{}", project_root=tmp_path))
    assert out.applied_skills == ["ios"]
    assert "ios" in out.skills_brief


# --- small CLIs: show, map, status ----------------------------------------------


def test_cli_show_json_not_found(tmp_path, monkeypatch):
    reset_cache = __import__("devcouncil.storage.db", fromlist=["reset_db_cache"]).reset_db_cache
    reset_cache()
    monkeypatch.chdir(tmp_path)
    runner.invoke(app, ["init"])
    res = runner.invoke(app, ["show", "MISSING", "--json"])
    assert res.exit_code != 0
    data = json.loads(res.output)
    assert data["ok"] is False


def test_cli_map_missing_project_root(tmp_path):
    missing = tmp_path / "nope"
    res = runner.invoke(app, ["map", "--project-root", str(missing)])
    assert res.exit_code != 0
    assert "does not exist" in res.output


def test_cli_status_uninitialized_payload(tmp_path, monkeypatch):
    from devcouncil.cli.commands import status as status_cmd

    monkeypatch.setattr(status_cmd, "get_db", lambda root: None)
    payload = status_cmd._status_payload(tmp_path)
    assert payload["initialized"] is False
    assert payload["phase"] == "UNINITIALIZED"


def test_cli_status_fail_on_blocking_json(tmp_path, monkeypatch):
    from devcouncil.domain.gap import Gap
    from devcouncil.storage.db import Database, reset_db_cache
    from devcouncil.storage.repositories import GapRepository, TaskRepository

    reset_db_cache()
    monkeypatch.chdir(tmp_path)
    runner.invoke(app, ["init"])
    db = Database(tmp_path / ".devcouncil" / "state.sqlite")
    with db.get_session() as session:
        TaskRepository(session).save(Task(id="TASK-1", title="t", description="d", status="running"))
        GapRepository(session).save(
            Gap(
                id="GAP-1",
                severity="high",
                gap_type="missing_test",
                description="no test",
                blocking=True,
                recommended_fix="add test",
                task_id="TASK-1",
            )
        )
    res = runner.invoke(app, ["status", "--json", "--fail-on-blocking"])
    assert res.exit_code != 0


def test_run_task_verify_cache_and_empty_paths(tmp_path, monkeypatch):
    from devcouncil.execution import stop_gate as sg

    monkeypatch.setattr(sg, "active_task_id", lambda _p: None)
    assert sg._run_task_verify(tmp_path, ttl_minutes=5) == (None, 0, [], False)

    monkeypatch.setattr(sg, "active_task_id", lambda _p: "TASK-1")
    monkeypatch.setattr(
        sg,
        "load_verify_cache",
        lambda *a, **k: {
            "blocking_gaps": 2,
            "next_actions": [{"action": "fix tests"}, "plain-string", {"nope": 1}],
        },
    )
    tid, gaps, actions, cached = sg._run_task_verify(tmp_path, ttl_minutes=5)
    assert tid == "TASK-1" and gaps == 2 and cached is True
    assert "fix tests" in actions and "plain-string" in actions

    monkeypatch.setattr(sg, "load_verify_cache", lambda *a, **k: None)
    monkeypatch.setattr("devcouncil.storage.db.get_db", lambda _p: None)
    assert sg._run_task_verify(tmp_path, ttl_minutes=5) == ("TASK-1", 0, [], False)

    class Sess:
        def get_session(self):
            return self

        def __enter__(self):
            return self

        def __exit__(self, *a):
            return False

    monkeypatch.setattr("devcouncil.storage.db.get_db", lambda _p: Sess())
    monkeypatch.setattr(
        "devcouncil.storage.repositories.TaskRepository",
        lambda session: SimpleNamespace(get_by_id=lambda _tid: None),
    )
    assert sg._run_task_verify(tmp_path, ttl_minutes=5) == ("TASK-1", 0, [], False)

    monkeypatch.setattr(
        "devcouncil.storage.repositories.TaskRepository",
        lambda session: (_ for _ in ()).throw(RuntimeError("boom")),
    )
    assert sg._run_task_verify(tmp_path, ttl_minutes=5) == ("TASK-1", 0, [], False)


def test_system_message_and_merge_branches():
    from devcouncil.execution import stop_gate as sg

    assert "ACTIVE TASK" in sg._merge_corrective("claim", "T1", 2, ["a", "b"])
    assert sg._merge_corrective("", None, 0, []) == ""
    assert sg._system_message(
        claim_results=[], blocking_gaps=0, decision="pass", notify_on_pass=False
    ) is None
    msg = sg._system_message(
        claim_results=[], blocking_gaps=0, decision="pass", notify_on_pass=True
    )
    assert msg and "stop-gate" in msg
    msg2 = sg._system_message(
        claim_results=[], blocking_gaps=3, decision="block", notify_on_pass=False
    )
    assert msg2 and "blocked" in msg2


def test_build_more_helpers(tmp_path, monkeypatch):
    from devcouncil.indexing.graph import build as build_mod

    assert build_mod.graph_path(tmp_path).name.endswith("json") or "graph" in str(
        build_mod.graph_path(tmp_path)
    )
    files = ["a.py", "b.ts", "vendor/x.py", "node_modules/y.js", "readme.md"]
    code = build_mod._code_files(files)
    assert "a.py" in code and "b.ts" in code
    assert "readme.md" not in code

    (tmp_path / "a.py").write_text("x=1\n", encoding="utf-8")
    fp = build_mod.content_fingerprint(tmp_path, ["a.py"])
    assert isinstance(fp, str) and len(fp) > 8


# --- wave6 additions (also in test_coverage_wave6.py when scope allows) --------


def test_wave6_run_task_verify_full_path_persists_evidence(tmp_path, monkeypatch):
    from devcouncil.domain.evidence import (
        CommandResult,
        DiffCoverageEvidence,
        DiffEvidence,
        TestEvidence,
    )
    from devcouncil.domain.gap import Gap
    from devcouncil.execution import stop_gate as sg

    task = Task(id="TASK-1", title="t", description="d", status="running")
    gap = Gap(
        id="G1",
        severity="high",
        gap_type="missing_test",
        description="no test",
        blocking=True,
        recommended_fix="add test",
        task_id="TASK-1",
    )
    evidence = [
        CommandResult(command="pytest", exit_code=0, stdout_path="", stderr_path="", summary="ok"),
        DiffCoverageEvidence(task_id="TASK-1", tool="coverage", measured=True),
        DiffEvidence(task_id="TASK-1", changed_files=["a.py"], added_files=[], deleted_files=[], diff_summary="s"),
        TestEvidence(
            requirement_id="R1",
            acceptance_criterion_id="AC1",
            command="pytest",
            status="passed",
            evidence_summary="ok",
        ),
    ]
    saved = {"gaps": 0, "evidence": 0, "cache": None}

    class Sess:
        def get_session(self):
            return self

        def __enter__(self):
            return self

        def __exit__(self, *a):
            return False

    class GapRepo:
        def delete_for_task(self, tid):
            pass

        def save(self, g):
            saved["gaps"] += 1

    class EvRepo:
        def delete_for_task(self, tid):
            pass

        def save_command_result(self, tid, ev):
            saved["evidence"] += 1

        def save_diff_coverage_evidence(self, ev):
            saved["evidence"] += 1

        def save_diff_evidence(self, ev):
            saved["evidence"] += 1

        def save_test_evidence(self, ev, tid):
            saved["evidence"] += 1

    class TaskRepo:
        def get_by_id(self, tid):
            return task

        def save(self, t):
            task.status = t.status

    async def fake_verify(self, t, reqs):
        return [gap], evidence

    monkeypatch.setattr(sg, "active_task_id", lambda _p: "TASK-1")
    monkeypatch.setattr(sg, "load_verify_cache", lambda *a, **k: None)
    monkeypatch.setattr("devcouncil.storage.db.get_db", lambda _p: Sess())
    monkeypatch.setattr("devcouncil.storage.repositories.TaskRepository", lambda s: TaskRepo())
    monkeypatch.setattr("devcouncil.storage.repositories.GapRepository", lambda s: GapRepo())
    monkeypatch.setattr("devcouncil.storage.repositories.EvidenceRepository", lambda s: EvRepo())
    monkeypatch.setattr("devcouncil.storage.repositories.RequirementRepository", lambda s: SimpleNamespace(get_all=lambda: []))
    monkeypatch.setattr("devcouncil.verification.verifier.Verifier.verify_task", fake_verify)
    monkeypatch.setattr(
        "devcouncil.verification.next_actions.split_next_actions",
        lambda gaps: ([SimpleNamespace(action="fix gap", model_dump=lambda: {"action": "fix gap"})], []),
    )

    def record_cache(project_root, **kwargs):
        saved["cache"] = kwargs

    monkeypatch.setattr(sg, "record_verify_cache", record_cache)

    tid, blocking, actions, cached = sg._run_task_verify(tmp_path, ttl_minutes=5)
    assert tid == "TASK-1"
    assert blocking == 1
    assert "fix gap" in actions
    assert cached is False
    assert saved["gaps"] == 1
    assert saved["evidence"] == 4
    assert saved["cache"]["passed"] is False
