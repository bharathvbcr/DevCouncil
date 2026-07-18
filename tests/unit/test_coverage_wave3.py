"""Wave-3 stable unit coverage: logs/rollback/requirements, repair, stop-gate, campaign."""

from __future__ import annotations

from types import SimpleNamespace

from typer.testing import CliRunner

from devcouncil.cli.main import app
from devcouncil.domain.gap import Gap
from devcouncil.domain.requirement import Requirement
from devcouncil.domain.task import Task


runner = CliRunner()


def test_logs_cli_paths(tmp_path, monkeypatch):
    from devcouncil.cli.commands import logs as logs_cmd

    monkeypatch.delenv("DEVCOUNCIL_LOG_DIR", raising=False)
    root = tmp_path
    log = logs_cmd._shared_log(root)
    log.parent.mkdir(parents=True, exist_ok=True)
    log.write_text("alpha line\nbeta ERROR line\ngamma line\n", encoding="utf-8")

    missing = runner.invoke(logs_cmd.app, ["tail", "--project-root", str(root / "none")])
    assert missing.exit_code == 0

    tailed = runner.invoke(
        logs_cmd.app,
        ["tail", "-n", "2", "--grep", "error", "--project-root", str(root)],
    )
    assert tailed.exit_code == 0
    assert "ERROR" in tailed.output

    path = runner.invoke(logs_cmd.app, ["path", "--project-root", str(root)])
    assert path.exit_code == 0
    assert "devcouncil.log" in path.output

    empty_runs = runner.invoke(logs_cmd.app, ["runs", "--project-root", str(root)])
    assert empty_runs.exit_code == 0
    assert "No runs" in empty_runs.output or "No per-run" in empty_runs.output

    runs = root / ".devcouncil" / "runs" / "run-1"
    runs.mkdir(parents=True)
    (runs / "run.log").write_text("run ok\n", encoding="utf-8")
    listed = runner.invoke(logs_cmd.app, ["runs", "--project-root", str(root)])
    assert listed.exit_code == 0
    assert "run-1" in listed.output

    (root / ".devcouncil" / "runs" / "empty").mkdir(exist_ok=True)
    listed2 = runner.invoke(logs_cmd.app, ["runs", "--project-root", str(root)])
    assert listed2.exit_code == 0


def test_rollback_missing_checkpoint(tmp_path, monkeypatch):
    from devcouncil.cli.commands import rollback as rb
    import typer

    class Svc:
        REF_BEFORE = "refs/devcouncil/{task_id}-before"
        REF_AFTER = "refs/devcouncil/{task_id}-after"

        def __init__(self, root):
            self.root = root

        def _ref_exists(self, _ref):
            return False

        def rollback(self, task_id):
            return SimpleNamespace(message="No checkpoint available")

    monkeypatch.setattr(rb, "CheckpointService", Svc)

    def _run(task_id: str):
        ctx = SimpleNamespace(invoked_subcommand=None)
        try:
            rb.rollback(ctx, task_id=task_id, project_root=tmp_path)
            return 0, ""
        except typer.Exit as exc:
            return exc.exit_code, ""

    code, _ = _run("TASK-X")
    assert code == 1

    ck = tmp_path / ".devcouncil" / "checkpoints"
    ck.mkdir(parents=True)
    (ck / "TASK-Y-before.patch").write_text("diff\n", encoding="utf-8")

    class Svc2(Svc):
        def rollback(self, task_id):
            return SimpleNamespace(message="Rollback failed: dirty tree")

    monkeypatch.setattr(rb, "CheckpointService", Svc2)
    code, _ = _run("TASK-Y")
    assert code == 1

    (ck / "TASK-Z-after.patch").write_text("diff\n", encoding="utf-8")

    class Svc3(Svc):
        def rollback(self, task_id):
            return SimpleNamespace(message="failed to apply")

    monkeypatch.setattr(rb, "CheckpointService", Svc3)
    code, _ = _run("TASK-Z")
    assert code == 1

    class SvcOk(Svc):
        def rollback(self, task_id):
            return SimpleNamespace(message="Restored checkpoint")

    monkeypatch.setattr(rb, "CheckpointService", SvcOk)
    (ck / "TASK-OK-before.patch").write_text("x\n", encoding="utf-8")
    code, _ = _run("TASK-OK")
    assert code == 0


def test_requirements_status_helpers_and_cli(tmp_path, monkeypatch):
    from devcouncil.cli.commands import requirements as req_cmd
    from devcouncil.cli.commands.init import initialize_project
    from devcouncil.domain.requirement import AcceptanceCriterion
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import RequirementRepository, TaskRepository

    assert req_cmd._derive_requirement_status("R1", []) == "unmapped"
    tasks = [
        Task(id="T1", title="t", description="d", status="blocked", requirement_ids=["R1"]),
    ]
    assert req_cmd._derive_requirement_status("R1", tasks) == "blocked"
    tasks[0].status = "verified"
    assert req_cmd._derive_requirement_status("R1", tasks) == "verified"
    tasks[0].status = "running"
    assert req_cmd._derive_requirement_status("R1", tasks) == "in_progress"
    tasks[0].status = "planned"
    assert req_cmd._derive_requirement_status("R1", tasks) == "planned"

    monkeypatch.setattr(req_cmd, "get_db", lambda _r: None)
    assert req_cmd._requirements_payload(tmp_path)["initialized"] is False

    monkeypatch.setattr(req_cmd, "get_db", get_db)
    initialize_project(tmp_path, quiet=True, with_map=False, with_skills=False)
    db = get_db(tmp_path)
    with db.get_session() as session:
        RequirementRepository(session).save(
            Requirement(
                id="REQ-1",
                title="Ship",
                description="d",
                priority="high",
                acceptance_criteria=[
                    AcceptanceCriterion(id="AC-1", description="works", verification_method="unit_test")
                ],
            )
        )
        TaskRepository(session).save(
            Task(
                id="TASK-1",
                title="t",
                description="d",
                status="planned",
                requirement_ids=["REQ-1"],
                planned_files=[],
            )
        )

    payload = req_cmd._requirements_payload(tmp_path)
    assert payload["total_count"] == 1
    assert payload["requirements"][0]["status"] == "planned"

    js = runner.invoke(app, ["requirements", "--json", "--project-root", str(tmp_path)])
    assert js.exit_code == 0
    assert "REQ-1" in js.output

    table = runner.invoke(app, ["requirements", "--project-root", str(tmp_path)])
    assert table.exit_code == 0
    assert "REQ-1" in table.output

    monkeypatch.setattr(req_cmd, "get_db", lambda _r: None)
    uninit = runner.invoke(app, ["requirements", "--project-root", str(tmp_path)])
    assert uninit.exit_code == 0


def test_repair_service_generate_plan():
    import asyncio

    from devcouncil.planning.repair_service import RepairOutput, RepairService

    class Router:
        async def complete_structured(self, role, messages, schema):
            assert role == "planner_a"
            assert schema is RepairOutput
            return RepairOutput(
                suggested_tasks=[
                    Task(id="FIX-1", title="fix", description="d", status="planned", planned_files=[])
                ]
            )

    svc = RepairService(Router())  # type: ignore[arg-type]
    gap = Gap(
        id="G1",
        severity="high",
        gap_type="missing_test",
        description="need tests",
        recommended_fix="add tests",
        blocking=True,
    )
    out = asyncio.run(svc.generate_repair_plan([gap], "ctx"))
    assert len(out.suggested_tasks) == 1


def test_evidence_html_render_helpers():
    from devcouncil.artifacts.graph import ArtifactGraph
    from devcouncil.reporting import evidence_html as eh

    assert eh._verdict_class("passed") == "verdict-passed"
    assert eh._verdict_class("unknown") == ""
    assert "proven" in eh._proven_badge(True)
    assert "unproven" in eh._proven_badge(False)
    assert "failed" in eh._status_badge("failed")
    assert "No gaps" in eh._render_gaps([])
    assert "GAP" in eh._render_gaps(
        [{"id": "GAP-1", "blocking": True, "severity": "high", "description": "x", "task_id": "T1"}]
    )
    tasks_html = eh._render_tasks(
        [
            {
                "id": "T1",
                "title": "t",
                "status": "planned",
                "requirement_ids": ["R1"],
                "diffs": [
                    {
                        "changed_files": ["a.py"],
                        "added_files": ["b.py"],
                        "deleted_files": ["c.py"],
                        "diff_summary": "summary",
                    }
                ],
            },
            {"id": "T2", "title": "u", "status": "done", "requirement_ids": [], "diffs": []},
        ]
    )
    assert "Changed:" in tasks_html and "Added:" in tasks_html and "Deleted:" in tasks_html
    assert "No diff evidence" in tasks_html

    html = eh.EvidenceHtmlGenerator.generate(ArtifactGraph())
    assert "DevCouncil Evidence Report" in html


def test_stop_gate_compact_snapshot_paths(tmp_path, monkeypatch):
    from devcouncil.execution import stop_gate as sg
    from devcouncil.execution.stop_gate_history import append_event

    root = tmp_path
    (root / ".devcouncil").mkdir()
    (root / ".devcouncil" / "config.yaml").write_text(
        "project:\n  name: t\nexecution:\n  stop_gate:\n    mode: assist\n",
        encoding="utf-8",
    )
    append_event(root, {"decision": "block", "claim": "x", "blocking_gaps": 1})

    monkeypatch.setattr(sg, "active_task_id", lambda _p: "TASK-1")
    monkeypatch.setattr(sg, "_project_phase", lambda _p: "EXECUTING")
    monkeypatch.setattr(sg, "_task_blocking_summary", lambda _p, _t: (2, ["fix tests", "rerun verify"]))
    monkeypatch.setattr(sg, "last_assistant_sentence", lambda _p: "Done for now.")

    snap = sg.build_compact_snapshot(
        root,
        {"session_id": "s1", "transcript_path": str(root / "t.jsonl")},
    )
    assert snap["task_id"] == "TASK-1"
    assert snap["phase"] == "EXECUTING"
    assert snap["blocking_gaps"] == 2
    assert snap["next_actions"]
    assert snap["last_stop_gate"]["decision"] == "block"
    assert snap["last_assistant_sentence"] == "Done for now."

    sg.write_compact_snapshot(root, {"session_id": "s1"})
    loaded = sg.read_compact_snapshot(root)
    assert loaded and loaded["task_id"] == "TASK-1"
    assert sg.read_compact_snapshot(root / "missing") is None

    sg.record_compact_brief(root, session_id="s1")
    assert sg.recent_compact_brief(root, 3600) is True
    assert sg.recent_compact_brief(root, 0) is False

    brief = sg.last_compact_brief_path(root)
    brief.write_text("[]", encoding="utf-8")
    assert sg.recent_compact_brief(root, 3600) is True

    brief_text = sg.compact_briefing(root, {})
    assert brief_text and "DevCouncil" in brief_text

    class GapObj:
        blocking = True
        description = "gap"

    class FakeDB:
        def get_session(self):
            return self

        def __enter__(self):
            return self

        def __exit__(self, *a):
            return False

    monkeypatch.setattr("devcouncil.storage.db.get_db", lambda _p: FakeDB())
    monkeypatch.setattr(
        "devcouncil.storage.repositories.GapRepository",
        lambda session: SimpleNamespace(get_for_task=lambda _tid: [GapObj()]),
    )
    monkeypatch.setattr(
        "devcouncil.verification.next_actions.split_next_actions",
        lambda gaps: ([SimpleNamespace(action="rerun verify")], []),
    )
    brief2 = sg.session_briefing(root, {})
    assert brief2 and "Blocking gaps" in brief2


def test_campaign_execute_lease_and_qc_errors(tmp_path, monkeypatch):
    from devcouncil.campaign.orchestrator import Campaign, build_coding_executor_factory, build_verifier_fn
    from devcouncil.domain.requirement import AcceptanceCriterion

    req = Requirement(
        id="R1",
        title="r",
        description="d",
        priority="high",
        acceptance_criteria=[
            AcceptanceCriterion(id="AC-1", description="ok", verification_method="unit_test")
        ],
    )
    task = Task(
        id="T1",
        title="analyze deeply",
        description="critical design review",
        status="planned",
        requirement_ids=["R1"],
        planned_files=[],
        difficulty="hard",
    )

    class BoomExec:
        def run_task(self, task, requirements):
            raise RuntimeError("boom")

    camp = Campaign(
        tmp_path,
        goal="g",
        tasks=[task],
        requirements=[req],
        executor_factory=lambda _o: BoomExec(),
        verify_fn=lambda *_: (_ for _ in ()).throw(RuntimeError("bad verify")),
        max_parallel=1,
        verify_serialized=False,
        use_leases=True,
        on_task_update=lambda t: None,
    )
    monkeypatch.setattr(camp, "_checkout_task", lambda owner, tid: None)
    ok, msg = camp._execute("worker-1", task)
    assert ok is False and "lease" in msg

    monkeypatch.setattr(camp, "_checkout_task", lambda owner, tid: "token")
    monkeypatch.setattr(camp, "_release_task", lambda tid, tok: None)
    ok2, msg2 = camp._execute("worker-1", task)
    assert ok2 is False and "executor error" in msg2

    passed, gaps = camp._quality_control(task)
    assert passed is False
    assert gaps and "verifier error" in gaps[0]

    assert camp._reqs_for(task)[0].id == "R1"
    bare = Task(id="T2", title="t", description="d", status="planned", planned_files=[])
    assert camp._reqs_for(bare) == [req]

    # Restore real lease helpers (earlier patches replaced bound methods).
    camp._checkout_task = Campaign._checkout_task.__get__(camp, Campaign)
    camp._release_task = Campaign._release_task.__get__(camp, Campaign)

    monkeypatch.setattr(
        "devcouncil.execution.lease_ops.checkout_task_payload",
        lambda *a, **k: {"ok": False, "error": "busy"},
    )
    assert camp._checkout_task("w1", "T1") is None
    monkeypatch.setattr(
        "devcouncil.execution.lease_ops.checkout_task_payload",
        lambda *a, **k: {"ok": True, "lease_token": "tok"},
    )
    assert camp._checkout_task("w1", "T1") == "tok"
    monkeypatch.setattr(
        "devcouncil.execution.lease_ops.release_task_payload",
        lambda *a, **k: {"ok": True},
    )
    camp._release_task("T1", "tok")

    monkeypatch.setattr(
        "devcouncil.telemetry.cost.group_cost",
        lambda *a, **k: (_ for _ in ()).throw(RuntimeError("x")),
    )
    camp._refresh_cost()
    camp._cost_budget_usd = 1.0
    camp._state.cost_usd = 2.0
    assert camp._over_budget() is True
    camp._cost_budget_usd = None
    assert camp._over_budget() is False

    fac = build_coding_executor_factory(tmp_path, "codex")
    assert callable(fac)
    monkeypatch.setattr(
        "devcouncil.executors.coding_cli.CodingCliExecutor",
        lambda *a, **k: SimpleNamespace(run_task=lambda *aa, **kk: None),
    )
    assert fac("worker-1") is not None

    class Ver:
        async def verify_task(self, task, requirements):
            return [SimpleNamespace(blocking=True, description="miss")], []

    monkeypatch.setattr("devcouncil.verification.verifier.Verifier", lambda *a, **k: Ver())
    vfn = build_verifier_fn(tmp_path)
    okv, gapv = vfn(task, [req])
    assert okv is False
    assert gapv
