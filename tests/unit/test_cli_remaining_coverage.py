import asyncio
import json
import os
import subprocess
import sys
from contextlib import contextmanager
from pathlib import Path
from types import SimpleNamespace

import pytest
import typer
from typer.testing import CliRunner

from devcouncil.cli.commands import agents as agents_cmd
from devcouncil.cli.commands import config as config_cmd
from devcouncil.cli.commands import go as go_cmd
from devcouncil.cli.commands import hook as hook_cmd
from devcouncil.cli.commands import integrate as integrate_cmd
from devcouncil.cli.commands import plan as plan_cmd
from devcouncil.cli.commands import report as report_cmd
from devcouncil.cli.commands import run as run_cmd
from devcouncil.cli.commands import runs as runs_cmd
from devcouncil.cli.commands import okf as okf_cmd
from devcouncil.cli.commands import setup as setup_cmd
from devcouncil.cli.commands import verify as verify_cmd
from devcouncil.cli.commands import watch as watch_cmd
from devcouncil.cli.main import app
from devcouncil.domain.evidence import (
    CommandResult,
    DiffCoverageEvidence,
    DiffEvidence,
    TestEvidence,
)
from devcouncil.domain.gap import Gap
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.llm.provider import ProviderRequestError


runner = CliRunner()


class _FakeDb:
    @contextmanager
    def get_session(self):
        yield "session"


def _gap(
    gap_id: str = "GAP-1",
    *,
    blocking: bool = True,
    gap_type: str = "test_failed",
    ac_id: str | None = None,
    method: str | None = None,
) -> Gap:
    return Gap(
        id=gap_id,
        severity="high",
        gap_type=gap_type,  # type: ignore[arg-type]
        task_id="TASK-1",
        description=f"gap {gap_id}",
        recommended_fix="fix it",
        blocking=blocking,
        acceptance_criterion_id=ac_id,
        expected_verification_method=method,
    )


def _task(task_id: str = "TASK-1", status: str = "planned") -> Task:
    return Task(
        id=task_id,
        title="Task",
        description="Do the thing",
        requirement_ids=["REQ-1"],
        acceptance_criterion_ids=["AC-1"],
        planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
        expected_tests=["pytest -q"],
        allowed_commands=["python -m pytest"],
        status=status,  # type: ignore[arg-type]
    )


def test_run_verify_after_execution_persists_all_evidence_types(monkeypatch, tmp_path):
    saved = {"gaps": [], "command": [], "diff": [], "coverage": [], "test": [], "deleted": []}
    task = _task()
    gap = _gap()
    evidence = [
        CommandResult(command="pytest", exit_code=0, stdout_path="out", stderr_path="err", summary="ok"),
        DiffEvidence(task_id=task.id, changed_files=["a.py"], added_files=[], deleted_files=[], diff_summary="diff"),
        DiffCoverageEvidence(task_id=task.id, measured=True, changed_lines=1, covered_lines=1),
        TestEvidence(
            requirement_id="REQ-1",
            acceptance_criterion_id="AC-1",
            command="pytest",
            status="passed",
            evidence_summary="passed",
        ),
    ]

    class FakeVerifier:
        def __init__(self, root, router=None):
            self.root = root
            self.router = router

        async def verify_task(self, task_arg, reqs_arg):
            assert task_arg is task
            assert reqs_arg == ["req"]
            return [gap], evidence

    class FakeGapRepository:
        def __init__(self, session):
            assert session == "session"

        def delete_for_task(self, task_id):
            saved["deleted"].append(("gaps", task_id))

        def save(self, item):
            saved["gaps"].append(item.id)

    class FakeEvidenceRepository:
        def __init__(self, session):
            assert session == "session"

        def delete_for_task(self, task_id):
            saved["deleted"].append(("evidence", task_id))

        def save_command_result(self, task_id, item):
            saved["command"].append((task_id, item.command))

        def save_diff_evidence(self, item):
            saved["diff"].append(item.task_id)

        def save_diff_coverage_evidence(self, item):
            saved["coverage"].append(item.task_id)

        def save_test_evidence(self, item, task_id):
            saved["test"].append((task_id, item.acceptance_criterion_id))

    monkeypatch.setattr(run_cmd, "Verifier", FakeVerifier)
    monkeypatch.setattr(run_cmd, "GapRepository", FakeGapRepository)
    monkeypatch.setattr(run_cmd, "EvidenceRepository", FakeEvidenceRepository)

    verified = run_cmd._verify_after_execution("session", task, ["req"], router="router", project_root=tmp_path)

    assert verified is False
    assert task.status == "blocked"
    assert saved == {
        "gaps": ["GAP-1"],
        "command": [("TASK-1", "pytest")],
        "diff": ["TASK-1"],
        "coverage": [],
        "test": [("TASK-1", "AC-1")],
        "deleted": [("gaps", "TASK-1"), ("evidence", "TASK-1")],
    }


def test_verify_after_execution_marks_verified_without_blocking_gaps(monkeypatch, tmp_path):
    task = _task()

    class FakeVerifier:
        def __init__(self, root, router=None):
            pass

        async def verify_task(self, task_arg, reqs_arg):
            return [_gap(blocking=False)], []

    class FakeRepo:
        def __init__(self, session):
            pass

        def delete_for_task(self, task_id):
            pass

        def save(self, item):
            pass

    monkeypatch.setattr(run_cmd, "Verifier", FakeVerifier)
    monkeypatch.setattr(run_cmd, "GapRepository", FakeRepo)
    monkeypatch.setattr(run_cmd, "EvidenceRepository", FakeRepo)

    assert run_cmd._verify_after_execution("session", task, [], project_root=tmp_path) is True
    assert task.status == "verified"


def test_run_live_review_after_execution_success_and_skip_branches(monkeypatch, tmp_path):
    transcript = tmp_path / "session.jsonl"
    transcript.write_text("{}\n", encoding="utf-8")
    calls = []

    monkeypatch.setattr(
        run_cmd,
        "load_config",
        lambda root: SimpleNamespace(integrations=SimpleNamespace(live_review=SimpleNamespace(enabled=True))),
    )
    monkeypatch.setattr(watch_cmd, "_resolve_transcript", lambda root, client, latest=False: transcript)
    monkeypatch.setattr("devcouncil.live.transcripts.latest_assistant_turn", lambda path, client: SimpleNamespace(turn_id="t1"))

    async def fake_review(turn, root, client, use_llm, task_id=None):
        return SimpleNamespace(id="CARD-1", verdict="Approved", task_id=task_id, client=client, status="open")

    monkeypatch.setattr(watch_cmd, "_review_turn", fake_review)
    monkeypatch.setattr(watch_cmd, "_save_card_once", lambda root, card, persist, force: (tmp_path / "card.json", False))
    monkeypatch.setattr(
        watch_cmd,
        "_log_card_reviewed",
        lambda root, card, path, duplicate, source: calls.append((card.id, path.name, duplicate, source)),
    )

    run_cmd._run_live_review_after_execution(tmp_path, "claude", "TASK-1")
    assert calls == [("CARD-1", "card.json", False, "e2e")]

    monkeypatch.setattr(
        run_cmd,
        "load_config",
        lambda root: SimpleNamespace(integrations=SimpleNamespace(live_review=SimpleNamespace(enabled=False))),
    )
    run_cmd._run_live_review_after_execution(tmp_path, "claude", "TASK-1")
    assert calls == [("CARD-1", "card.json", False, "e2e")]


def test_run_command_manual_missing_and_gate_branches(monkeypatch, tmp_path):
    monkeypatch.setattr(run_cmd, "initialize_project", lambda root, quiet=True: None)
    monkeypatch.setattr(run_cmd, "set_log_dir", lambda root: None, raising=False)
    monkeypatch.setattr(run_cmd, "get_db", lambda root: None)
    no_db = runner.invoke(app, ["run", "TASK-1", "--project-root", str(tmp_path)])
    assert no_db.exit_code == 0
    assert "state is unavailable" in no_db.output

    saved = []
    task = _task()

    class FakeTaskRepo:
        def __init__(self, session):
            pass

        def get_by_id(self, task_id):
            return task if task_id == "TASK-1" else None

        def save(self, item):
            saved.append((item.id, item.status))

    class PassingGate:
        def check_task_ready(self, task_arg, root):
            return SimpleNamespace(passed=True, gaps=[])

    monkeypatch.setattr(run_cmd, "get_db", lambda root: _FakeDb())
    monkeypatch.setattr(run_cmd, "TaskRepository", FakeTaskRepo)
    monkeypatch.setattr("devcouncil.gating.policy.GatePolicy", PassingGate)
    monkeypatch.setattr(run_cmd, "_capture_before_snapshot", lambda task_id, project_root: None)
    monkeypatch.setattr(run_cmd, "_record_project_phase", lambda session, phase: saved.append(("phase", phase.value)))

    missing = runner.invoke(app, ["run", "TASK-404", "--project-root", str(tmp_path)])
    manual = runner.invoke(app, ["run", "TASK-1", "--project-root", str(tmp_path), "--executor", "manual"])

    assert missing.exit_code == 0
    assert "Task TASK-404 not found" in missing.output
    assert manual.exit_code == 0
    assert "marked as RUNNING" in manual.output
    assert ("TASK-1", "running") in saved

    class FailingGate:
        def check_task_ready(self, task_arg, root):
            return SimpleNamespace(passed=False, gaps=[_gap()])

    monkeypatch.setattr("devcouncil.gating.policy.GatePolicy", FailingGate)
    blocked = runner.invoke(app, ["run", "TASK-1", "--project-root", str(tmp_path)])
    assert blocked.exit_code == 0
    assert "not ready" in blocked.output
    assert "gap GAP-1" in blocked.output


def _install_run_ready(monkeypatch, tmp_path, task):
    saved = []

    class FakeTaskRepo:
        def __init__(self, session):
            pass

        def get_by_id(self, task_id):
            return task if task_id == task.id else None

        def save(self, item):
            saved.append((item.id, item.status))

    class FakeReqRepo:
        def __init__(self, session):
            pass

        def get_all(self):
            return ["req"]

    class PassingGate:
        def check_task_ready(self, task_arg, root):
            return SimpleNamespace(passed=True, gaps=[])

    class FakeCheckpointService:
        def __init__(self, root):
            pass

        def create_before(self, task_id):
            return SimpleNamespace(patch_path=tmp_path / "before.patch", git_ref_created=False, ref=None)

    monkeypatch.setattr(run_cmd, "initialize_project", lambda root, quiet=True: None)
    monkeypatch.setattr(run_cmd, "get_db", lambda root: _FakeDb())
    monkeypatch.setattr(run_cmd, "TaskRepository", FakeTaskRepo)
    monkeypatch.setattr(run_cmd, "RequirementRepository", FakeReqRepo)
    monkeypatch.setattr("devcouncil.gating.policy.GatePolicy", PassingGate)
    monkeypatch.setattr("devcouncil.execution.checkpoints.CheckpointService", FakeCheckpointService)
    monkeypatch.setattr(run_cmd, "_capture_after_patch", lambda task_id, root: None)
    monkeypatch.setattr(run_cmd, "_record_project_phase", lambda session, phase: saved.append(("phase", phase.value)))
    monkeypatch.setattr(run_cmd, "_build_verification_router", lambda root: "router")
    monkeypatch.setattr(run_cmd, "_record_agent_verification", lambda *a, **k: saved.append(("agent", a[2], a[4])))
    monkeypatch.setattr(run_cmd, "_run_live_review_after_execution", lambda *a, **k: saved.append(("live", a[1], a[2])))
    return saved


def test_run_coding_cli_mini_openhands_and_native_branches(monkeypatch, tmp_path):
    task = _task()
    saved = _install_run_ready(monkeypatch, tmp_path, task)
    verify_results = iter([True, False, True])
    monkeypatch.setattr(
        run_cmd,
        "_verify_after_execution",
        lambda session, task_arg, reqs, router=None, project_root=Path("."): next(verify_results),
    )

    class FakeCodingCliExecutor:
        def __init__(self, root, client, profile=None, stream_output=None):
            self.last_run_id = "run-1"
            self.last_transcript_path = root / ".devcouncil" / "runs" / "run-1" / "transcript.txt"
            run_dir = root / ".devcouncil" / "runs" / "run-1"
            run_dir.mkdir(parents=True, exist_ok=True)
            (run_dir / "run.log").write_text("log", encoding="utf-8")

        def run_task(self, task_arg, reqs):
            return SimpleNamespace(success=True, message="ok")

    monkeypatch.setattr(run_cmd, "CodingCliExecutor", FakeCodingCliExecutor)
    coding = runner.invoke(
        app,
        ["run", "TASK-1", "--executor", "claude", "--profile", "prod", "--stream", "--project-root", str(tmp_path)],
    )
    assert coding.exit_code == 0
    assert "CLAUDE finished and task TASK-1 verified" in coding.output
    assert "Run artifacts" in coding.output

    class FailingCodingCliExecutor(FakeCodingCliExecutor):
        def run_task(self, task_arg, reqs):
            return SimpleNamespace(success=False, message="not installed")

    monkeypatch.setattr(run_cmd, "CodingCliExecutor", FailingCodingCliExecutor)
    failed_cli = runner.invoke(app, ["run", "TASK-1", "--executor", "claude", "--project-root", str(tmp_path)])
    assert failed_cli.exit_code == 0
    assert "failed to start or execute: not installed" in failed_cli.output

    class FakeMiniExecutor:
        def __init__(self, root):
            pass

        def run_task(self, task_arg, reqs):
            return SimpleNamespace(success=True)

    monkeypatch.setattr(run_cmd, "MiniSWEExecutor", FakeMiniExecutor)
    mini = runner.invoke(app, ["run", "TASK-1", "--executor", "mini", "--project-root", str(tmp_path)])
    assert mini.exit_code == 0
    assert "blocked by verification gaps" in mini.output

    class FakeOpenHandsExecutor:
        def __init__(self, root):
            pass

        def run_task(self, task_arg, reqs):
            return SimpleNamespace(success=True)

    monkeypatch.setattr(run_cmd, "OpenHandsExecutor", FakeOpenHandsExecutor)
    openhands = runner.invoke(app, ["run", "TASK-1", "--executor", "openhands", "--project-root", str(tmp_path)])
    assert openhands.exit_code == 0
    assert "OpenHands finished and task TASK-1 verified" in openhands.output

    class FailingMiniExecutor(FakeMiniExecutor):
        def run_task(self, task_arg, reqs):
            return SimpleNamespace(success=False)

    monkeypatch.setattr(run_cmd, "MiniSWEExecutor", FailingMiniExecutor)
    mini_failed = runner.invoke(app, ["run", "TASK-1", "--executor", "mini", "--project-root", str(tmp_path)])
    assert "mini-SWE-agent failed to start" in mini_failed.output

    monkeypatch.setattr(run_cmd, "load_config", lambda root: (_ for _ in ()).throw(FileNotFoundError("missing config")))
    native_config = runner.invoke(app, ["run", "TASK-1", "--executor", "native", "--project-root", str(tmp_path)])
    assert native_config.exit_code == 0
    assert "missing config" in native_config.output
    assert ("agent", "claude", True) in saved


def test_verify_reconciliation_and_print_limits(capsys):
    gaps = [
        _gap("DROP", gap_type="acceptance_criteria_unproven", ac_id="AC-1"),
        _gap("KEEP", gap_type="test_failed", ac_id="AC-1"),
        _gap("ADVISORY", blocking=False, gap_type="acceptance_criteria_unproven", ac_id="AC-2"),
    ]
    kept = verify_cmd.reconcile_cross_task_acceptance(gaps, {"AC-1"})
    assert [gap.id for gap in kept] == ["KEEP", "ADVISORY"]

    verify_cmd._print_task_result("TASK-OK", [])
    verify_cmd._print_task_result("TASK-MANY", [_gap(f"GAP-{idx}") for idx in range(25)])
    out = capsys.readouterr().out
    assert "verified successfully" in out
    assert "Showing first 20 of 25 gaps" in out
    assert "BLOCKED" in out


def test_verify_sandbox_json_paths(monkeypatch, tmp_path, capsys):
    tasks = [_task("TASK-FAIL"), _task("TASK-PASS")]
    saved = []
    phases = []

    class FakeTaskRepo:
        def __init__(self, session):
            pass

        def get_all(self):
            return tasks

        def get_by_id(self, task_id):
            return next((task for task in tasks if task.id == task_id), None)

        def save(self, task):
            saved.append((task.id, task.status))

    class FakeReqRepo:
        def __init__(self, session):
            pass

        def get_all(self):
            return ["req"]

    class FakeStateRepo:
        def __init__(self, session):
            pass

        def record_phase(self, phase):
            phases.append(phase)

    class EmptyRepo:
        def __init__(self, session):
            pass

        def delete_for_task(self, task_id):
            pass

    class FakeSandbox:
        def run(self, task, commands, reqs):
            if task.id == "TASK-FAIL":
                return SimpleNamespace(status="failed", commands=commands)
            return SimpleNamespace(status="passed", commands=commands)

    monkeypatch.setattr(verify_cmd, "initialize_project", lambda root, quiet=True: None)
    monkeypatch.setattr(verify_cmd, "set_log_dir", lambda root: None, raising=False)
    monkeypatch.setattr(verify_cmd, "get_db", lambda root: _FakeDb())
    monkeypatch.setattr(verify_cmd, "TaskRepository", FakeTaskRepo)
    monkeypatch.setattr(verify_cmd, "RequirementRepository", FakeReqRepo)
    monkeypatch.setattr(verify_cmd, "GapRepository", EmptyRepo)
    monkeypatch.setattr(verify_cmd, "EvidenceRepository", EmptyRepo)
    monkeypatch.setattr(verify_cmd, "StateRepository", FakeStateRepo)
    monkeypatch.setattr("devcouncil.verification.sandbox.get_sandbox", lambda sandbox, root: FakeSandbox())

    with pytest.raises(typer.Exit) as exc:
        verify_cmd.verify(task_id=None, sandbox="docker", json_format=True, project_root=tmp_path)

    assert exc.value.exit_code == 1
    assert ("TASK-FAIL", "blocked") in saved
    assert ("TASK-PASS", "verified") in saved
    assert phases[-1].endswith("BLOCKED")
    assert '"blocked_tasks": 1' in capsys.readouterr().out


def test_verify_sandbox_unsupported_and_missing_tasks(monkeypatch, tmp_path, capsys):
    class FakeTaskRepo:
        def __init__(self, session):
            pass

        def get_all(self):
            return []

        def get_by_id(self, task_id):
            return _task(task_id)

    class FakeReqRepo:
        def __init__(self, session):
            pass

        def get_all(self):
            return []

    class EmptyRepo:
        def __init__(self, session):
            pass

    monkeypatch.setattr(verify_cmd, "initialize_project", lambda root, quiet=True: None)
    monkeypatch.setattr(verify_cmd, "set_log_dir", lambda root: None, raising=False)
    monkeypatch.setattr(verify_cmd, "get_db", lambda root: _FakeDb())
    monkeypatch.setattr(verify_cmd, "TaskRepository", FakeTaskRepo)
    monkeypatch.setattr(verify_cmd, "RequirementRepository", FakeReqRepo)
    monkeypatch.setattr(verify_cmd, "GapRepository", EmptyRepo)
    monkeypatch.setattr(verify_cmd, "EvidenceRepository", EmptyRepo)
    monkeypatch.setattr(
        "devcouncil.verification.sandbox.get_sandbox",
        lambda sandbox, root: SimpleNamespace(run=lambda task, commands, reqs: SimpleNamespace(status="unsupported")),
    )

    verify_cmd.verify(task_id=None, sandbox="local", json_format=True, project_root=tmp_path)
    verify_cmd.verify(task_id="TASK-1", sandbox="nix", json_format=True, project_root=tmp_path)
    out = capsys.readouterr().out
    assert "No tasks found to verify" in out
    assert "Sandbox nix is unavailable" in out


def test_verify_local_reconciles_cross_task_acceptance_and_json(monkeypatch, tmp_path, capsys):
    tasks = [_task("TASK-1"), _task("TASK-2")]
    reqs = ["req"]
    saved_tasks = []
    saved_gaps = []
    deleted = []
    evidence_saved = []
    phases = []
    events = []

    class FakeTaskRepo:
        def __init__(self, session):
            pass

        def get_all(self):
            return tasks

        def get_by_id(self, task_id):
            return next((task for task in tasks if task.id == task_id), None)

        def save(self, task):
            saved_tasks.append((task.id, task.status))

    class FakeReqRepo:
        def __init__(self, session):
            pass

        def get_all(self):
            return reqs

    class FakeGapRepo:
        def __init__(self, session):
            pass

        def delete_for_task(self, task_id):
            deleted.append(("gap", task_id))

        def save(self, gap):
            saved_gaps.append((gap.task_id, gap.id))

    class FakeEvidenceRepo:
        def __init__(self, session):
            pass

        def delete_for_task(self, task_id):
            deleted.append(("evidence", task_id))

        def save_command_result(self, task_id, item):
            evidence_saved.append(("command", task_id, item.command))

        def save_diff_coverage_evidence(self, item):
            evidence_saved.append(("coverage", item.task_id))

        def save_diff_evidence(self, item):
            evidence_saved.append(("diff", item.task_id))

        def save_test_evidence(self, item, task_id):
            evidence_saved.append(("test", task_id, item.acceptance_criterion_id))

    class FakeStateRepo:
        def __init__(self, session):
            pass

        def record_phase(self, phase):
            phases.append(phase)

    class FakeVerifier:
        last_outcome = SimpleNamespace(
            mode="deterministic",
            compiler_active=True,
            diff_empty=False,
            coverage_measured=True,
            coverage_skipped_reason=None,
        )

        def __init__(self, root, router=None):
            pass

        async def verify_task(self, task, reqs_arg):
            if task.id == "TASK-1":
                return [
                    _gap(
                        "GAP-AC",
                        gap_type="acceptance_criteria_unproven",
                        ac_id="AC-1",
                    )
                ], [
                    CommandResult(command="pytest", exit_code=0, stdout_path="out", stderr_path="err", summary="ok"),
                    DiffCoverageEvidence(task_id=task.id, measured=True, changed_lines=1, covered_lines=1),
                    DiffEvidence(task_id=task.id, changed_files=["src/app.py"], added_files=[], deleted_files=[], diff_summary="diff"),
                ]
            return [], [
                TestEvidence(
                    requirement_id="REQ-1",
                    acceptance_criterion_id="AC-1",
                    command="pytest",
                    status="passed",
                    evidence_summary="passed",
                )
            ]

    monkeypatch.setattr(verify_cmd, "initialize_project", lambda root, quiet=True: None)
    monkeypatch.setattr(verify_cmd, "get_db", lambda root: _FakeDb())
    monkeypatch.setattr(verify_cmd, "TaskRepository", FakeTaskRepo)
    monkeypatch.setattr(verify_cmd, "RequirementRepository", FakeReqRepo)
    monkeypatch.setattr(verify_cmd, "GapRepository", FakeGapRepo)
    monkeypatch.setattr(verify_cmd, "EvidenceRepository", FakeEvidenceRepo)
    monkeypatch.setattr(verify_cmd, "StateRepository", FakeStateRepo)
    monkeypatch.setattr(verify_cmd, "Verifier", FakeVerifier)
    monkeypatch.setattr(verify_cmd, "load_config", lambda root: (_ for _ in ()).throw(RuntimeError("no model")))
    monkeypatch.setattr(
        verify_cmd,
        "CodeReviewGraphAdapter",
        lambda root: SimpleNamespace(get_context=lambda paths: SimpleNamespace(available=True, model_dump=lambda: {"paths": paths})),
    )
    monkeypatch.setattr(verify_cmd, "TraceLogger", lambda root: SimpleNamespace(log_event=lambda *a, **k: events.append((a, k))))

    verify_cmd.verify(task_id=None, sandbox="local", json_format=True, project_root=tmp_path)
    out = capsys.readouterr().out
    payload = json.loads(out)

    assert payload["ok"] is True
    assert payload["blocked_tasks"] == 0
    assert ("TASK-1", "verified") in saved_tasks
    assert ("TASK-2", "verified") in saved_tasks
    assert ("gap", "TASK-1") in deleted
    assert ("command", "TASK-1", "pytest") in evidence_saved
    assert ("coverage", "TASK-1") in evidence_saved
    assert ("diff", "TASK-1") in evidence_saved
    assert ("test", "TASK-2", "AC-1") in evidence_saved
    assert phases[-1] == "TASK_VERIFIED"
    assert any(call[0][0] == "task_reconciled" for call in events)


def test_report_async_helpers_and_callback(monkeypatch, tmp_path, capsys):
    monkeypatch.delenv("GITHUB_TOKEN", raising=False)
    monkeypatch.delenv("GITHUB_REPOSITORY", raising=False)
    asyncio.run(report_cmd.run_github_report(SimpleNamespace(), tmp_path))
    assert "GITHUB_TOKEN" in capsys.readouterr().out

    monkeypatch.setenv("GITHUB_TOKEN", "token")
    monkeypatch.setenv("GITHUB_REPOSITORY", "owner/repo")
    monkeypatch.setenv("GITHUB_PR_NUMBER", "not-int")
    asyncio.run(report_cmd.run_github_pr_comment(SimpleNamespace()))
    assert "must be an integer" in capsys.readouterr().out

    called = []

    class FakeIntegration:
        def __init__(self, token, repo, sha):
            called.append((token, repo, sha))

        async def report_verification(self, graph):
            called.append(("reported", graph))

    monkeypatch.setattr(report_cmd.subprocess, "check_output", lambda argv, cwd: b"abcdef123456\n")
    monkeypatch.setattr(report_cmd, "GitHubIntegration", FakeIntegration)
    asyncio.run(report_cmd.run_github_report("graph", tmp_path))
    assert called == [("token", "owner/repo", "abcdef123456"), ("reported", "graph")]

    class FakeGraph:
        def blocking_gaps(self):
            return [_gap()]

    class FakeGraphRepo:
        def __init__(self, session):
            pass

        def load_graph(self):
            return FakeGraph()

    events = []
    monkeypatch.setattr(report_cmd, "initialize_project", lambda root, quiet=True: None)
    monkeypatch.setattr(report_cmd, "get_db", lambda root: _FakeDb())
    monkeypatch.setattr(report_cmd, "ArtifactGraphRepository", FakeGraphRepo)
    monkeypatch.setattr(report_cmd, "live_review_summary", lambda root: {"cards": {}, "blocking_cards": []})
    monkeypatch.setattr(report_cmd.ReportBuilder, "build_json", lambda graph, live_review=None: '{"ok": true}')
    monkeypatch.setattr(report_cmd.ReportBuilder, "build_markdown", lambda graph, live_review=None: "# ok")
    monkeypatch.setattr(report_cmd, "TraceLogger", lambda root: SimpleNamespace(log_event=lambda *a, **k: events.append((a, k))))

    report_cmd.report(SimpleNamespace(invoked_subcommand="child"), project_root=tmp_path)
    assert events == []
    with pytest.raises(typer.Exit) as exc:
        report_cmd.report(
            SimpleNamespace(invoked_subcommand=None),
            planning_only=False,
            json_format=True,
            github=False,
            github_pr_comment=False,
            gitlab_pr_comment=False,
            fail_on_blocking=True,
            project_root=tmp_path,
        )
    assert exc.value.exit_code == 1
    assert '{"ok": true}' in capsys.readouterr().out
    assert events


def test_report_pr_comment_and_gitlab_branches(monkeypatch, tmp_path, capsys):
    monkeypatch.delenv("GITLAB_TOKEN", raising=False)
    asyncio.run(report_cmd.run_gitlab_mr_comment(SimpleNamespace()))
    assert "GITLAB_TOKEN" in capsys.readouterr().out

    monkeypatch.setenv("GITLAB_TOKEN", "token")
    monkeypatch.setenv("GITLAB_PROJECT_ID", "project")
    monkeypatch.setenv("GITLAB_MR_IID", "bad")
    asyncio.run(report_cmd.run_gitlab_mr_comment(SimpleNamespace()))
    assert "must be an integer" in capsys.readouterr().out

    posted = []

    class FakeGitHubCommenter:
        def __init__(self, token, repo, pull_number):
            posted.append(("github", token, repo, pull_number))

        async def post_comment(self, body):
            posted.append(("github-body", body))

    class FakeGitLabCommenter:
        def __init__(self, token, project_id, mr_iid, base_url=""):
            posted.append(("gitlab", token, project_id, mr_iid, base_url))

        async def post_comment(self, body):
            posted.append(("gitlab-body", body))

    monkeypatch.setenv("GITHUB_TOKEN", "token")
    monkeypatch.setenv("GITHUB_REPOSITORY", "owner/repo")
    monkeypatch.setenv("GITHUB_PR_NUMBER", "7")
    monkeypatch.setenv("GITLAB_MR_IID", "8")
    monkeypatch.setenv("GITLAB_API_URL", "https://gitlab.example/api/v4")
    monkeypatch.setattr(report_cmd, "GitHubPRCommenter", FakeGitHubCommenter)
    monkeypatch.setattr(report_cmd, "GitLabMRCommenter", FakeGitLabCommenter)
    monkeypatch.setattr(report_cmd, "build_pr_comment_body", lambda graph, live_review=None: f"body:{live_review}")
    asyncio.run(report_cmd.run_github_pr_comment("graph", live_review={"x": 1}))
    asyncio.run(report_cmd.run_gitlab_mr_comment("graph", live_review={"x": 2}))
    assert ("github", "token", "owner/repo", 7) in posted
    assert ("gitlab", "token", "project", 8, "https://gitlab.example/api/v4") in posted

    class FakeGraphRepo:
        def __init__(self, session):
            pass

        def load_graph(self):
            return SimpleNamespace(blocking_gaps=lambda: [])

    calls = []
    monkeypatch.setattr(report_cmd, "initialize_project", lambda root, quiet=True: None)
    monkeypatch.setattr(report_cmd, "get_db", lambda root: _FakeDb())
    monkeypatch.setattr(report_cmd, "ArtifactGraphRepository", FakeGraphRepo)
    monkeypatch.setattr(report_cmd, "live_review_summary", lambda root: {"cards": {}})
    monkeypatch.setattr(report_cmd, "TraceLogger", lambda root: SimpleNamespace(log_event=lambda *a, **k: None))
    monkeypatch.setattr(report_cmd, "run_github_report", lambda graph, root: calls.append(("check", graph, root)) or _async_none())
    monkeypatch.setattr(report_cmd, "run_github_pr_comment", lambda graph, live_review=None: calls.append(("gh-comment", live_review)) or _async_none())
    monkeypatch.setattr(report_cmd, "run_gitlab_mr_comment", lambda graph, live_review=None: calls.append(("gl-comment", live_review)) or _async_none())

    for flag in ("github", "github_pr_comment", "gitlab_pr_comment"):
        kwargs = {
            "planning_only": False,
            "json_format": False,
            "github": False,
            "github_pr_comment": False,
            "gitlab_pr_comment": False,
            "fail_on_blocking": False,
            "project_root": tmp_path,
        }
        kwargs[flag] = True
        report_cmd.report(SimpleNamespace(invoked_subcommand=None), **kwargs)
    assert [call[0] for call in calls] == ["check", "gh-comment", "gl-comment"]


async def _async_none():
    return None


def test_agents_add_help_doctor_and_run_branches(monkeypatch, tmp_path, capsys):
    saved = []
    monkeypatch.setattr(agents_cmd, "_project_root", lambda root=None: tmp_path)
    monkeypatch.setattr(agents_cmd, "load_agent_profiles", lambda root: {"default": {}, "prod": {}})
    monkeypatch.setattr(agents_cmd, "is_reserved_agent_name", lambda name: name == "codex")
    monkeypatch.setattr(agents_cmd, "_load_raw_config", lambda root: {})
    monkeypatch.setattr(agents_cmd, "_save_raw_config", lambda root, config: saved.append(config))

    for kwargs in [
        {"name": "x", "command": "tool", "input_mode": "bad"},
        {"name": " ", "command": "tool"},
        {"name": "x", "command": " "},
        {"name": "codex", "command": "tool"},
        {"name": "x", "command": "tool", "default_profile": "missing"},
    ]:
        with pytest.raises(typer.Exit):
            agents_cmd.add_agent(**kwargs)

    agents_cmd.add_agent(
        "My Agent",
        command="tool",
        arg=["--flag"],
        input_mode="argument",
        prompt_arg="--prompt",
        timeout_seconds=12,
        display_name="Mine",
        kind="custom",
        supports_mcp=True,
        supports_diff_review=True,
        default_profile="prod",
        help_arg=["--help"],
        project_root=tmp_path,
    )
    entry = saved[0]["integrations"]["cli_agents"]["agents"]["my agent"]
    assert entry["command"] == "tool"
    assert entry["args"] == ["--flag"]

    monkeypatch.setattr(agents_cmd, "_which", lambda command: None)
    monkeypatch.setattr(agents_cmd, "load_cli_agent_specs", lambda root: {})
    with pytest.raises(typer.Exit):
        agents_cmd.agent_help("missing", project_root=tmp_path)
    assert "Unknown agent" in capsys.readouterr().out

    spec = SimpleNamespace(executable="tool", help_command=["tool", "--help"])
    monkeypatch.setattr(agents_cmd, "load_cli_agent_specs", lambda root: {"mine": spec})
    with pytest.raises(typer.Exit) as missing_tool:
        agents_cmd.agent_help("mine", project_root=tmp_path)
    assert missing_tool.value.exit_code == 1

    monkeypatch.setattr(agents_cmd, "_which", lambda command: "/bin/tool")
    monkeypatch.setattr(
        agents_cmd.subprocess,
        "run",
        lambda *a, **k: SimpleNamespace(returncode=3, stdout="help out\n", stderr="warn\n"),
    )
    with pytest.raises(typer.Exit) as help_exit:
        agents_cmd.agent_help("mine", project_root=tmp_path)
    assert help_exit.value.exit_code == 3
    assert "help out" in capsys.readouterr().out

    captured = []
    monkeypatch.setattr(agents_cmd.run_command, "run", lambda *a, **k: captured.append((a, k)))
    agents_cmd.run_agent("TASK-1", agent="mine", profile="prod", stream=True, project_root=tmp_path)
    assert captured[0][1] == {"executor": "mine", "profile": "prod", "stream": True, "project_root": tmp_path}


def test_setup_helper_branches(monkeypatch, tmp_path, capsys):
    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    config_path = dev_dir / "config.yaml"
    config_path.write_text("models:\n  provider: openrouter\n  roles:\n    spec_writer:\n      model: old\n", encoding="utf-8")

    setup_cmd._set_model_provider(tmp_path, "ollama-local")
    setup_cmd._set_model_roles(tmp_path, model="shared-model", role_models={"critic_a": "critic-model"})
    text = config_path.read_text(encoding="utf-8")
    assert "provider: ollama" in text
    assert "shared-model" in text
    assert "critic-model" in text

    secret_path = setup_cmd._write_local_secret(tmp_path, "OPENROUTER_API_KEY", "sk-test")
    assert "OPENROUTER_API_KEY=sk-test" in secret_path.read_text(encoding="utf-8")
    with pytest.raises(ValueError):
        setup_cmd._write_local_secret(tmp_path, "OPENROUTER_API_KEY", "bad\nkey")

    monkeypatch.setattr(setup_cmd, "load_local_secrets", lambda root: {})
    monkeypatch.setattr(setup_cmd, "load_config", lambda root: SimpleNamespace(models=SimpleNamespace(provider="ollama")))
    setup_cmd._configure_api_key(tmp_path, api_key=None, skip_api_key=False)
    assert "Ollama uses a local server" in capsys.readouterr().out

    monkeypatch.setattr(setup_cmd, "load_config", lambda root: SimpleNamespace(models=SimpleNamespace(provider="openrouter")))
    monkeypatch.setattr(setup_cmd.sys.stdin, "isatty", lambda: False)
    setup_cmd._configure_api_key(tmp_path, api_key=None, skip_api_key=False)
    setup_cmd._configure_api_key(tmp_path, api_key=None, skip_api_key=True)
    out = capsys.readouterr().out
    assert "Run dev setup --api-key" in out
    assert "Skipped OPENROUTER_API_KEY setup" in out

    monkeypatch.setenv("OPENROUTER_API_KEY", "env-key")
    setup_cmd._configure_api_key(tmp_path, api_key="ignored", skip_api_key=False)
    assert "already set in the environment" in capsys.readouterr().out

    monkeypatch.delenv("OPENROUTER_API_KEY", raising=False)
    monkeypatch.setattr(setup_cmd, "load_local_secrets", lambda root: {"OPENROUTER_API_KEY": "local-key"})
    setup_cmd._configure_api_key(tmp_path, api_key="ignored", skip_api_key=False)
    assert "already set in .devcouncil/secrets.env" in capsys.readouterr().out

    writes = []
    monkeypatch.setattr(setup_cmd, "load_local_secrets", lambda root: {})
    monkeypatch.setattr(setup_cmd, "_write_local_secret", lambda root, env, value: writes.append((env, value)) or (root / "secrets.env"))
    setup_cmd._configure_vertexai_settings(tmp_path, "vertexai", "proj", "us-central1")
    setup_cmd._configure_api_key(tmp_path, api_key="sk-new", skip_api_key=False)
    assert ("VERTEXAI_PROJECT", "proj") in writes
    assert ("VERTEXAI_LOCATION", "us-central1") in writes
    assert ("OPENROUTER_API_KEY", "sk-new") in writes


def test_go_helpers_and_git_branches(monkeypatch, tmp_path):
    assert go_cmd._normalize_executor(" Native_Preview ") == "native-preview"
    assert go_cmd._unique_task_ids(["A", "B", "A", "C"]) == ["A", "B", "C"]
    assert go_cmd._command_label(SimpleNamespace(info_name=None)) == "dev e2e"
    assert go_cmd._command_label(SimpleNamespace(info_name="go")) == "dev go"
    assert go_cmd._write_report_file(tmp_path, Path("reports/latest.json"), "{}") == tmp_path / "reports/latest.json"
    assert (tmp_path / "reports" / "latest.json").read_text(encoding="utf-8") == "{}"

    monkeypatch.setattr(go_cmd.subprocess, "run", lambda *a, **k: (_ for _ in ()).throw(OSError("no git")))
    assert go_cmd._is_git_repo(tmp_path) is False
    assert go_cmd._current_head(tmp_path) is None
    assert go_cmd._commit_task_changes(tmp_path, "TASK-1", "blocked") is False

    responses = iter(
        [
            SimpleNamespace(returncode=0, stdout="true\n", stderr=""),
            SimpleNamespace(returncode=0, stdout="abc123\n", stderr=""),
            SimpleNamespace(returncode=0, stdout=" M file.py\n", stderr=""),
            SimpleNamespace(returncode=0, stdout="", stderr=""),
            SimpleNamespace(returncode=0, stdout="", stderr=""),
        ]
    )
    calls = []

    def fake_run(argv, cwd=None, capture_output=False, text=False, check=False):
        calls.append(argv)
        return next(responses)

    monkeypatch.setattr(go_cmd.subprocess, "run", fake_run)
    assert go_cmd._is_git_repo(tmp_path) is True
    assert go_cmd._current_head(tmp_path) == "abc123"
    assert go_cmd._commit_task_changes(tmp_path, "TASK-1", "verified") is True
    assert ["git", "add", "-A"] in calls


def test_go_load_tasks_by_id_and_report_render(monkeypatch, tmp_path):
    tasks = {"TASK-1": _task("TASK-1"), "TASK-2": _task("TASK-2")}

    class FakeTaskRepo:
        def __init__(self, session):
            pass

        def get_by_id(self, task_id):
            return tasks.get(task_id)

        def get_all(self):
            return list(tasks.values())

    class FakeGraphRepo:
        def __init__(self, session):
            pass

        def load_graph(self):
            return "graph"

    phases = []

    class FakeStateRepo:
        def __init__(self, session):
            pass

        def record_phase(self, phase):
            phases.append(phase)

    monkeypatch.setattr(go_cmd, "get_db", lambda root: _FakeDb())
    monkeypatch.setattr(go_cmd, "TaskRepository", FakeTaskRepo)
    monkeypatch.setattr(go_cmd, "ArtifactGraphRepository", FakeGraphRepo)
    monkeypatch.setattr(go_cmd, "StateRepository", FakeStateRepo)
    monkeypatch.setattr(go_cmd, "live_review_summary", lambda root: {"cards": {}})
    monkeypatch.setattr(go_cmd.ReportBuilder, "build_json", lambda graph, live_review=None: json.dumps({"graph": graph}))
    monkeypatch.setattr(go_cmd.ReportBuilder, "build_markdown", lambda graph, live_review=None: "# report")

    loaded, missing = go_cmd._load_tasks_by_id(tmp_path, ["TASK-1", "MISSING", "TASK-2"])
    assert [task.id for task in loaded] == ["TASK-1", "TASK-2"]
    assert missing == ["MISSING"]
    assert [task.id for task in go_cmd._load_tasks(tmp_path)] == ["TASK-1", "TASK-2"]
    assert go_cmd._task_status(tmp_path, "TASK-1") == "planned"
    assert go_cmd._task_status(tmp_path, "NOPE") == "missing"
    assert go_cmd._render_final_report(tmp_path, json_report=True) == '{"graph": "graph"}'
    assert go_cmd._render_final_report(tmp_path, json_report=False) == "# report"
    go_cmd._record_project_done(tmp_path)
    go_cmd._record_project_blocked(tmp_path)
    assert phases == ["PROJECT_DONE", "TASK_BLOCKED"]


def _call_go(tmp_path, **overrides):
    kwargs = {
        "ctx": SimpleNamespace(info_name="go"),
        "goal": "ship feature",
        "executor": "claude",
        "dry_run": True,
        "quick": True,
        "force": False,
        "continue_on_blocked": False,
        "json_report": False,
        "report_file": None,
        "agent": False,
        "profile": None,
        "stream": False,
        "project_root": tmp_path,
    }
    kwargs.update(overrides)
    return go_cmd.go(**kwargs)


def test_go_command_error_and_planning_branches(monkeypatch, tmp_path):
    monkeypatch.setattr(go_cmd, "initialize_project", lambda root, quiet=True: None)
    monkeypatch.setattr(go_cmd, "set_log_dir", lambda root: None)
    monkeypatch.setattr(go_cmd, "resolve_goal_intent", lambda goal, root: (f"expanded {goal}", "intent note"))
    monkeypatch.setattr(go_cmd, "resolve_automated_executor", lambda root, executor: "manual")
    with pytest.raises(typer.Exit) as manual_exit:
        _call_go(tmp_path, executor=None)
    assert manual_exit.value.exit_code == 2

    monkeypatch.setattr(go_cmd, "resolve_automated_executor", lambda root, executor: "unknown")
    monkeypatch.setattr(go_cmd, "_custom_cli_agents", lambda root: set())
    with pytest.raises(typer.Exit) as unsupported:
        _call_go(tmp_path)
    assert unsupported.value.exit_code == 2

    monkeypatch.setattr(go_cmd, "resolve_automated_executor", lambda root, executor: "claude")
    monkeypatch.setattr(go_cmd, "_custom_cli_agents", lambda root: set())

    async def raising_plan(*args, **kwargs):
        raise ProviderRequestError("no credits", status_code=402)

    monkeypatch.setattr(go_cmd.plan_command, "run_plan_flow", raising_plan)
    monkeypatch.setattr(go_cmd.plan_command, "print_planning_error", lambda exc: None)
    with pytest.raises(typer.Exit) as plan_error:
        _call_go(tmp_path)
    assert plan_error.value.exit_code == 1

    async def empty_plan(*args, **kwargs):
        return []

    monkeypatch.setattr(go_cmd.plan_command, "run_plan_flow", empty_plan)
    with pytest.raises(typer.Exit) as no_tasks:
        _call_go(tmp_path, force=False)
    assert no_tasks.value.exit_code == 1

    approved_tasks = [_task("TASK-1")]
    monkeypatch.setattr(go_cmd.plan_command, "approve", lambda run_id=None, force=False, project_root=None: None)
    monkeypatch.setattr(go_cmd, "_load_tasks", lambda root: approved_tasks)
    monkeypatch.setattr(go_cmd, "_max_repair_attempts", lambda root: 0)
    monkeypatch.setattr(go_cmd, "topological_order", lambda tasks: tasks)
    monkeypatch.setattr(go_cmd, "_execute_task_with_repair", lambda *a, **k: ("verified", 0))
    monkeypatch.setattr(go_cmd, "_commit_task_changes", lambda root, task_id, status: True)
    monkeypatch.setattr(go_cmd, "_is_git_repo", lambda root: False)
    monkeypatch.setattr(go_cmd, "_record_project_done", lambda root: None)
    monkeypatch.setattr(go_cmd, "_record_project_blocked", lambda root: None)
    monkeypatch.setattr(go_cmd.report_command, "report", lambda *a, **k: None)
    monkeypatch.setattr(go_cmd, "_render_final_report", lambda root, json_report: '{"ok": true}')
    written = []
    monkeypatch.setattr(go_cmd, "_write_report_file", lambda root, report_file, content: written.append((report_file, content)) or (root / report_file))
    _call_go(tmp_path, force=True, agent=True)
    assert written == [(go_cmd.AGENT_REPORT_FILE, '{"ok": true}')]


def test_go_command_execution_dependency_reconcile_and_failure(monkeypatch, tmp_path):
    tasks = [
        _task("TASK-DONE", status="verified"),
        _task("TASK-RUN"),
        _task("TASK-SKIP"),
    ]
    tasks[2].depends_on = ["MISSING-UPSTREAM"]
    statuses = {"TASK-DONE": "verified", "TASK-RUN": "planned", "TASK-SKIP": "planned"}
    phases = []
    reports = []

    monkeypatch.setattr(go_cmd, "initialize_project", lambda root, quiet=True: None)
    monkeypatch.setattr(go_cmd, "set_log_dir", lambda root: None)
    monkeypatch.setattr(go_cmd, "resolve_goal_intent", lambda goal, root: (goal, None))
    monkeypatch.setattr(go_cmd, "resolve_automated_executor", lambda root, executor: "claude")
    monkeypatch.setattr(go_cmd, "_custom_cli_agents", lambda root: set())

    async def planned(*args, **kwargs):
        return [task.id for task in tasks]

    monkeypatch.setattr(go_cmd.plan_command, "run_plan_flow", planned)
    monkeypatch.setattr(go_cmd, "_load_tasks_by_id", lambda root, ids: (tasks, []))
    monkeypatch.setattr(go_cmd, "_max_repair_attempts", lambda root: 1)
    monkeypatch.setattr(go_cmd, "_build_repair_service", lambda root: "repair-service")
    monkeypatch.setattr(go_cmd, "load_config", lambda root: "config")
    monkeypatch.setattr(go_cmd, "topological_order", lambda task_list: task_list)
    monkeypatch.setattr(go_cmd, "_task_status", lambda root, task_id: statuses.get(task_id, "missing"))

    def fake_execute(root, task, **kwargs):
        assert kwargs["repair_service"] == "repair-service"
        assert kwargs["config"] == "config"
        statuses[task.id] = "blocked"
        return "blocked", 1

    monkeypatch.setattr(go_cmd, "_execute_task_with_repair", fake_execute)
    monkeypatch.setattr(go_cmd, "_commit_task_changes", lambda root, task_id, status: True)
    monkeypatch.setattr(go_cmd, "_is_git_repo", lambda root: True)
    monkeypatch.setattr(go_cmd.verify_command, "verify", lambda **kwargs: (_ for _ in ()).throw(typer.Exit(code=1)))
    monkeypatch.setattr(
        go_cmd,
        "_load_tasks",
        lambda root: [
            _task("TASK-DONE", status="verified"),
            _task("TASK-RUN", status="blocked"),
            _task("TASK-SKIP", status="planned"),
        ],
    )
    monkeypatch.setattr(go_cmd, "_record_project_done", lambda root: phases.append("done"))
    monkeypatch.setattr(go_cmd, "_record_project_blocked", lambda root: phases.append("blocked"))
    monkeypatch.setattr(go_cmd.report_command, "report", lambda *a, **k: reports.append(k))

    with pytest.raises(typer.Exit) as exc:
        _call_go(tmp_path, continue_on_blocked=True, json_report=True, report_file=Path("out.json"))
    assert exc.value.exit_code == 1
    assert phases == ["blocked"]
    assert reports and reports[0]["json_format"] is True

    monkeypatch.setattr(
        go_cmd,
        "_load_tasks",
        lambda root: [
            _task("TASK-DONE", status="verified"),
            _task("TASK-RUN", status="verified"),
            _task("TASK-SKIP", status="verified"),
        ],
    )
    phases.clear()
    _call_go(tmp_path, continue_on_blocked=True)
    assert phases == ["done"]


def _write_plan_run(root: Path, run_id: str = "run-1", *, include_spec: bool = True, include_critiques: bool = False) -> Path:
    run_dir = root / ".devcouncil" / "runs" / run_id
    run_dir.mkdir(parents=True)
    requirement = {
        "id": "REQ-1",
        "title": "Requirement",
        "description": "Do the thing",
        "priority": "high",
        "source": "user",
        "acceptance_criteria": [
            {"id": "AC-1", "description": "Verified", "verification_method": "unit_test", "required": True}
        ],
    }
    decision = {
        "accepted_finding_ids": ["F-1"],
        "rejected_finding_ids": [{"id": "F-2", "reason": "duplicate"}],
        "final_requirements": [requirement],
        "final_tasks": [_task("TASK-1").model_dump(mode="json")],
    }
    (run_dir / "decision.json").write_text(json.dumps(decision), encoding="utf-8")
    if include_spec:
        (run_dir / "requirements.json").write_text(
            json.dumps({"requirements": [requirement], "assumptions": [], "blocking_questions": []}),
            encoding="utf-8",
        )
    if include_critiques:
        findings = [
            {
                "id": "F-1",
                "source_agent": "critic_a",
                "target_plan_id": "PLAN-A",
                "severity": "high",
                "finding_type": "missing_test",
                "claim": "needs tests",
                "falsifiable_check": "pytest",
                "status": "open",
            },
            {
                "id": "F-2",
                "source_agent": "critic_b",
                "target_plan_id": "PLAN-B",
                "severity": "medium",
                "finding_type": "architecture_risk",
                "claim": "too broad",
                "falsifiable_check": "review",
                "status": "open",
            },
        ]
        (run_dir / "critique_a.json").write_text(json.dumps({"findings": findings}), encoding="utf-8")
    return run_dir


def test_plan_approve_missing_state_and_run_errors(monkeypatch, tmp_path):
    monkeypatch.setattr(plan_cmd, "get_db", lambda root: None)
    no_db = runner.invoke(app, ["approve", "--project-root", str(tmp_path)])
    assert no_db.exit_code == 1
    assert "state is unavailable" in no_db.output

    monkeypatch.setattr(plan_cmd, "get_db", lambda root: _FakeDb())
    missing = runner.invoke(app, ["approve", "--project-root", str(tmp_path)])
    assert missing.exit_code == 1
    assert "No planning run with a decision" in missing.output

    assert plan_cmd._latest_run_with_decision(tmp_path, "missing") is None


def test_plan_approve_gate_failure_force_and_transition(monkeypatch, tmp_path):
    older = _write_plan_run(tmp_path, "older", include_spec=False)
    newer = _write_plan_run(tmp_path, "newer", include_critiques=True)
    os.utime(older / "decision.json", (1, 1))
    os.utime(newer / "decision.json", (2, 2))
    assert plan_cmd._latest_run_with_decision(tmp_path, None) == newer

    saved = {"deleted": 0, "replaced": [], "transitioned": []}

    class FakeGapRepo:
        def __init__(self, session):
            pass

        def delete_plan_gaps(self):
            saved["deleted"] += 1

    class FakePlanningRepo:
        def __init__(self, session):
            pass

        def replace_active_plan(self, requirements, assumptions, tasks, findings):
            saved["replaced"].append(
                {
                    "requirements": [req.id for req in requirements],
                    "assumptions": assumptions,
                    "tasks": [task.id for task in tasks],
                    "findings": [finding.status for finding in findings],
                }
            )

    class FailingPolicy:
        def check_plan_approval(self, *args, **kwargs):
            return SimpleNamespace(passed=False, gaps=[_gap(gap_id="GAP-PLAN")])

    class PassingPolicy:
        def check_plan_approval(self, *args, **kwargs):
            return SimpleNamespace(passed=True, gaps=[])

    class FakeOrchestrator:
        def __init__(self, root):
            self.root = root

        async def transition_to(self, phase):
            saved["transitioned"].append(phase)

    monkeypatch.setattr(plan_cmd, "get_db", lambda root: _FakeDb())
    monkeypatch.setattr(plan_cmd, "GapRepository", FakeGapRepo)
    monkeypatch.setattr(plan_cmd, "PlanningStateRepository", FakePlanningRepo)
    monkeypatch.setattr(plan_cmd, "GatePolicy", FailingPolicy)
    monkeypatch.setattr(plan_cmd, "Orchestrator", FakeOrchestrator)

    failed = runner.invoke(app, ["approve", "--project-root", str(tmp_path)])
    assert failed.exit_code == 1
    assert "Plan still fails approval gates" in failed.output
    assert saved["replaced"] == []

    forced = runner.invoke(app, ["approve", "--force", "--project-root", str(tmp_path)])
    assert forced.exit_code == 0
    assert "Plan from run newer approved (1 tasks)" in forced.output
    assert saved["deleted"] == 1
    assert saved["replaced"] == [
        {"requirements": ["REQ-1"], "assumptions": [], "tasks": ["TASK-1"], "findings": ["converted", "rejected"]}
    ]
    assert saved["transitioned"] == [plan_cmd.ProjectPhase.PLAN_APPROVED]

    class RaisingOrchestrator:
        def __init__(self, root):
            pass

        async def transition_to(self, phase):
            raise ValueError("wrong phase")

    monkeypatch.setattr(plan_cmd, "GatePolicy", PassingPolicy)
    monkeypatch.setattr(plan_cmd, "Orchestrator", RaisingOrchestrator)
    transition_error = runner.invoke(app, ["approve", "--run-id", "older", "--project-root", str(tmp_path)])
    assert transition_error.exit_code == 1
    assert "Cannot approve from the current project phase" in transition_error.output


def test_watch_cli_helpers_and_commands(monkeypatch, tmp_path, capsys):
    assert watch_cmd._filtered_signals(tmp_path, None) == []
    signals = [
        SimpleNamespace(client="claude", transcript_path="rel.jsonl", task_id="TASK-1", path="signal.json", review_command="dev watch review", model_dump=lambda: {"client": "claude"}),
        SimpleNamespace(client="codex", transcript_path=None, task_id=None, path="signal2.json", review_command=None, model_dump=lambda: {"client": "codex"}),
    ]
    monkeypatch.setattr(watch_cmd, "load_signals", lambda root: signals)
    assert len(watch_cmd._filtered_signals(tmp_path, "claude")) == 1
    assert watch_cmd._resolve_signal_transcript(tmp_path, signals[0]) == (tmp_path / "rel.jsonl").resolve()
    assert watch_cmd._resolve_signal_transcript(tmp_path, signals[1]) is None

    transcript = tmp_path / "session.jsonl"
    transcript.write_text("{}", encoding="utf-8")
    monkeypatch.setattr(
        watch_cmd,
        "discover_sessions",
        lambda root, client: [
            SimpleNamespace(id="s1", transcript_path=str(transcript)),
            SimpleNamespace(id="s2", transcript_path=str(tmp_path / "other.jsonl")),
        ],
    )
    assert watch_cmd._resolve_transcript(tmp_path, "claude", transcript=Path("session.jsonl")) == transcript.resolve()
    assert watch_cmd._resolve_transcript(tmp_path, "claude", latest=True) == transcript.resolve()
    assert watch_cmd._resolve_transcript(tmp_path, "claude", session="s2") == (tmp_path / "other.jsonl").resolve()
    assert watch_cmd._resolve_transcript(tmp_path, "claude", session="missing") is None

    card = SimpleNamespace(
        id="CARD-1",
        verdict="Critical Issues",
        status="open",
        task_id=None,
        summary="summary",
        concerns=["concern"],
        alternatives=["alt"],
        evidence_requests=["evidence"],
        message_for_agent="fix",
        client="claude",
        model_copy=lambda update: SimpleNamespace(
            id="CARD-1",
            verdict="Critical Issues",
            status=update["status"],
            task_id=None,
            summary="summary",
            concerns=[],
            alternatives=[],
            evidence_requests=[],
            message_for_agent="fix",
            client="claude",
        ),
    )
    existing = tmp_path / ".devcouncil" / "live" / "cards" / "CARD-1.json"
    existing.parent.mkdir(parents=True)
    existing.write_text("{}", encoding="utf-8")
    monkeypatch.setattr(watch_cmd, "load_cards", lambda root: [SimpleNamespace(id="CARD-1", status="ignored")])
    monkeypatch.setattr(watch_cmd, "save_card", lambda root, card_arg: tmp_path / f"{card_arg.status}.json")
    assert watch_cmd._save_card_once(tmp_path, card, persist=False, force=False) == (None, False)
    assert watch_cmd._save_card_once(tmp_path, card, persist=True, force=False) == (existing, True)
    assert watch_cmd._save_card_once(tmp_path, card, persist=True, force=True) == (tmp_path / "ignored.json", False)

    logged = []
    monkeypatch.setattr(watch_cmd, "TraceLogger", lambda root: SimpleNamespace(log_event=lambda *a, **k: logged.append((a, k))))
    watch_cmd._log_card_reviewed(tmp_path, card, existing, duplicate=True, source="test")
    watch_cmd._log_card_resolved(tmp_path, card)
    watch_cmd._log_signal_processed(tmp_path, signals[0], tmp_path / "processed", card)
    assert [entry[0][0] for entry in logged] == [
        "live_review_card_reused",
        "live_review_card_status_updated",
        "live_review_signal_processed",
    ]

    watch_cmd._print_card(card)
    assert "Critical Issues" in capsys.readouterr().out


def test_watch_commands_json_error_paths(monkeypatch, tmp_path):
    monkeypatch.setattr(watch_cmd, "load_cards", lambda root: [])
    bad_limit = runner.invoke(app, ["watch", "cards", "--limit", "0", "--json", "--project-root", str(tmp_path)])
    assert bad_limit.exit_code == 2
    assert json.loads(bad_limit.output)["error"] == "--limit must be greater than 0."

    monkeypatch.setattr(watch_cmd, "filter_cards", lambda *a, **k: ([], "bad filter", "status"))
    bad_filter = runner.invoke(app, ["watch", "cards", "--status", "bad", "--json", "--project-root", str(tmp_path)])
    assert bad_filter.exit_code == 2
    assert json.loads(bad_filter.output)["error"] == "bad filter"

    missing_review = runner.invoke(app, ["watch", "review", "--json", "--project-root", str(tmp_path)])
    assert missing_review.exit_code == 2
    assert "No transcript selected" in missing_review.output

    transcript = tmp_path / "empty.jsonl"
    transcript.write_text("", encoding="utf-8")
    monkeypatch.setattr(watch_cmd, "latest_assistant_turn", lambda path, client: None)
    no_turn = runner.invoke(
        app,
        ["watch", "review", "--transcript", str(transcript), "--json", "--project-root", str(tmp_path)],
    )
    assert no_turn.exit_code == 1
    assert "No assistant turn found" in no_turn.output


def test_watch_commands_success_paths(monkeypatch, tmp_path):
    transcript = tmp_path / "transcript.jsonl"
    transcript.write_text("{}\n", encoding="utf-8")
    sessions = [SimpleNamespace(client="claude", id="s1", turns=2, transcript_path=str(transcript), model_dump=lambda: {"id": "s1"})]
    monkeypatch.setattr(watch_cmd, "discover_sessions", lambda root, client: sessions)
    as_json = runner.invoke(app, ["watch", "sessions", "--client", "claude", "--json", "--project-root", str(tmp_path)])
    plain = runner.invoke(app, ["watch", "sessions", "--client", "claude", "--project-root", str(tmp_path)])
    assert as_json.exit_code == 0 and json.loads(as_json.output)["sessions"] == [{"id": "s1"}]
    assert "DevCouncil Watch Sessions" in plain.output

    payload = {
        "active_task_id": "TASK-1",
        "scope_task_id": "TASK-1",
        "pending_signals": 1,
        "cards": {"total": 2, "open": 1, "critical_open": 1},
        "blocking_cards": [{"id": "CARD-1", "task_id": "TASK-1", "summary": "bad"}],
        "pending_signal_items": [{"client": "claude", "task_id": "TASK-1", "transcript_path": "t", "review_command": "dev watch review"}],
    }
    monkeypatch.setattr(watch_cmd, "live_review_summary", lambda root, task_id=None: payload)
    status = runner.invoke(app, ["watch", "status", "--project-root", str(tmp_path)])
    status_json = runner.invoke(app, ["watch", "status", "--json", "--project-root", str(tmp_path)])
    assert "Blocking Live-Review Cards" in status.output
    assert json.loads(status_json.output)["active_task_id"] == "TASK-1"

    card = SimpleNamespace(
        id="CARD-1",
        verdict="Approved",
        status="open",
        task_id="TASK-1",
        summary="summary",
        concerns=[],
        alternatives=[],
        evidence_requests=[],
        message_for_agent="ok",
        client="claude",
        model_dump=lambda: {"id": "CARD-1", "status": "resolved"},
    )
    monkeypatch.setattr(watch_cmd, "update_card_status", lambda root, card_id, status: card)
    logged = []
    monkeypatch.setattr(watch_cmd, "TraceLogger", lambda root: SimpleNamespace(log_event=lambda *a, **k: logged.append(a[0])))
    resolved = runner.invoke(app, ["watch", "resolve", "CARD-1", "--status", "resolved", "--json", "--project-root", str(tmp_path)])
    assert resolved.exit_code == 0
    assert json.loads(resolved.output)["ok"] is True
    assert "live_review_card_status_updated" in logged

    monkeypatch.setattr(watch_cmd, "get_card", lambda root, card_id: card)
    monkeypatch.setattr(watch_cmd, "build_live_repair_prompt", lambda root, card_arg: "repair prompt")
    repair = runner.invoke(app, ["watch", "repair", "CARD-1", "--json", "--project-root", str(tmp_path)])
    assert json.loads(repair.output)["prompt"] == "repair prompt"

    monkeypatch.setattr(watch_cmd, "load_cards", lambda root: [card])
    monkeypatch.setattr(watch_cmd, "build_bulk_live_repair_prompt", lambda root, cards: f"bulk:{len(cards)}")
    repair_all = runner.invoke(app, ["watch", "repair-all", "--json", "--project-root", str(tmp_path)])
    assert json.loads(repair_all.output)["prompt"] == "bulk:1"

    signal = SimpleNamespace(
        client="claude",
        transcript_path=str(transcript),
        task_id="TASK-1",
        path="signal.json",
        review_command="cmd",
        model_dump=lambda: {"client": "claude"},
    )
    monkeypatch.setattr(watch_cmd, "load_signals", lambda root: [signal])
    signals_out = runner.invoke(app, ["watch", "signals", "--json", "--project-root", str(tmp_path)])
    assert json.loads(signals_out.output)["signals"] == [{"client": "claude"}]

    turn = SimpleNamespace(turn_id="turn-1")
    monkeypatch.setattr(watch_cmd, "latest_assistant_turn", lambda path, client: turn)

    async def fake_review(turn_arg, root, client, use_llm, task_id=None):
        return card

    monkeypatch.setattr(watch_cmd, "_review_turn", fake_review)
    monkeypatch.setattr(watch_cmd, "_save_card_once", lambda root, card_arg, persist, force: (tmp_path / "card.json", False))
    monkeypatch.setattr(watch_cmd, "mark_processed", lambda signal_arg, root: tmp_path / "processed")
    pending = runner.invoke(app, ["watch", "pending", "--json", "--project-root", str(tmp_path)])
    assert json.loads(pending.output)["reviewed"][0]["path"].endswith("card.json")

    no_new = runner.invoke(app, ["watch", "follow", "--transcript", str(transcript), "--once", "--project-root", str(tmp_path)])
    assert no_new.exit_code == 0
    assert "Saved critique card" in no_new.output

    monkeypatch.setattr(watch_cmd, "load_turns", lambda transcript_arg, client: [SimpleNamespace(model_dump=lambda: {"turn": 1})])
    imported = runner.invoke(app, ["watch", "import", str(transcript), "--json"])
    assert json.loads(imported.output)["turns"] == [{"turn": 1}]


def test_runs_commands_list_show_and_helpers(tmp_path, monkeypatch):
    runs_dir = tmp_path / ".devcouncil" / "runs"
    run1 = runs_dir / "run-1"
    run2 = runs_dir / "run-2"
    run1.mkdir(parents=True)
    run2.mkdir()
    (run1 / "agent-run.json").write_text(
        json.dumps({"run_id": "run-1", "task_id": "TASK-1", "agent": "claude", "status": "running", "transcript": "transcript.txt"}),
        encoding="utf-8",
    )
    (run1 / "transcript.txt").write_text("api_key=abcdef1234567890\nline\n" * 50, encoding="utf-8")
    (run2 / "agent-run.json").write_text("{bad", encoding="utf-8")
    old = 1_000_000
    os.utime(run1 / "agent-run.json", (old, old))
    monkeypatch.setattr(runs_cmd.time, "time", lambda: old + 9999)
    monkeypatch.setattr(runs_cmd, "_orphan_after_seconds", lambda root: 10)

    assert runs_cmd._load_manifest(run2 / "agent-run.json") is None
    assert runs_cmd._find_transcript(run1, {"transcript": "transcript.txt"}) == run1 / "transcript.txt"
    assert "[REDACTED:generic_api_key]" in runs_cmd._transcript_tail(run1 / "transcript.txt")

    listed = runner.invoke(app, ["runs", "list", "--json", "--project-root", str(tmp_path)])
    assert listed.exit_code == 0
    assert json.loads(listed.output)["runs"][0]["orphaned"] is True
    plain = runner.invoke(app, ["runs", "list", "--project-root", str(tmp_path)])
    assert "orphaned" in plain.output
    filtered = runner.invoke(app, ["runs", "list", "--status", "finished", "--project-root", str(tmp_path)])
    assert "No agent runs" in filtered.output

    shown = runner.invoke(app, ["runs", "show", "run-1", "--json", "--project-root", str(tmp_path)])
    payload = json.loads(shown.output)
    assert payload["ok"] is True
    assert payload["orphaned"] is True
    assert "[REDACTED:generic_api_key]" in payload["transcript_tail"]
    missing = runner.invoke(app, ["runs", "show", "missing", "--json", "--project-root", str(tmp_path)])
    assert missing.exit_code == 1
    assert json.loads(missing.output)["ok"] is False


def test_okf_cli_export_ingest_validate_and_html(monkeypatch, tmp_path):
    monkeypatch.setattr(okf_cmd, "initialize_project", lambda root, quiet=True: None)
    monkeypatch.setattr(okf_cmd, "get_db", lambda root: None)
    no_db = runner.invoke(app, ["okf", "export", "--project-root", str(tmp_path)])
    assert no_db.exit_code == 1

    class FakeGraphRepo:
        def __init__(self, session):
            pass

        def load_graph(self):
            return "graph"

    monkeypatch.setattr(okf_cmd, "get_db", lambda root: _FakeDb())
    monkeypatch.setattr(okf_cmd, "ArtifactGraphRepository", FakeGraphRepo)
    monkeypatch.setattr("devcouncil.skills.registry.load_skills", lambda project_root=None: [SimpleNamespace(name="skill")])
    monkeypatch.setattr("devcouncil.app.config.load_config", lambda root: SimpleNamespace(project=SimpleNamespace(name="Project"), knowledge=SimpleNamespace(directory=".devcouncil/knowledge")))
    design_dir = tmp_path / ".devcouncil" / "knowledge" / "design"
    design_dir.mkdir(parents=True)
    (design_dir / "design.md").write_text("---\nname: Design\n---\n# Overview\n", encoding="utf-8")
    monkeypatch.setattr(
        "devcouncil.reporting.okf_bundle_writer.OKFBundleWriter.generate",
        lambda *a, **k: [tmp_path / "out" / "index.md", tmp_path / "out" / "doc.md"],
    )
    exported = runner.invoke(app, ["okf", "export", "--output", str(tmp_path / "out"), "--project-root", str(tmp_path)])
    assert exported.exit_code == 0
    assert "Exported 2 OKF documents" in exported.output
    assert "Included 1 engineering skill" in exported.output
    assert "Included 1 design" in exported.output

    class Fetched:
        def __init__(self, directory, name="bundle"):
            self.directory = directory
            self.suggested_name = name
            self.cleaned = False

        def cleanup(self):
            self.cleaned = True

    source = tmp_path / "source"
    source.mkdir()
    (source / "a.md").write_text("# A\n", encoding="utf-8")
    fetched = Fetched(source, "fetched")
    monkeypatch.setattr("devcouncil.knowledge.fetch.fetch_bundle", lambda bundle: fetched)
    monkeypatch.setattr(okf_cmd, "read_bundle", lambda src: SimpleNamespace(documents=[object()]))
    monkeypatch.setattr(okf_cmd, "validate_bundle", lambda parsed: ["warning"])
    ingested = runner.invoke(app, ["okf", "ingest", str(source), "--project-root", str(tmp_path)])
    assert ingested.exit_code == 0
    assert "validation issue" in ingested.output
    assert "Ingested 1 OKF document" in ingested.output
    assert fetched.cleaned is True

    monkeypatch.setattr("devcouncil.knowledge.fetch.fetch_bundle", lambda bundle: (_ for _ in ()).throw(RuntimeError("bad source")))
    bad_ingest = runner.invoke(app, ["okf", "ingest", "bad", "--project-root", str(tmp_path)])
    assert bad_ingest.exit_code == 1
    assert "Could not fetch bundle" in bad_ingest.output

    not_dir = runner.invoke(app, ["okf", "validate", str(tmp_path / "missing")])
    assert not_dir.exit_code == 1
    monkeypatch.setattr(okf_cmd, "read_bundle", lambda src: SimpleNamespace(documents=[1, 2]))
    monkeypatch.setattr(okf_cmd, "validate_bundle", lambda parsed: [])
    valid = runner.invoke(app, ["okf", "validate", str(source)])
    assert valid.exit_code == 0
    assert "Valid OKF bundle" in valid.output
    monkeypatch.setattr(okf_cmd, "validate_bundle", lambda parsed: ["bad link"])
    invalid = runner.invoke(app, ["okf", "validate", str(source)])
    assert invalid.exit_code == 1
    assert "bad link" in invalid.output

    monkeypatch.setattr(okf_cmd, "read_bundle", lambda src: SimpleNamespace(documents=[]))
    empty_html = runner.invoke(app, ["okf", "html", str(source)])
    assert empty_html.exit_code == 1
    monkeypatch.setattr(okf_cmd, "read_bundle", lambda src: SimpleNamespace(documents=[object()]))
    monkeypatch.setattr("devcouncil.reporting.okf_html.write_bundle_html", lambda parsed, out_dir: [out_dir / "index.html"])
    html = runner.invoke(app, ["okf", "html", str(source), "--output", str(tmp_path / "site")])
    assert html.exit_code == 0
    assert "Rendered 1 page" in html.output


def test_integrate_config_writers_hooks_and_subprocess_helpers(monkeypatch, tmp_path, capsys):
    assert integrate_cmd._project_root(tmp_path) == tmp_path.resolve()
    assert integrate_cmd._codex_command(tmp_path)[:4] == ["codex", "mcp", "add", "devcouncil"]
    assert "--scope" in integrate_cmd._gemini_command(tmp_path, "project")
    assert integrate_cmd._claude_command(tmp_path, "local")[4:6] == ["local", "devcouncil"]
    assert integrate_cmd._cursor_mcp_config(tmp_path)["mcpServers"]["devcouncil"]["env"]["DEVCOUNCIL_PROJECT_ROOT"] == str(tmp_path)
    assert integrate_cmd._warp_mcp_config(tmp_path)["devcouncil"]["args"] == ["mcp-server"]
    assert integrate_cmd._opencode_mcp_entry(tmp_path)["enabled"] is True
    assert integrate_cmd._antigravity_mcp_config(tmp_path)["mcpServers"]["devcouncil"]["cwd"] == str(tmp_path)

    integrate_cmd._configure_cursor(tmp_path, apply=False)
    integrate_cmd._configure_opencode(tmp_path, apply=False)
    integrate_cmd._configure_antigravity(tmp_path, apply=False)
    integrate_cmd._configure_warp(tmp_path, apply=False)
    preview = capsys.readouterr().out
    assert "Cursor" in preview
    assert "OpenCode" in preview
    assert "Antigravity" in preview
    assert "Warp" in preview

    monkeypatch.setattr(integrate_cmd.shutil, "which", lambda command: None)
    assert integrate_cmd._configure("Tool", ["missing", "arg"], apply=True) is False
    assert integrate_cmd._run(["missing"]) == 127
    assert integrate_cmd._run_capture(["missing"])[0] == 127

    monkeypatch.setattr(integrate_cmd.shutil, "which", lambda command: "/bin/tool")
    monkeypatch.setattr(integrate_cmd.subprocess, "run", lambda *a, **k: SimpleNamespace(returncode=0, stdout="out", stderr="err"))
    assert integrate_cmd._configure("Tool", ["tool", "arg"], apply=False) is True
    assert integrate_cmd._configure("Tool", ["tool", "arg"], apply=True) is True
    assert integrate_cmd._run(["tool"]) == 0
    assert integrate_cmd._run_capture(["tool"]) == (0, "outerr")
    monkeypatch.setattr(integrate_cmd.subprocess, "run", lambda *a, **k: (_ for _ in ()).throw(subprocess.TimeoutExpired("tool", 1)))
    assert integrate_cmd._run_capture(["tool"], timeout=1) == (124, "timed out")
    monkeypatch.setattr(integrate_cmd.subprocess, "run", lambda *a, **k: (_ for _ in ()).throw(OSError("boom")))
    assert integrate_cmd._run(["tool"]) == 127
    assert "could not be executed" in integrate_cmd._run_capture(["tool"])[1]

    monkeypatch.setattr(integrate_cmd.sys, "platform", "win32")
    assert integrate_cmd._quote_powershell_arg("plain") == "plain"
    assert integrate_cmd._quote_powershell_arg("has space") == "'has space'"
    assert integrate_cmd._quote_powershell_arg("it's") == "'it''s'"
    assert integrate_cmd._format_command(["cmd", "has space"]) == "cmd 'has space'"
    monkeypatch.setattr(integrate_cmd.sys, "platform", sys.platform)

    bad_json = tmp_path / "bad.json"
    bad_json.write_text("{bad", encoding="utf-8")
    with pytest.raises(ValueError):
        integrate_cmd._load_json_strict(bad_json, "Bad")
    assert integrate_cmd._load_json(tmp_path / "missing.json") == {}
    assert integrate_cmd._load_json(bad_json) == {}

    integrate_cmd._write_cursor_config(tmp_path)
    integrate_cmd._write_warp_mcp_config(tmp_path)
    integrate_cmd._write_opencode_config(tmp_path)
    integrate_cmd._write_antigravity_mcp_config(tmp_path)
    assert (tmp_path / ".cursor" / "mcp.json").exists()
    assert (tmp_path / ".devcouncil" / "integrations" / "warp-mcp.json").exists()
    assert (tmp_path / "opencode.json").exists()
    assert (tmp_path / ".agents" / "mcp_config.json").exists()

    with integrate_cmd._batched_raw_config(tmp_path):
        integrate_cmd._record_cursor_config(tmp_path)
        integrate_cmd._record_warp_config(tmp_path)
        integrate_cmd._record_opencode_config(tmp_path)
        integrate_cmd._record_antigravity_config(tmp_path)
        with integrate_cmd._batched_raw_config(tmp_path):
            integrate_cmd._mutate_raw_config(tmp_path, lambda config: config.setdefault("nested", True))
    config = integrate_cmd._load_raw_config(tmp_path)
    assert config["integrations"]["cursor"]["enabled"] is True
    assert config["integrations"]["warp"]["enabled"] is True
    assert config["integrations"]["opencode"]["enabled"] is True
    assert config["integrations"]["antigravity"]["enabled"] is True
    assert config["nested"] is True

    settings = {}
    integrate_cmd._upsert_hook(settings, "Event", "Matcher", "cmd", "name")
    integrate_cmd._upsert_hook(settings, "Event", "Matcher", "cmd", "name")
    assert len(settings["hooks"]["Event"][0]["hooks"]) == 1
    integrate_cmd._upsert_cursor_hook(settings, "preToolUse", "Shell", "cmd")
    integrate_cmd._upsert_cursor_hook(settings, "preToolUse", "Shell", "cmd")
    assert len(settings["hooks"]["preToolUse"]) == 1

    (tmp_path / ".codex").mkdir(exist_ok=True)
    (tmp_path / ".codex" / "config.toml").write_text("[features]\nold = true\n[other]\nx = 1\n", encoding="utf-8")
    codex_written = integrate_cmd._install_codex_hooks(tmp_path)
    gemini_written = integrate_cmd._install_gemini_hooks(tmp_path)
    cursor_written = integrate_cmd._install_cursor_hooks(tmp_path)
    claude_written = integrate_cmd._install_claude_hooks(tmp_path, write_gate=True)
    assert any(path.name == "hooks.json" for path in codex_written)
    assert "codex_hooks = true" in (tmp_path / ".codex" / "config.toml").read_text(encoding="utf-8")
    assert gemini_written == [tmp_path / ".gemini" / "settings.json"]
    assert cursor_written == [tmp_path / ".cursor" / "hooks.json"]
    claude_settings = json.loads((tmp_path / ".claude" / "settings.local.json").read_text(encoding="utf-8"))
    assert "PreToolUse" in claude_settings["hooks"]
    assert "SessionStart" in claude_settings["hooks"]

    source = tmp_path / "plugin-source.mjs"
    source.write_text("export default {}\n", encoding="utf-8")
    monkeypatch.setattr(integrate_cmd, "_opencode_plugin_source", lambda: source)
    opencode_written = integrate_cmd._install_opencode_hooks(tmp_path)
    assert any(path.name == integrate_cmd.OPENCODE_HOOK_PLUGIN_NAME for path in opencode_written)
    assert "./.devcouncil/integrations/opencode_devcouncil_plugin.mjs" in json.loads((tmp_path / "opencode.json").read_text(encoding="utf-8"))["plugin"]

    assert ("codex", tmp_path / ".codex" / "hooks.json") in integrate_cmd._preview_hook_paths(tmp_path, "all")
    with pytest.raises(typer.Exit):
        integrate_cmd._configure_native_hooks(tmp_path, tool="bad", apply=False)
    integrate_cmd._configure_native_hooks(tmp_path, tool="codex", apply=False)
    monkeypatch.setattr(integrate_cmd, "_install_codex_hooks", lambda root: [root / "codex"])
    integrate_cmd._configure_native_hooks(tmp_path, tool="codex", apply=True)
    out = capsys.readouterr().out
    assert "Native hook config preview" in out
    assert "codex native hooks configured" in out


def test_integrate_cli_overview_status_matrix_and_doctor(monkeypatch, tmp_path):
    overview = runner.invoke(app, ["integrate"])
    assert overview.exit_code == 0
    assert "DevCouncil Coding CLI Integrations" in overview.output

    monkeypatch.setattr(integrate_cmd, "_project_root", lambda path=None: tmp_path)
    monkeypatch.setattr(integrate_cmd.shutil, "which", lambda executable: f"/bin/{executable}" if executable in {"codex", "cursor-agent"} else None)
    monkeypatch.setattr(integrate_cmd, "load_agent_profiles", lambda root: {"default": {}})
    monkeypatch.setattr(
        integrate_cmd,
        "load_cli_agent_specs",
        lambda root: {
            "custom": SimpleNamespace(
                built_in=False,
                executable="custom-tool",
                input_mode="bad",
                default_profile="missing",
            )
        },
    )
    doctor = runner.invoke(app, ["integrate", "doctor", "--project-root", str(tmp_path)])
    assert doctor.exit_code == 0
    assert "Integration Doctor" in doctor.output
    assert "CLI agent: custom" in doctor.output

    monkeypatch.setattr(
        integrate_cmd,
        "CODING_CLI_INTEGRATION_INFO",
        {
            "codex": SimpleNamespace(label="Codex", tier=1, mcp=True, hooks=True, enforcement="pre-action", notes="ok"),
            "aider": SimpleNamespace(label="Aider", tier=3, mcp=False, hooks=False, enforcement="verify-only", notes="verify only"),
        },
    )
    monkeypatch.setattr(integrate_cmd, "integration_tier_label", lambda tier: f"tier:{tier}")
    matrix = runner.invoke(app, ["integrate", "matrix"])
    assert matrix.exit_code == 0
    assert "Integration Matrix" in matrix.output
    assert "pre-action" in matrix.output
    status = runner.invoke(app, ["integrate", "status", "--project-root", str(tmp_path)])
    assert status.exit_code == 0
    assert "DevCouncil Integration Status" in status.output


def test_integrate_subcommand_apply_preview_and_failure_branches(monkeypatch, tmp_path):
    monkeypatch.setattr(integrate_cmd, "_project_root", lambda path=None: tmp_path)

    configure_calls = []
    monkeypatch.setattr(
        integrate_cmd,
        "_configure",
        lambda tool, command, apply: configure_calls.append((tool, command[0], apply)) or not (tool == "Codex CLI" and apply),
    )
    codex_fail = runner.invoke(app, ["integrate", "codex", "--apply", "--project-root", str(tmp_path)])
    assert codex_fail.exit_code == 1
    gemini_bad = runner.invoke(app, ["integrate", "gemini", "--scope", "bad", "--project-root", str(tmp_path)])
    assert gemini_bad.exit_code == 2
    gemini_ok = runner.invoke(app, ["integrate", "gemini", "--scope", "user", "--project-root", str(tmp_path)])
    assert gemini_ok.exit_code == 0
    assert any(call[0] == "Gemini CLI" for call in configure_calls)

    monkeypatch.setattr(integrate_cmd, "_uninstall_claude", lambda root: ["hooks", "mcp"])
    uninstall = runner.invoke(app, ["integrate", "claude", "--uninstall", "--project-root", str(tmp_path)])
    assert "Removed DevCouncil Claude integration" in uninstall.output
    monkeypatch.setattr(integrate_cmd, "_uninstall_claude", lambda root: [])
    uninstall_empty = runner.invoke(app, ["integrate", "claude", "--uninstall", "--project-root", str(tmp_path)])
    assert "Nothing to remove" in uninstall_empty.output
    claude_bad_scope = runner.invoke(app, ["integrate", "claude", "--scope", "bad", "--project-root", str(tmp_path)])
    assert claude_bad_scope.exit_code == 2

    monkeypatch.setattr(integrate_cmd, "_configure", lambda tool, command, apply: True)
    monkeypatch.setattr(integrate_cmd, "_install_claude_hooks", lambda root, write_gate=False: [root / "hooks.json"])
    monkeypatch.setattr(integrate_cmd, "_install_claude_assets", lambda root: [root / "asset.md"])
    claude_apply = runner.invoke(app, ["integrate", "claude", "--apply", "--write-gate", "--project-root", str(tmp_path)])
    assert claude_apply.exit_code == 0
    assert "with write-gate" in claude_apply.output
    monkeypatch.setattr(integrate_cmd, "_install_claude_assets", lambda root: (_ for _ in ()).throw(OSError("asset boom")))
    claude_error = runner.invoke(app, ["integrate", "claude", "--apply", "--project-root", str(tmp_path)])
    assert claude_error.exit_code == 1
    assert "Claude asset setup failed" in claude_error.output

    fake_assets = SimpleNamespace(
        build_slash_commands=lambda root: [SimpleNamespace(path=root / ".claude" / "commands" / "dev.md")],
        build_subagents=lambda root: [SimpleNamespace(path=root / ".claude" / "agents" / "dev.md")],
        build_output_style=lambda root: [SimpleNamespace(path=root / ".claude" / "styles" / "dev.md")],
    )
    monkeypatch.setitem(sys.modules, "devcouncil.integrations.claude_assets", fake_assets)
    assets_preview = runner.invoke(app, ["integrate", "claude-assets", "--project-root", str(tmp_path)])
    assert assets_preview.exit_code == 0
    assert "Claude Code assets (preview)" in assets_preview.output
    monkeypatch.setattr(integrate_cmd, "_install_claude_assets", lambda root: [root / ".claude" / "commands" / "dev.md"])
    assets_apply = runner.invoke(app, ["integrate", "claude-assets", "--apply", "--project-root", str(tmp_path)])
    assert "Wrote 1 Claude asset" in assets_apply.output
    monkeypatch.setattr(integrate_cmd, "_install_claude_assets", lambda root: (_ for _ in ()).throw(ValueError("bad assets")))
    assets_error = runner.invoke(app, ["integrate", "claude-assets", "--apply", "--project-root", str(tmp_path)])
    assert assets_error.exit_code == 1

    monkeypatch.setitem(sys.modules, "devcouncil.integrations.claude_assets", SimpleNamespace(PLUGIN_ROOT_REL=".devcouncil/claude-plugin"))
    plugin_preview = runner.invoke(app, ["integrate", "claude-plugin", "--project-root", str(tmp_path)])
    assert "plugin bundle (preview)" in plugin_preview.output
    monkeypatch.setattr(integrate_cmd, "_install_claude_plugin", lambda root, write_gate=False: [root / "plugin.json"])
    plugin_apply = runner.invoke(app, ["integrate", "claude-plugin", "--apply", "--project-root", str(tmp_path)])
    assert "Built Claude plugin bundle" in plugin_apply.output
    monkeypatch.setattr(integrate_cmd, "_install_claude_plugin", lambda root, write_gate=False: (_ for _ in ()).throw(FileNotFoundError("missing")))
    plugin_error = runner.invoke(app, ["integrate", "claude-plugin", "--apply", "--project-root", str(tmp_path)])
    assert plugin_error.exit_code == 1

    reports = []

    class FakeReport:
        def __init__(self, ok):
            self.ok = ok

        def to_json(self):
            return '{"ok": false}'

    def fake_apply(root, target, **kwargs):
        reports.append((target, kwargs))
        return FakeReport(ok=target != "opencode")

    monkeypatch.setattr(integrate_cmd, "apply_integration_target", fake_apply)
    cursor = runner.invoke(app, ["integrate", "cursor", "--apply", "--project-root", str(tmp_path)])
    opencode = runner.invoke(app, ["integrate", "opencode", "--apply", "--project-root", str(tmp_path)])
    antigravity = runner.invoke(app, ["integrate", "antigravity", "--apply", "--project-root", str(tmp_path)])
    warp = runner.invoke(app, ["integrate", "warp", "--apply", "--project-root", str(tmp_path)])
    aider = runner.invoke(app, ["integrate", "aider", "--apply", "--project-root", str(tmp_path)])
    assert cursor.exit_code == 0
    assert opencode.exit_code == 1
    assert antigravity.exit_code == 0
    assert warp.exit_code == 0
    assert aider.exit_code == 0
    assert [target for target, _ in reports] == ["cursor", "opencode", "antigravity", "warp", "aider"]

    monkeypatch.setattr(integrate_cmd, "_configure_aider", lambda root, apply: True)
    aider_preview = runner.invoke(app, ["integrate", "aider", "--project-root", str(tmp_path)])
    assert aider_preview.exit_code == 0

    monkeypatch.setattr(integrate_cmd, "is_reserved_agent_name", lambda name: name == "codex")
    monkeypatch.setattr(integrate_cmd, "load_agent_profiles", lambda root: {"default": {}})
    cli_bad_mode = runner.invoke(app, ["integrate", "cli-agent", "x", "--command", "tool", "--input-mode", "bad", "--project-root", str(tmp_path)])
    cli_blank = runner.invoke(app, ["integrate", "cli-agent", " ", "--command", "tool", "--project-root", str(tmp_path)])
    cli_reserved = runner.invoke(app, ["integrate", "cli-agent", "codex", "--command", "tool", "--project-root", str(tmp_path)])
    cli_preview = runner.invoke(app, ["integrate", "cli-agent", "mine", "--command", "tool", "--arg", "--flag", "--project-root", str(tmp_path)])
    cli_apply = runner.invoke(app, ["integrate", "cli-agent", "mine", "--command", "tool", "--apply", "--project-root", str(tmp_path)])
    assert cli_bad_mode.exit_code == 2
    assert cli_blank.exit_code == 2
    assert cli_reserved.exit_code == 2
    assert "Bring your own CLI executor preview" in cli_preview.output
    assert "Registered CLI executor" in cli_apply.output

    all_bad_gemini = runner.invoke(app, ["integrate", "all", "--gemini-scope", "bad", "--project-root", str(tmp_path)])
    all_bad_claude = runner.invoke(app, ["integrate", "all", "--claude-scope", "bad", "--project-root", str(tmp_path)])
    assert all_bad_gemini.exit_code == 2
    assert all_bad_claude.exit_code == 2
    monkeypatch.setattr(integrate_cmd, "apply_integration_target", lambda root, target, **kwargs: FakeReport(ok=True))
    all_apply = runner.invoke(app, ["integrate", "all", "--apply", "--strict", "--project-root", str(tmp_path)])
    assert "Coding CLI integrations configured" in all_apply.output

    monkeypatch.setattr(integrate_cmd, "resolve_coding_cli_probe_order", lambda root: ["codex", "gemini"])
    monkeypatch.setattr(integrate_cmd, "detect_available_coding_cli", lambda root, probe_order=None: "codex")
    monkeypatch.setattr(integrate_cmd, "resolve_automated_executor", lambda root, executor: "codex")
    monkeypatch.setattr(integrate_cmd, "resolve_coding_cli_executable", lambda root, client: f"/bin/{client}" if client == "codex" else None)
    monkeypatch.setattr(integrate_cmd, "integration_status_summary", lambda root: {"custom_probe_order": False, "probe_order": ["codex", "gemini"]})
    recommend = runner.invoke(app, ["integrate", "recommend", "--project-root", str(tmp_path)])
    assert "Recommended executor" in recommend.output
    monkeypatch.setattr(integrate_cmd, "detect_available_coding_cli", lambda root, probe_order=None: None)
    recommend_none = runner.invoke(app, ["integrate", "recommend", "--project-root", str(tmp_path)])
    assert "No built-in coding CLI" in recommend_none.output


def test_config_models_command_branches(monkeypatch, tmp_path):
    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    config_path = dev_dir / "config.yaml"
    config_path.write_text(
        "models:\n  provider: openrouter\n  roles:\n    planner_a:\n      model: old\n",
        encoding="utf-8",
    )
    monkeypatch.setattr(config_cmd, "load_config", lambda root: SimpleNamespace())

    show_all = runner.invoke(app, ["config", "models", "--project-root", str(tmp_path)])
    assert show_all.exit_code == 0
    assert "Model Configuration" in show_all.output
    show_role = runner.invoke(app, ["config", "models", "--role", "planner_a", "--project-root", str(tmp_path)])
    assert show_role.exit_code == 0
    assert "old" in show_role.output
    missing_role = runner.invoke(app, ["config", "models", "--role", "critic", "--project-root", str(tmp_path)])
    assert "Role 'critic' not found" in missing_role.output

    provider = runner.invoke(app, ["config", "models", "--provider", "ollama-local", "--project-root", str(tmp_path)])
    assert provider.exit_code == 0
    assert "Updated model provider" in provider.output
    all_roles = runner.invoke(app, ["config", "models", "--model", "shared", "--role-model", "critic_a=critic", "--project-root", str(tmp_path)])
    assert all_roles.exit_code == 0
    assert "Updated all model roles" in all_roles.output
    one_role = runner.invoke(app, ["config", "models", "--role", "new_role", "--model", "new-model", "--project-root", str(tmp_path)])
    assert one_role.exit_code == 0
    assert "new_role" in one_role.output

    invalid_provider = runner.invoke(app, ["config", "models", "--provider", "bad", "--project-root", str(tmp_path)])
    assert invalid_provider.exit_code == 2
    invalid_role_model = runner.invoke(app, ["config", "models", "--role-model", "bad", "--project-root", str(tmp_path)])
    assert invalid_role_model.exit_code == 2
    monkeypatch.setattr(config_cmd, "load_config", lambda root: (_ for _ in ()).throw(FileNotFoundError("missing config")))
    missing = runner.invoke(app, ["config", "models", "--project-root", str(tmp_path)])
    assert missing.exit_code == 0
    assert "missing config" in missing.output


def test_hook_commands_pre_post_lifecycle_and_statusline(monkeypatch, tmp_path, capsys):
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    assert hook_cmd._project_root(None) == tmp_path.resolve()
    assert hook_cmd._project_root(tmp_path) == tmp_path.resolve()

    monkeypatch.setattr(hook_cmd, "active_task_id", lambda root: "TASK-1")
    monkeypatch.setattr(hook_cmd, "get_db", lambda root: None)
    assert hook_cmd._active_task(tmp_path) is None
    hook_cmd._emit_decision("codex", "allow", "ok")
    assert json.loads(capsys.readouterr().out)["decision"] == "allow"
    hook_cmd._emit_decision("gemini", "warn", "careful")
    assert "systemMessage" in capsys.readouterr().out
    with pytest.raises(typer.Exit) as denied:
        hook_cmd._emit_decision("claude", "deny", "blocked")
    assert denied.value.exit_code == 2
    assert "blocked" in capsys.readouterr().err

    empty = runner.invoke(app, ["hook", "pre-tool-use", "--client", "codex"], input="")
    assert empty.exit_code == 0
    assert json.loads(empty.output)["decision"] == "allow"
    malformed = runner.invoke(app, ["hook", "pre-tool-use", "{bad", "--strict", "--client", "claude"])
    assert malformed.exit_code == 2
    assert "strict mode" in malformed.stderr

    class FakeHookPolicy:
        def __init__(self, project_root):
            pass

        def evaluate(self, call_data, active_task):
            return SimpleNamespace(action="warn", reason=f"checked {call_data['tool']}")

    monkeypatch.setattr(hook_cmd, "HookPolicy", FakeHookPolicy)
    valid = runner.invoke(app, ["hook", "pre-tool-use", '{"tool": "Write"}', "--client", "gemini", "--project-root", str(tmp_path)])
    assert valid.exit_code == 0
    assert json.loads(valid.output)["systemMessage"].endswith("checked Write")

    post = runner.invoke(app, ["hook", "post-tool-use", "{}", "--client", "codex", "--project-root", str(tmp_path)])
    assert post.exit_code == 0
    assert json.loads(post.output)["decision"] == "allow"

    logged = []
    monkeypatch.setattr(hook_cmd, "write_signal", lambda root, client, payload: tmp_path / f"{client}.signal")
    monkeypatch.setattr(hook_cmd, "TraceLogger", lambda root: SimpleNamespace(log_event=lambda *a, **k: logged.append((a, k))))
    agent = runner.invoke(app, ["hook", "agent-response", "{bad", "--client", "codex", "--project-root", str(tmp_path)])
    assert agent.exit_code == 0
    assert json.loads(agent.output)["decision"] == "allow"
    assert logged[-1][0][0] == "agent_response_ready"

    class FakeGraph:
        def coverage_summary(self):
            return {"total_tasks": 2, "total_gaps": 1, "blocking_gaps": 1}

    class FakeGraphRepo:
        def __init__(self, session):
            pass

        def load_graph(self):
            return FakeGraph()

    class FakeStateRepo:
        def __init__(self, session):
            pass

        def get_state(self):
            return SimpleNamespace(current_phase="PLAN_APPROVED")

    monkeypatch.setattr(hook_cmd, "get_db", lambda root: _FakeDb())
    monkeypatch.setattr("devcouncil.storage.repositories.ArtifactGraphRepository", FakeGraphRepo)
    monkeypatch.setattr("devcouncil.storage.repositories.StateRepository", FakeStateRepo)
    assert "tasks: 2" in hook_cmd._status_line(tmp_path)
    hook_cmd._emit_additional_context("Event", "context")
    hook_cmd._emit_system_message("Event", "message")
    output_lines = capsys.readouterr().out.splitlines()
    assert "additionalContext" in output_lines[-2]
    assert "systemMessage" in output_lines[-1]
    monkeypatch.setattr(hook_cmd, "session_briefing", lambda root: "DevCouncil session")
    assert hook_cmd._session_start_context(tmp_path, {"source": "normal"}).startswith("DevCouncil")
    monkeypatch.setattr(hook_cmd, "compact_briefing", lambda root, payload: "compact context")
    assert hook_cmd._session_start_context(tmp_path, {"source": "compact"}) == "compact context"
    assert hook_cmd._read_stdin_payload("") == {}
    assert hook_cmd._read_stdin_payload("[]") == {"raw": "[]"}
    assert hook_cmd._read_stdin_payload("{bad") == {"raw": "{bad"}

    monkeypatch.setattr(hook_cmd, "session_briefing", lambda root: "session context")
    session_start = runner.invoke(app, ["hook", "session-start", '{"source": "normal"}', "--project-root", str(tmp_path)])
    assert "additionalContext" in session_start.output

    monkeypatch.setattr(hook_cmd, "compact_snapshot_recent", lambda root, seconds: True)
    skipped_prompt = runner.invoke(app, ["hook", "user-prompt-submit", "{}", "--project-root", str(tmp_path)])
    assert skipped_prompt.output == ""
    monkeypatch.setattr(hook_cmd, "compact_snapshot_recent", lambda root, seconds: False)
    monkeypatch.setattr(hook_cmd, "_status_line", lambda root: "status line")
    prompt = runner.invoke(app, ["hook", "user-prompt-submit", "{}", "--project-root", str(tmp_path)])
    assert "status line" in prompt.output

    session_end = runner.invoke(app, ["hook", "session-end", "{}", "--project-root", str(tmp_path)])
    post_compact = runner.invoke(app, ["hook", "post-compact", "{}", "--project-root", str(tmp_path)])
    notification = runner.invoke(app, ["hook", "notification", '{"message": "hello"}', "--project-root", str(tmp_path)])
    assert session_end.exit_code == 0
    assert post_compact.exit_code == 0
    assert notification.exit_code == 0

    monkeypatch.setattr(hook_cmd, "build_compact_snapshot", lambda root, payload: {"session_id": "s1", "task_id": "TASK-1"})
    monkeypatch.setattr(hook_cmd, "compact_snapshot_path", lambda root: tmp_path / ".devcouncil" / "state" / "compact_snapshot.json")
    monkeypatch.setattr(hook_cmd, "write_json", lambda path, data: path.parent.mkdir(parents=True, exist_ok=True) or path.write_text(json.dumps(data), encoding="utf-8"))
    monkeypatch.setattr(
        "devcouncil.app.config.load_config",
        lambda root: SimpleNamespace(execution=SimpleNamespace(compact_snapshot_toast=True, verify_on_post_task=False, skip_prompt_status_after_compact_seconds=60)),
    )
    pre_compact = runner.invoke(app, ["hook", "pre-compact", '{"session_id": "s1"}', "--project-root", str(tmp_path)])
    assert "compact snapshot" in pre_compact.output

    subagent = runner.invoke(app, ["hook", "subagent-stop", "{}", "--project-root", str(tmp_path)])
    assert subagent.exit_code == 0
    statusline_missing = runner.invoke(app, ["hook", "claude-statusline", "{}", "--project-root", str(tmp_path)])
    assert "status line" in statusline_missing.output
    monkeypatch.setattr(hook_cmd, "_status_line", lambda root: None)
    statusline_none = runner.invoke(app, ["hook", "claude-statusline", "{}", "--project-root", str(tmp_path)])
    assert "not initialized" in statusline_none.output

    post_task = runner.invoke(app, ["hook", "post-task", "--client", "codex", "--project-root", str(tmp_path)])
    assert "coding agent finished task" in post_task.output
    assert '"decision":"allow"' in post_task.output
    monkeypatch.setattr(
        "devcouncil.app.config.load_config",
        lambda root: SimpleNamespace(execution=SimpleNamespace(verify_on_post_task=True)),
    )
    monkeypatch.setattr(hook_cmd, "_verify_active_task", lambda root: "[green]TASK-1 verified.[/green]")
    post_task_verify = runner.invoke(app, ["hook", "post-task", "--project-root", str(tmp_path)])
    assert "TASK-1 verified" in post_task_verify.output


