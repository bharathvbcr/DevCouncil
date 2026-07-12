import json
from pathlib import Path
from types import SimpleNamespace

import pytest
from typer.testing import CliRunner

import devcouncil.cli.commands.verify as verify_cmd
from devcouncil.cli.main import app
from devcouncil.domain.task import Task
from devcouncil.domain.gap import Gap
from devcouncil.storage.db import Database, reset_db_cache
from devcouncil.storage.repositories import TaskRepository
from devcouncil.verification.verifier import Verifier

runner = CliRunner()


def _setup_verify_db(tmp_path: Path, monkeypatch) -> tuple[Path, str]:
    reset_db_cache()
    monkeypatch.chdir(tmp_path)
    runner.invoke(app, ["init"])
    
    db = Database(tmp_path / ".devcouncil" / "state.sqlite")
    task = Task(
        id="TASK-1",
        title="Verify Task",
        description="d",
        status="planned",
    )
    with db.get_session() as session:
        TaskRepository(session).save(task)
    return tmp_path, "TASK-1"


def test_cli_verify_success(tmp_path, monkeypatch):
    _setup_verify_db(tmp_path, monkeypatch)
    
    # Mock verify_task to return zero gaps and some mock evidence
    async def fake_verify_task(self, task, reqs):
        from devcouncil.domain.evidence import CommandResult
        res = CommandResult(command="pytest", exit_code=0, stdout_path="", stderr_path="", summary="ok")
        return [], [res]
        
    monkeypatch.setattr(Verifier, "verify_task", fake_verify_task)
    
    res = runner.invoke(app, ["verify", "TASK-1"])
    assert res.exit_code == 0
    assert "verified" in res.output.lower()


def test_cli_verify_with_gaps(tmp_path, monkeypatch):
    _setup_verify_db(tmp_path, monkeypatch)
    
    # Mock verify_task to return a blocking gap
    async def fake_verify_task(self, task, reqs):
        gap = Gap(
            id="GAP-1",
            severity="high",
            gap_type="missing_test",
            description="Missing tests",
            blocking=True,
            recommended_fix="Write tests",
            task_id="TASK-1",
        )
        return [gap], []
        
    monkeypatch.setattr(Verifier, "verify_task", fake_verify_task)
    
    res = runner.invoke(app, ["verify", "TASK-1"])
    assert res.exit_code != 0
    assert "blocked" in res.output.lower() or "gap" in res.output.lower()


def test_cli_verify_json_format(tmp_path, monkeypatch):
    _setup_verify_db(tmp_path, monkeypatch)
    
    async def fake_verify_task(self, task, reqs):
        return [], []
        
    monkeypatch.setattr(Verifier, "verify_task", fake_verify_task)
    
    res = runner.invoke(app, ["verify", "TASK-1", "--json"])
    assert res.exit_code == 0
    
    # Strip any prefix warnings/log lines
    output = res.output
    if "{" in output:
        output = output[output.index("{"):]
    data = json.loads(output)
    assert data["ok"] is True


def _seed_task(root: Path, task_id: str, **kwargs) -> None:
    db = Database(root / ".devcouncil" / "state.sqlite")
    with db.get_session() as session:
        TaskRepository(session).save(
            Task(id=task_id, title=task_id, description="d", status="planned", **kwargs)
        )


# --- db unavailable ---------------------------------------------------------------


def test_verify_db_unavailable_human(tmp_path, monkeypatch):
    reset_db_cache()
    monkeypatch.chdir(tmp_path)
    runner.invoke(app, ["init"])
    monkeypatch.setattr(verify_cmd, "get_db", lambda root: None)

    res = runner.invoke(app, ["verify"])
    assert res.exit_code == 0
    assert "state is unavailable" in res.output


def test_verify_db_unavailable_json(tmp_path, monkeypatch):
    reset_db_cache()
    monkeypatch.chdir(tmp_path)
    runner.invoke(app, ["init"])
    monkeypatch.setattr(verify_cmd, "get_db", lambda root: None)

    res = runner.invoke(app, ["verify", "--json"])
    assert res.exit_code == 0
    output = res.output[res.output.index("{"):]
    assert json.loads(output)["ok"] is False


# --- no tasks found ---------------------------------------------------------------


def test_verify_task_not_found_human(tmp_path, monkeypatch):
    _setup_verify_db(tmp_path, monkeypatch)
    res = runner.invoke(app, ["verify", "NOPE"])
    assert res.exit_code == 0
    assert "not found" in res.output


def test_verify_no_tasks_json(tmp_path, monkeypatch):
    reset_db_cache()
    monkeypatch.chdir(tmp_path)
    runner.invoke(app, ["init"])
    res = runner.invoke(app, ["verify", "--json"])
    assert res.exit_code == 0
    output = res.output[res.output.index("{"):]
    assert json.loads(output)["ok"] is False


# --- sandbox (non-local) branches -------------------------------------------------


def _fake_sandbox(status, commands=None):
    result = SimpleNamespace(status=status, commands=commands or [])

    class _Sandbox:
        def run(self, task, cmds, reqs):
            return result

    def _get(sandbox, root):
        return _Sandbox()

    return _get


def test_verify_sandbox_unsupported(tmp_path, monkeypatch):
    _setup_verify_db(tmp_path, monkeypatch)
    import devcouncil.verification.sandbox as sandbox_mod
    monkeypatch.setattr(sandbox_mod, "get_sandbox", _fake_sandbox("unsupported"))

    res = runner.invoke(app, ["verify", "TASK-1", "--sandbox", "docker"])
    assert res.exit_code == 0
    assert "unavailable" in res.output


def test_verify_sandbox_failed(tmp_path, monkeypatch):
    _setup_verify_db(tmp_path, monkeypatch)
    import devcouncil.verification.sandbox as sandbox_mod
    monkeypatch.setattr(sandbox_mod, "get_sandbox", _fake_sandbox("failed"))

    res = runner.invoke(app, ["verify", "TASK-1", "--sandbox", "docker", "--json"])
    assert res.exit_code == 1
    assert "TASK-1" in res.output


def test_verify_sandbox_passed(tmp_path, monkeypatch):
    _setup_verify_db(tmp_path, monkeypatch)
    import devcouncil.verification.sandbox as sandbox_mod
    monkeypatch.setattr(sandbox_mod, "get_sandbox", _fake_sandbox("passed"))

    res = runner.invoke(app, ["verify", "TASK-1", "--sandbox", "nix"])
    assert res.exit_code == 0
    assert "passed in nix sandbox" in res.output


# --- evidence types + graph context -----------------------------------------------


def test_verify_saves_all_evidence_kinds(tmp_path, monkeypatch):
    _setup_verify_db(tmp_path, monkeypatch)

    # Force the code-review graph context to look available so the graph branch runs.
    class _Ctx:
        available = True

        def model_dump(self):
            return {"available": True}

    monkeypatch.setattr(
        verify_cmd, "CodeReviewGraphAdapter",
        lambda root: SimpleNamespace(get_context=lambda files: _Ctx()),
    )

    async def fake_verify_task(self, task, reqs):
        from devcouncil.domain.evidence import (
            CommandResult,
            DiffCoverageEvidence,
            DiffEvidence,
            TestEvidence,
        )
        evidence = [
            CommandResult(command="pytest", exit_code=0, stdout_path="", stderr_path="", summary="ok"),
            DiffCoverageEvidence(task_id=task.id, tool="coverage", measured=True),
            DiffEvidence(task_id=task.id, changed_files=["a.py"], added_files=[], deleted_files=[], diff_summary="s"),
            TestEvidence(
                requirement_id="R1", acceptance_criterion_id="AC1", command="pytest",
                status="passed", evidence_summary="ok",
            ),
        ]
        return [], evidence

    monkeypatch.setattr(Verifier, "verify_task", fake_verify_task)
    res = runner.invoke(app, ["verify", "TASK-1"])
    assert res.exit_code == 0
    assert "verified" in res.output.lower()


# --- non-blocking gaps + gap truncation -------------------------------------------


def test_verify_non_blocking_gaps(tmp_path, monkeypatch):
    _setup_verify_db(tmp_path, monkeypatch)

    async def fake_verify_task(self, task, reqs):
        gap = Gap(
            id="GAP-N", severity="low", gap_type="missing_test", description="nit",
            blocking=False, recommended_fix="fix", task_id="TASK-1",
        )
        return [gap], []

    monkeypatch.setattr(Verifier, "verify_task", fake_verify_task)
    res = runner.invoke(app, ["verify", "TASK-1"])
    assert res.exit_code == 0
    assert "non-blocking gaps" in res.output


def test_verify_many_gaps_truncates_render(tmp_path, monkeypatch):
    _setup_verify_db(tmp_path, monkeypatch)

    async def fake_verify_task(self, task, reqs):
        gaps = [
            Gap(
                id=f"GAP-{i}", severity="low", gap_type="missing_test", description=f"nit {i}",
                blocking=False, recommended_fix="fix", task_id="TASK-1",
            )
            for i in range(25)
        ]
        return gaps, []

    monkeypatch.setattr(Verifier, "verify_task", fake_verify_task)
    res = runner.invoke(app, ["verify", "TASK-1"])
    assert res.exit_code == 0
    assert "Showing first" in res.output


# --- cross-task acceptance reconciliation + multi-task summary --------------------


def test_verify_all_cross_task_reconciliation(tmp_path, monkeypatch):
    reset_db_cache()
    monkeypatch.chdir(tmp_path)
    runner.invoke(app, ["init"])
    _seed_task(tmp_path, "TASK-A")
    _seed_task(tmp_path, "TASK-B")

    async def fake_verify_task(self, task, reqs):
        from devcouncil.domain.evidence import TestEvidence
        if task.id == "TASK-A":
            # Proves AC1 via passing evidence.
            ev = TestEvidence(
                requirement_id="R1", acceptance_criterion_id="AC1", command="pytest",
                status="passed", evidence_summary="ok",
            )
            return [], [ev]
        # TASK-B is blocked only by an acceptance_criteria_unproven gap for AC1.
        gap = Gap(
            id="GAP-B", severity="high", gap_type="acceptance_criteria_unproven",
            description="AC1 unproven", blocking=True, recommended_fix="test it",
            task_id="TASK-B", acceptance_criterion_id="AC1",
        )
        return [gap], []

    monkeypatch.setattr(Verifier, "verify_task", fake_verify_task)
    res = runner.invoke(app, ["verify", "--json"])
    # Reconciliation clears TASK-B's only blocking gap → overall ok.
    output = res.output[res.output.index("{"):]
    data = json.loads(output)
    assert data["blocked_tasks"] == 0
    assert res.exit_code == 0


def test_verify_all_multi_task_blocked_summary(tmp_path, monkeypatch):
    reset_db_cache()
    monkeypatch.chdir(tmp_path)
    runner.invoke(app, ["init"])
    _seed_task(tmp_path, "TASK-A")
    _seed_task(tmp_path, "TASK-B")

    async def fake_verify_task(self, task, reqs):
        if task.id == "TASK-B":
            gap = Gap(
                id="GAP-B", severity="high", gap_type="missing_test",
                description="no test", blocking=True, recommended_fix="test",
                task_id="TASK-B",
            )
            return [gap], []
        return [], []

    monkeypatch.setattr(Verifier, "verify_task", fake_verify_task)
    res = runner.invoke(app, ["verify"])
    assert res.exit_code == 1
    assert "blocked" in res.output.lower()


def test_verify_all_multi_task_all_verified_summary(tmp_path, monkeypatch):
    reset_db_cache()
    monkeypatch.chdir(tmp_path)
    runner.invoke(app, ["init"])
    _seed_task(tmp_path, "TASK-A")
    _seed_task(tmp_path, "TASK-B")

    async def fake_verify_task(self, task, reqs):
        return [], []

    monkeypatch.setattr(Verifier, "verify_task", fake_verify_task)
    res = runner.invoke(app, ["verify"])
    assert res.exit_code == 0
    assert "Verified 2 tasks successfully" in res.output
