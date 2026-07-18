"""Wave-6 stable coverage: stop_gate verify/cache/compact branches, build helpers,
hook emitters, api_routes scanners, map_artifacts, and communities."""

from __future__ import annotations

import ast
import asyncio
import os
from types import SimpleNamespace
from unittest.mock import MagicMock

import pytest
from typer.testing import CliRunner

from devcouncil.app.errors import GatingError
from devcouncil.cli.main import app
from devcouncil.domain.gap import Gap
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.execution.permissions import PermissionManager, PermissionPolicy
from devcouncil.execution.stop_gate_state import (
    get_block_count,
    increment_block_count,
    state_path,
)
from devcouncil.indexing.graph.pdg.cdg import _is_guard_block, build_cdg
from devcouncil.indexing.graph.pdg.cfg import CFGResult
from devcouncil.indexing.graph.pdg.schema import BasicBlock, CFGEdge
from devcouncil.utils.json_persist import write_json

runner = CliRunner()


# --- stop_gate_state -----------------------------------------------------------


def test_stop_gate_state_edge_cases(tmp_path, monkeypatch):
    assert get_block_count(tmp_path, "") == 0
    assert increment_block_count(tmp_path, "") == 0

    path = state_path(tmp_path)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("[]", encoding="utf-8")
    assert get_block_count(tmp_path, "s1") == 0

    write_json(path, {"sessions": "bad"})
    assert get_block_count(tmp_path, "s1") == 0

    write_json(path, {"sessions": {"s1": -3}})
    assert get_block_count(tmp_path, "s1") == 0

    write_json(path, {"sessions": {"s1": "x"}})
    assert get_block_count(tmp_path, "s1") == 0

    path.write_text("null", encoding="utf-8")
    assert increment_block_count(tmp_path, "s2") == 1

    write_json(path, {"sessions": []})
    assert increment_block_count(tmp_path, "s3") == 1

    write_json(path, {"sessions": {"s4": 2}})

    def boom(*_a, **_k):
        raise OSError("disk full")

    monkeypatch.setattr("devcouncil.execution.stop_gate_state.write_json", boom)
    assert increment_block_count(tmp_path, "s4") == 3


# --- CDG -----------------------------------------------------------------------


def test_build_cdg_branch_and_guard():
    blocks = [
        BasicBlock(id="b0", start_line=1, end_line=1, text="if x:"),
        BasicBlock(id="b1", start_line=2, end_line=2, text="return 1"),
        BasicBlock(id="b2", start_line=3, end_line=3, text="y = 2"),
    ]
    edges = [
        CFGEdge(source="b0", target="b1", kind="true"),
        CFGEdge(source="b0", target="b2", kind="false"),
        CFGEdge(source="b0", target="b1", kind="true"),
        CFGEdge(source="b1", target="b2", kind="fallthrough"),
        CFGEdge(source="b0", target="b2", kind="loop"),
        CFGEdge(source="b0", target="b1", kind="exception"),
    ]
    cfg = CFGResult(blocks=blocks, edges=edges, entry="b0", exits=["b1"])
    cdg = build_cdg(cfg, ast.parse("def f():\n  return 1\n").body[0])
    assert any(e.guard for e in cdg)
    assert any(e.branch == "T" for e in cdg)
    assert any(e.branch == "F" for e in cdg)
    assert _is_guard_block(blocks[1]) is True
    assert _is_guard_block(blocks[2]) is False


# --- logs CLI ------------------------------------------------------------------


def test_logs_runs_empty_and_populated(tmp_path):
    runs = tmp_path / ".devcouncil" / "runs"
    runs.mkdir(parents=True)
    empty = runner.invoke(app, ["logs", "runs", "--project-root", str(tmp_path)])
    assert empty.exit_code == 0
    assert "No per-run logs" in empty.output

    run_dir = runs / "run-1"
    run_dir.mkdir()
    (run_dir / "run.log").write_text("line-a\nline-b\nerror here\n", encoding="utf-8")
    listed = runner.invoke(app, ["logs", "runs", "-n", "5", "--project-root", str(tmp_path)])
    assert listed.exit_code == 0
    assert "run-1" in listed.output

    tailed = runner.invoke(
        app,
        ["logs", "tail", "--run", "run-1", "-n", "2", "-g", "error", "--project-root", str(tmp_path)],
    )
    assert tailed.exit_code == 0
    assert "error here" in tailed.output

    path_cmd = runner.invoke(app, ["logs", "path", "--project-root", str(tmp_path)])
    assert path_cmd.exit_code == 0


def test_logs_follow_reads_new_lines(tmp_path, monkeypatch):
    import devcouncil.cli.commands.logs as logs_cmd

    log = tmp_path / "shared.log"
    log.write_text("seed\n", encoding="utf-8")
    monkeypatch.setattr(logs_cmd, "_shared_log", lambda _r: log)

    class FakeHandle:
        def __init__(self):
            self._n = 0

        def seek(self, *_a):
            return None

        def readline(self):
            self._n += 1
            if self._n == 1:
                return "fresh\n"
            raise KeyboardInterrupt()

        def __enter__(self):
            return self

        def __exit__(self, *exc):
            return False

    monkeypatch.setattr("builtins.open", lambda *a, **k: FakeHandle())
    monkeypatch.setattr(logs_cmd.time, "sleep", lambda *_: None)
    result = runner.invoke(app, ["logs", "tail", "-f", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "fresh" in result.output


# --- shell interactive ---------------------------------------------------------


def test_shell_interactive_loop(tmp_path, monkeypatch):
    from devcouncil.cli.commands.init import initialize_project
    import devcouncil.cli.commands.shell as shell_cmd
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import TaskRepository

    initialize_project(tmp_path, quiet=True, with_map=False, with_skills=False)
    db = get_db(tmp_path)
    with db.get_session() as session:
        TaskRepository(session).save(
            Task(id="TASK-I", title="i", description="d", status="planned", planned_files=[])
        )

    calls = {"n": 0}

    class OkSess:
        def start(self, force=False):
            return None

        def finish(self):
            return None

        def run_one(self, cmd):
            return 1 if cmd == "bad" else 0

    def fake_input(_prompt=""):
        calls["n"] += 1
        seq = ["", "bad", "exit"]
        if calls["n"] > len(seq):
            raise EOFError()
        return seq[calls["n"] - 1]

    monkeypatch.setattr(shell_cmd, "GuardedShellSession", lambda *a, **k: OkSess())
    monkeypatch.setattr("builtins.input", fake_input)
    result = runner.invoke(app, ["shell", "TASK-I", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "exited with 1" in result.output


def test_shell_interactive_eof(tmp_path, monkeypatch):
    from devcouncil.cli.commands.init import initialize_project
    import devcouncil.cli.commands.shell as shell_cmd
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import TaskRepository

    initialize_project(tmp_path, quiet=True, with_map=False, with_skills=False)
    db = get_db(tmp_path)
    with db.get_session() as session:
        TaskRepository(session).save(
            Task(id="TASK-E", title="e", description="d", status="planned", planned_files=[])
        )

    class OkSess:
        def start(self, force=False):
            return None

        def finish(self):
            return None

        def run_one(self, cmd):
            return 0

    monkeypatch.setattr(shell_cmd, "GuardedShellSession", lambda *a, **k: OkSess())
    monkeypatch.setattr("builtins.input", lambda *_a, **_k: (_ for _ in ()).throw(EOFError()))
    result = runner.invoke(app, ["shell", "TASK-E", "--project-root", str(tmp_path)])
    assert result.exit_code == 0


# --- permissions ---------------------------------------------------------------


def test_permissions_ignore_planned_and_shell_deny(tmp_path):
    ignore = tmp_path / ".devcouncilignore"
    ignore.write_text("# comment\nsecrets/*\n\n", encoding="utf-8")
    mgr = PermissionManager(PermissionPolicy(allowed_shell_commands=["echo"]), tmp_path)
    task = Task(
        id="T1",
        title="t",
        description="d",
        status="planned",
        planned_files=[PlannedFile(path="src/*.py", reason="r", allowed_change="modify")],
        allowed_commands=["pytest"],
    )
    assert mgr.is_file_change_allowed("secrets/key.txt", task) is False
    planned = mgr._planned_file_for("src/app.py", task)
    assert planned is not None
    assert mgr._planned_file_for("other/x.py", task) is None

    with pytest.raises(GatingError):
        mgr.validate_action("shell", "rm -rf /", task)

    mgr.validate_action("shell", "pytest -q", task)

    ignore.write_text("x\n", encoding="utf-8")
    ignore.chmod(0)
    try:
        mgr2 = PermissionManager(PermissionPolicy(), tmp_path)
        assert mgr2.dynamic_ignores == []
    finally:
        ignore.chmod(0o644)


# --- repair CLI ----------------------------------------------------------------


def test_repair_no_db_and_no_gaps(tmp_path, monkeypatch):
    import devcouncil.cli.commands.repair as repair_cmd
    from devcouncil.storage.db import get_db

    monkeypatch.setattr(repair_cmd, "get_db", lambda _r: None)
    asyncio.run(repair_cmd.run_repair_flow(tmp_path))

    from devcouncil.cli.commands.init import initialize_project

    monkeypatch.setattr(repair_cmd, "get_db", get_db)
    initialize_project(tmp_path, quiet=True, with_map=False, with_skills=False)
    asyncio.run(repair_cmd.run_repair_flow(tmp_path))


def test_repair_with_llm_and_gaps(tmp_path, monkeypatch):
    import devcouncil.cli.commands.repair as repair_cmd
    from devcouncil.cli.commands.init import initialize_project
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import GapRepository, TaskRepository

    initialize_project(tmp_path, quiet=True, with_map=False, with_skills=False)
    db = get_db(tmp_path)
    with db.get_session() as session:
        TaskRepository(session).save(
            Task(id="TASK-R", title="r", description="d", status="blocked", planned_files=[])
        )
        GapRepository(session).save(
            Gap(
                id="G1",
                task_id="TASK-R",
                requirement_id="REQ-1",
                description="blocked",
                severity="high",
                gap_type="test_failed",
                recommended_fix="fix it",
                blocking=True,
            )
        )

    cfg = SimpleNamespace(
        models=SimpleNamespace(provider="openrouter", roles={}),
        provider=SimpleNamespace(),
    )
    monkeypatch.setattr(repair_cmd, "load_config", lambda _r: cfg)
    monkeypatch.setattr(repair_cmd, "validate_model_provider", lambda _p: "openrouter")
    monkeypatch.setattr(repair_cmd, "get_api_key", lambda *_a, **_k: "key")
    monkeypatch.setattr(repair_cmd, "create_provider", lambda *a, **k: MagicMock())
    monkeypatch.setattr(repair_cmd, "ModelRouter", lambda *a, **k: MagicMock())

    async def fake_plan(gaps, ctx):
        return SimpleNamespace(
            suggested_tasks=[
                Task(id="FIX", title="fix", description="d", status="planned", planned_files=[])
            ]
        )

    repair_svc = MagicMock()
    repair_svc.generate_repair_plan = fake_plan
    monkeypatch.setattr(repair_cmd, "RepairService", lambda *_a, **_k: repair_svc)
    monkeypatch.setattr(
        "devcouncil.planning.correction_manifest.write_correction_manifest",
        lambda *a, **k: tmp_path / "manifest.yaml",
    )
    monkeypatch.setattr(
        repair_cmd.ContextBuilder,
        "get_structure_summary",
        lambda self: "ctx",
    )
    asyncio.run(repair_cmd.run_repair_flow(tmp_path))

    ctx = MagicMock()
    ctx.invoked_subcommand = "noop"
    repair_cmd.repair(ctx, project_root=tmp_path)


# --- mailbox corrupt / lock recovery ------------------------------------------


def test_mailbox_corrupt_yaml_and_write_cleanup(tmp_path, monkeypatch):
    from devcouncil.campaign import mailbox as mb

    box = mb.Mailbox(tmp_path)
    path = box.path_for("worker")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(": not: valid: [[[", encoding="utf-8")
    assert box._read_raw("worker") == []

    # Exercise _write_atomic finally cleanup when tmp survives replace failure.
    real_replace = os.replace
    calls = {"n": 0}

    def flaky_replace(src, dst):
        calls["n"] += 1
        if calls["n"] == 1:
            raise OSError("busy")
        return real_replace(src, dst)

    monkeypatch.setattr(mb.os, "replace", flaky_replace)
    with pytest.raises(OSError):
        box.send("worker", "hi", from_agent="director")
    # Second send after cleanup succeeds
    monkeypatch.setattr(mb.os, "replace", real_replace)
    msg = box.send("worker", "hi2", from_agent="director")
    assert msg.id
