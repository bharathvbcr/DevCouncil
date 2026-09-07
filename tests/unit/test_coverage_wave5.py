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


class _StoreProbe:
    """A `DevMapClient` double for the one question `doctor` asks the store."""

    def __init__(self, *, generation_id=7, node_count=12):
        self.generation_id = generation_id
        self.node_count = node_count

    def status(self):
        return self


def test_check_mapping_stack_probes_the_store_not_the_python_graph(tmp_path, monkeypatch):
    """"Does a graph exist" is a `status` call, not a whole-graph read.

    `doctor` asked it twice per run through `load_code_graph`, which
    materialises every node and edge out of the Python `index.sqlite` cache:
    measured on a tmp copy of this repository, p50 2546.8 ms / min 1872.8 ms
    against `try_connect`'s p50 41.2 ms / min 36.4 ms, for a boolean.
    """
    from devcouncil.cli.commands import doctor as doctor_cmd

    graph_path = tmp_path / ".devcouncil" / "graph" / "code_graph.json"
    graph_path.parent.mkdir(parents=True, exist_ok=True)
    graph_path.write_text("{}", encoding="utf-8")

    monkeypatch.setattr(
        "devcouncil.devmap_client.try_connect", lambda root: _StoreProbe()
    )
    rows = doctor_cmd.check_mapping_stack(tmp_path)
    graph_rows = [row for row in rows if row[0] == "Code graph"]
    assert graph_rows and "OK" in graph_rows[0][1], graph_rows


def test_check_mapping_stack_reports_an_export_with_no_store_behind_it(
    tmp_path, monkeypatch
):
    """A readable JSON export whose store is gone is a warning, not an OK."""
    from devcouncil.cli.commands import doctor as doctor_cmd

    graph_path = tmp_path / ".devcouncil" / "graph" / "code_graph.json"
    graph_path.parent.mkdir(parents=True, exist_ok=True)
    graph_path.write_text("{}", encoding="utf-8")

    monkeypatch.setattr("devcouncil.devmap_client.try_connect", lambda root: None)
    rows = doctor_cmd.check_mapping_stack(tmp_path)
    graph_rows = [row for row in rows if row[0] == "Code graph"]
    assert graph_rows and "OK" not in graph_rows[0][1], graph_rows
    assert "devmap" in graph_rows[0][2] or "dev map" in graph_rows[0][2], graph_rows


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


# --- api_routes.py ----------------------------------------------------------
#
# Retired with the module. `indexing/graph/api_routes.py` was a Python
# re-implementation of `devmap routes` / `shape-check` / `api-impact` over
# `load_code_graph`; both its production readers (the MCP route tools and
# `dev map routes` / `shape-check` / `api-impact`) now ask the kernel, so the
# module and the coverage of its private helpers went with it. The canonical
# owner is `devmap_query::api_routes`, whose suite carries the same cases —
# `a_template_literal_parameter_normalises_without_leaving_its_dollar`,
# `a_flask_converter_normalises_whole_rather_than_from_its_colon`,
# `paths_match_segment_wise_and_reject_different_depths`,
# `a_consumer_is_matched_to_its_route_with_the_keys_it_reads`,
# `a_key_the_handler_never_returns_is_a_mismatch`,
# `a_route_nothing_calls_is_not_reported_as_no_risk` — plus several the Python
# copy never had, such as
# `a_truncated_scan_never_reports_a_route_as_safe_to_change`.


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
    # systemMessage is a universal *top-level* field; nesting it inside
    # hookSpecificOutput (as this asserted before) silently dropped the message.
    assert payload["systemMessage"] == "toast"
    assert "hookSpecificOutput" not in payload


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


def test_build_graph_json_max_bytes_honors_config(tmp_path, monkeypatch):
    """The read bound is configurable, and an unreadable config is not "no bound".

    Its sibling `_graph_json_indent` chose the indent of the *Python* writer of
    `code_graph.json`; that writer is deleted (the kernel is the only one), so
    the setting it read has no effect and the helper went with it. The byte
    bound survives, on the reader.
    """
    from devcouncil.indexing.graph import build as graph_build

    cfg = SimpleNamespace(indexing=SimpleNamespace(graph_json_max_bytes=4096))
    monkeypatch.setattr("devcouncil.app.config.load_config", lambda _r: cfg)
    assert graph_build._graph_json_max_bytes(tmp_path) == 4096

    monkeypatch.setattr(
        "devcouncil.app.config.load_config",
        lambda _r: (_ for _ in ()).throw(RuntimeError("no cfg")),
    )
    assert graph_build._graph_json_max_bytes(tmp_path) == 128 * 1024 * 1024


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
    data = json.loads(res.stdout)
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
