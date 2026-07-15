"""Batch B — `dev go` closes the loop with bounded, accountable self-repair."""

import json
from types import SimpleNamespace

import devcouncil.cli.commands.go as go
import devcouncil.cli.commands.run as run_command
from devcouncil.domain.gap import Gap
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.storage.db import Database
from devcouncil.storage.native import CorrectionManifestRepository
from devcouncil.storage.repositories import GapRepository, TaskRepository


def _task():
    return SimpleNamespace(id="TASK-001")


def _wire(monkeypatch, tmp_path, *, statuses, signatures):
    calls = []
    monkeypatch.setattr(run_command, "run", lambda task_id, **k: calls.append(task_id))
    status_iter = iter(statuses)
    monkeypatch.setattr(go, "_task_status", lambda root, tid: next(status_iter))
    sig_iter = iter(signatures)
    monkeypatch.setattr(go, "_blocking_gap_signature", lambda root, tid: next(sig_iter))
    monkeypatch.setattr(go, "_remediable_incomplete_signature", lambda root, tid: "")
    monkeypatch.setattr(go, "_commit_task_changes", lambda root, tid, status: False)
    monkeypatch.setattr(
        "devcouncil.planning.correction_manifest.write_correction_manifest",
        lambda root, tid, **k: tmp_path / "m.json",
    )
    return calls


def test_repair_loop_stops_when_verified(monkeypatch, tmp_path):
    calls = _wire(monkeypatch, tmp_path, statuses=["blocked", "verified"], signatures=["sigA"])
    status, attempts = go._execute_task_with_repair(
        tmp_path, _task(), executor="codex", profile=None, stream=False, max_repairs=3, repair_service=None
    )
    assert status == "verified"
    assert attempts == 1
    assert len(calls) == 2  # initial run + one repair


def test_repair_loop_respects_budget(monkeypatch, tmp_path):
    calls = _wire(
        monkeypatch, tmp_path,
        statuses=["blocked", "blocked", "blocked"],
        signatures=["sigA", "sigB"],  # always different -> no-progress never triggers
    )
    status, attempts = go._execute_task_with_repair(
        tmp_path, _task(), executor="codex", profile=None, stream=False, max_repairs=2, repair_service=None
    )
    assert status == "blocked"
    assert attempts == 2  # bounded by budget
    assert len(calls) == 3  # initial + 2 repairs


def test_repair_loop_stops_on_no_progress(monkeypatch, tmp_path):
    calls = _wire(
        monkeypatch, tmp_path,
        statuses=["blocked", "blocked"],
        signatures=["same", "same"],  # identical blocking gaps -> stalled
    )
    status, attempts = go._execute_task_with_repair(
        tmp_path, _task(), executor="codex", profile=None, stream=False, max_repairs=5, repair_service=None
    )
    assert status == "blocked"
    assert attempts == 1  # gave up after the gaps reappeared unchanged
    assert len(calls) == 2


def test_repair_loop_continues_when_executor_failed_with_same_gaps(monkeypatch, tmp_path):
    calls = _wire(
        monkeypatch, tmp_path,
        statuses=["blocked", "blocked", "blocked"],
        signatures=["same", "same", "same"],
    )
    monkeypatch.setattr(go, "_executor_run_failed", lambda root, tid: True)
    status, attempts = go._execute_task_with_repair(
        tmp_path, _task(), executor="codex", profile=None, stream=False, max_repairs=2, repair_service=None
    )
    assert status == "blocked"
    assert attempts == 2
    assert len(calls) == 3


def test_manual_executor_does_not_repair(monkeypatch, tmp_path):
    calls = _wire(monkeypatch, tmp_path, statuses=["blocked"], signatures=[])
    status, attempts = go._execute_task_with_repair(
        tmp_path, _task(), executor="manual", profile=None, stream=False, max_repairs=0, repair_service=None
    )
    assert status == "blocked"
    assert attempts == 0
    assert len(calls) == 1  # one run, no repair


def _seed_blocked_task(tmp_path):
    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / ".devcouncil" / "config.yaml").write_text(
        "project:\n  name: test\nexecution:\n  max_repair_attempts: 3\n", encoding="utf-8"
    )
    db = Database(tmp_path / ".devcouncil" / "state.sqlite")
    db.create_db_and_tables()
    with db.get_session() as session:
        TaskRepository(session).save(Task(
            id="TASK-001", title="T", description="D",
            planned_files=[PlannedFile(path="src/a.py", reason="x", allowed_change="modify")],
            expected_tests=["pytest tests/"],
        ))
        GapRepository(session).save(Gap(
            id="GAP-1", severity="high", gap_type="test_failed", task_id="TASK-001",
            description="tests failed", recommended_fix="fix", blocking=True,
        ))


def test_correction_manifest_attempt_increments(tmp_path):
    from devcouncil.planning.correction_manifest import load_latest_correction_manifest, write_correction_manifest

    _seed_blocked_task(tmp_path)

    write_correction_manifest(tmp_path, "TASK-001")
    first = load_latest_correction_manifest(tmp_path, "TASK-001")
    assert first.prior_failed_attempts == 1

    write_correction_manifest(tmp_path, "TASK-001")
    second = load_latest_correction_manifest(tmp_path, "TASK-001")
    assert second.prior_failed_attempts == 2
    assert len(second.attempt_history) == 1
    assert "attempt 1" in second.attempt_history[0]

    db = Database(tmp_path / ".devcouncil" / "state.sqlite")
    with db.get_session() as session:
        record = CorrectionManifestRepository(session).latest_for_task("TASK-001")
        assert record.attempt == 2


def test_task_max_repairs_widens_for_hard_tasks(tmp_path):
    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / ".devcouncil" / "config.yaml").write_text(
        "project:\n  name: test\nexecution:\n  max_repair_attempts: 3\n"
        "verification:\n  rigor:\n    extra_repair_attempts_on_hard: 2\n",
        encoding="utf-8",
    )
    easy = Task(id="TASK-E", title="E", description="easy", difficulty="easy")
    hard = Task(id="TASK-H", title="H", description="hard", difficulty="hard")
    assert go._task_max_repairs(tmp_path, easy, 3) == 3
    assert go._task_max_repairs(tmp_path, hard, 3) == 5


def test_repair_loop_clears_incomplete_via_verify_only(monkeypatch, tmp_path):
    calls = []
    verify_calls = []
    incomplete = ["incomplete_sig"]

    monkeypatch.setattr(run_command, "run", lambda task_id, **k: calls.append(task_id))
    monkeypatch.setattr(go, "_task_status", lambda root, tid: "verified")
    monkeypatch.setattr(go, "_blocking_gap_signature", lambda root, tid: "")

    def reverify(root, tid):
        verify_calls.append(tid)
        incomplete[0] = ""
        return "verified"

    monkeypatch.setattr(go, "_reverify_task", reverify)
    monkeypatch.setattr(go, "_remediable_incomplete_signature", lambda root, tid: incomplete[0])
    monkeypatch.setattr(go, "_commit_task_changes", lambda root, tid, status: False)
    monkeypatch.setattr(
        "devcouncil.planning.correction_manifest.write_correction_manifest",
        lambda root, tid, **k: tmp_path / "m.json",
    )

    status, attempts = go._execute_task_with_repair(
        tmp_path, _task(), executor="codex", profile=None, stream=False, max_repairs=3, repair_service=None
    )
    assert status == "verified"
    assert attempts == 0
    assert len(calls) == 1
    assert verify_calls == ["TASK-001"]


def test_repair_skips_executor_when_unavailable_and_incomplete(monkeypatch, tmp_path):
    calls = []
    monkeypatch.setattr(run_command, "run", lambda task_id, **k: calls.append(task_id))
    monkeypatch.setattr(go, "_task_status", lambda root, tid: "verified")
    monkeypatch.setattr(go, "_blocking_gap_signature", lambda root, tid: "")
    monkeypatch.setattr(go, "_remediable_incomplete_signature", lambda root, tid: "still_incomplete")
    monkeypatch.setattr(go, "_reverify_task", lambda root, tid: "verified")
    monkeypatch.setattr(go, "_executor_run_unavailable", lambda root, tid: True)
    monkeypatch.setattr(go, "_commit_task_changes", lambda root, tid, status: False)
    monkeypatch.setattr(
        "devcouncil.planning.correction_manifest.write_correction_manifest",
        lambda root, tid, **k: tmp_path / "m.json",
    )

    status, attempts = go._execute_task_with_repair(
        tmp_path, _task(), executor="codex", profile=None, stream=False, max_repairs=3, repair_service=None
    )
    assert status == "verified"
    assert attempts == 0
    assert len(calls) == 1


def test_repair_stops_when_blocked_and_unavailable(monkeypatch, tmp_path):
    """P0: advisor/infra unavailability must not burn repair budget while blocked."""
    calls = []
    monkeypatch.setattr(run_command, "run", lambda task_id, **k: calls.append(task_id))
    monkeypatch.setattr(go, "_task_status", lambda root, tid: "blocked")
    monkeypatch.setattr(go, "_blocking_gap_signature", lambda root, tid: "gapA")
    monkeypatch.setattr(go, "_remediable_incomplete_signature", lambda root, tid: "")
    monkeypatch.setattr(go, "_reverify_task", lambda root, tid: "blocked")
    monkeypatch.setattr(go, "_executor_run_unavailable", lambda root, tid: True)
    monkeypatch.setattr(go, "_executor_run_failed", lambda root, tid: True)
    monkeypatch.setattr(go, "_commit_task_changes", lambda root, tid, status: False)
    monkeypatch.setattr(
        "devcouncil.planning.correction_manifest.write_correction_manifest",
        lambda root, tid, **k: tmp_path / "m.json",
    )

    status, attempts = go._execute_task_with_repair(
        tmp_path, _task(), executor="claude", profile=None, stream=False, max_repairs=3, repair_service=None
    )
    assert status == "blocked"
    assert attempts == 0
    assert len(calls) == 1


def test_executor_run_unavailable_detects_advisor_markers(tmp_path):
    runs = tmp_path / ".devcouncil" / "runs" / "run-adv"
    runs.mkdir(parents=True)
    (runs / "agent-run.json").write_text(
        json.dumps({
            "task_id": "TASK-001",
            "returncode": 1,
            "stderr_preview": ["Error: model does not support the advisor tool"],
        }),
        encoding="utf-8",
    )
    assert go._executor_run_unavailable(tmp_path, "TASK-001") is True


def test_executor_run_unavailable_detects_session_limit(tmp_path):
    runs = tmp_path / ".devcouncil" / "runs" / "run-1"
    runs.mkdir(parents=True)
    (runs / "agent-run.json").write_text(
        json.dumps({
            "task_id": "TASK-001",
            "returncode": 1,
            "stderr_preview": ["You've hit your session limit"],
        }),
        encoding="utf-8",
    )
    assert go._executor_run_unavailable(tmp_path, "TASK-001") is True
    assert go._executor_run_unavailable(tmp_path, "OTHER") is False


def test_executor_exception_does_not_crash_run(monkeypatch, tmp_path):
    # An executor that raises must be caught: the task ends non-verified and the run
    # continues, rather than the exception aborting the whole `dev go`.
    def boom(task_id, **k):
        raise RuntimeError("native agent StructuredOutputError")
    monkeypatch.setattr(run_command, "run", boom)
    monkeypatch.setattr(go, "_task_status", lambda root, tid: "blocked")
    monkeypatch.setattr(go, "_blocking_gap_signature", lambda root, tid: "")  # no concrete gaps -> stop
    monkeypatch.setattr(go, "_commit_task_changes", lambda root, tid, status: False)

    status, attempts = go._execute_task_with_repair(
        tmp_path, _task(), executor="native-preview", profile=None, stream=False,
        max_repairs=3, repair_service=None,
    )
    assert status == "blocked"
    assert attempts == 0  # crashed executor produced no concrete gaps -> no repair spin
