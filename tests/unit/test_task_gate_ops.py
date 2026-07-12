"""Unit tests for ``execution/task_gate_ops.py`` shared task-gate operations.

Exercises the ops functions directly (the CLI/MCP surfaces are thin wrappers),
mocking DB/LLM/subprocess side effects. Covers every branch: lease errors,
not-initialized, task-not-found, policy deny/allow, evidence read/truncate,
next-task selection, run-command success/failure/timeout, and handoff.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from devcouncil.domain.evidence import (
    CommandResult,
    DiffCoverageEvidence,
    DiffEvidence,
    TestEvidence,
)
from devcouncil.domain.gap import Gap
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.execution import task_gate_ops as ops
from devcouncil.storage.db import Database, reset_db_cache
from devcouncil.storage.native import ShellCommandRepository, TaskLeaseRepository
from devcouncil.storage.repositories import EvidenceRepository, TaskRepository


def _setup(tmp_path: Path, *, task: Task | None = None, lease: bool = True):
    """Seed a project DB with a task (+ optional active lease). Returns lease token."""
    reset_db_cache()
    dev = tmp_path / ".devcouncil"
    dev.mkdir(exist_ok=True)
    db = Database(dev / "state.sqlite")
    db.create_db_and_tables()
    task = task or Task(
        id="TASK-1",
        title="t",
        description="d",
        planned_files=[PlannedFile(path="src/a.py", reason="edit", allowed_change="modify")],
    )
    token = None
    with db.get_session() as session:
        TaskRepository(session).save(task)
        if lease:
            rec = TaskLeaseRepository(session).acquire(
                task.id, owner="test", agent="test", ttl_seconds=600,
            )
            token = rec.lease_token
    return db, token


# --------------------------------------------------------------------------- #
# not_initialized branch across every op                                       #
# --------------------------------------------------------------------------- #

def test_not_initialized_branches(tmp_path, monkeypatch):
    monkeypatch.setattr(ops, "_db", lambda root: None)
    root = tmp_path
    assert ops.verify_task_payload(root, task_id="T", lease_token="x")["code"] == "not_initialized"
    assert ops.update_task_scope_payload(root, task_id="T", lease_token="x")["code"] == "not_initialized"
    assert ops.append_evidence_payload(root, task_id="T", lease_token="x", command="c", summary="s")["code"] == "not_initialized"
    assert ops.get_evidence_payload(root, task_id="T")["code"] == "not_initialized"
    assert ops.policy_check_write_payload(root, path="a.py")["code"] == "not_initialized"
    assert ops.record_command_payload(root, task_id="T", lease_token="x", command="c", status="started")["code"] == "not_initialized"
    assert ops.next_task_payload(root)["code"] == "not_initialized"
    assert ops.run_command_payload(root, task_id="T", lease_token="x", command="echo hi")["code"] == "not_initialized"
    assert ops.handoff_agent_payload(root, task_id="T", lease_token="x", from_agent="a", to_agent="b")["code"] == "not_initialized"


# --------------------------------------------------------------------------- #
# _build_router                                                                #
# --------------------------------------------------------------------------- #

def test_build_router_success(tmp_path, monkeypatch):
    """Cover the happy path where config + provider + router all construct."""
    import types

    class _Role:
        def model_dump(self):
            return {"model": "m"}

    class _Cfg:
        class models:
            provider = "anthropic"
            roles = {"executor": _Role()}
        provider = {}

    monkeypatch.setattr("devcouncil.app.config.load_config", lambda root: _Cfg())
    monkeypatch.setattr("devcouncil.llm.provider.validate_model_provider", lambda p: None)
    monkeypatch.setattr("devcouncil.app.config.get_api_key", lambda p, root: "key")
    monkeypatch.setattr("devcouncil.llm.provider.create_provider", lambda *a, **k: object())
    sentinel = object()
    monkeypatch.setattr("devcouncil.llm.router.ModelRouter", lambda *a, **k: sentinel)
    assert ops._build_router(tmp_path) is sentinel


def test_build_router_returns_none_on_failure(tmp_path, monkeypatch):
    monkeypatch.setattr("devcouncil.app.config.load_config", lambda root: (_ for _ in ()).throw(RuntimeError("no cfg")))
    assert ops._build_router(tmp_path) is None


# --------------------------------------------------------------------------- #
# verify_task_payload                                                          #
# --------------------------------------------------------------------------- #

def test_verify_unsupported_sandbox(tmp_path):
    payload = ops.verify_task_payload(tmp_path, task_id="T", lease_token="x", sandbox="docker")
    assert payload["ok"] is False
    assert payload["code"] == "unsupported_sandbox"
    assert payload["sandbox"] == "docker"


def test_verify_invalid_lease(tmp_path):
    _setup(tmp_path)
    payload = ops.verify_task_payload(tmp_path, task_id="TASK-1", lease_token="wrong")
    assert payload["ok"] is False
    assert payload["code"] in {"invalid_lease", "lease_held_by_other"}


def test_verify_task_not_found(tmp_path, monkeypatch):
    # Lease exists on TASK-1, but simulate the task row vanishing after lease check.
    _db, token = _setup(tmp_path)
    monkeypatch.setattr(ops.TaskRepository, "get_by_id", lambda self, tid: None)
    payload = ops.verify_task_payload(tmp_path, task_id="TASK-1", lease_token=token)
    assert payload["ok"] is False
    assert payload["code"] == "not_found"


class _FakeOutcome:
    mode = "compiled"
    compiler_active = True
    diff_empty = False
    coverage_measured = True
    coverage_skipped_reason = None
    difficulty = "normal"
    rigor_applied = ["stub_gate"]


def _install_fake_verifier(monkeypatch, gaps, evidence, *, outcome=_FakeOutcome()):
    class _FakeVerifier:
        def __init__(self, project_root, router=None):
            self.last_outcome = outcome

        async def verify_task(self, task, requirements):
            return list(gaps), list(evidence)

    import devcouncil.verification.verifier as verifier_mod

    monkeypatch.setattr(verifier_mod, "Verifier", _FakeVerifier)


def test_verify_success_verified_status(tmp_path, monkeypatch):
    _db2, token = _setup(tmp_path)
    evidence = [
        CommandResult(command="c", exit_code=0, stdout_path="", stderr_path="", summary="s"),
        DiffEvidence(task_id="TASK-1", changed_files=["src/a.py"], added_files=[], deleted_files=[], diff_summary="x"),
        DiffCoverageEvidence(task_id="TASK-1", measured=True, changed_lines=1, covered_lines=1),
        TestEvidence(requirement_id="R", acceptance_criterion_id="AC", command="c", status="passed", evidence_summary="ok"),
    ]
    _install_fake_verifier(monkeypatch, gaps=[], evidence=evidence)
    payload = ops.verify_task_payload(tmp_path, task_id="TASK-1", lease_token=token)
    assert payload["ok"] is True
    assert payload["status"] == "verified"
    assert payload["passed"] is True
    assert payload["verification_mode"] == "compiled"
    assert payload["rigor_applied"] == ["stub_gate"]


def test_verify_blocked_status_with_gap(tmp_path, monkeypatch):
    _db2, token = _setup(tmp_path)
    gap = Gap(
        id="g1", severity="high", gap_type="test_failed", task_id="TASK-1",
        description="d", recommended_fix="f", blocking=True,
    )
    _install_fake_verifier(monkeypatch, gaps=[gap], evidence=[], outcome=None)
    payload = ops.verify_task_payload(tmp_path, task_id="TASK-1", lease_token=token)
    assert payload["ok"] is True
    assert payload["status"] == "blocked"
    assert payload["passed"] is False
    assert payload["blocking_gaps"]
    assert payload["verification_mode"] == "unknown"


# --------------------------------------------------------------------------- #
# update_task_scope_payload                                                    #
# --------------------------------------------------------------------------- #

def test_scope_invalid_lease(tmp_path):
    _setup(tmp_path)
    payload = ops.update_task_scope_payload(tmp_path, task_id="TASK-1", lease_token="bad")
    assert payload["ok"] is False


def test_scope_task_not_found(tmp_path, monkeypatch):
    _db, token = _setup(tmp_path)
    monkeypatch.setattr(ops.TaskRepository, "get_by_id", lambda self, tid: None)
    payload = ops.update_task_scope_payload(tmp_path, task_id="TASK-1", lease_token=token)
    assert payload["code"] == "not_found"


def test_scope_accepts_and_rejects_commands_and_tests(tmp_path):
    _db, token = _setup(tmp_path)
    payload = ops.update_task_scope_payload(
        tmp_path,
        task_id="TASK-1",
        lease_token=token,
        allowed_commands=["python -m pytest tests/test_x.py -q", "echo hi"],
        expected_tests=[
            'python -c "import a; assert a.x == 1"',  # acceptance-capable
            "python --version",  # trivial -> rejected
        ],
    )
    assert payload["ok"] is True
    # At least one command accepted and one trivial-ish rejected path exercised.
    assert "python -m pytest tests/test_x.py -q" in payload["allowed_commands"]
    assert isinstance(payload["rejected_allowed_commands"], list)
    assert isinstance(payload["rejected_expected_tests"], list)


def test_scope_dedupes_existing_command(tmp_path):
    task = Task(id="TASK-1", title="t", description="d", allowed_commands=["pytest"])
    _db, token = _setup(tmp_path, task=task)
    payload = ops.update_task_scope_payload(
        tmp_path, task_id="TASK-1", lease_token=token, allowed_commands=["pytest"],
    )
    assert payload["allowed_commands"].count("pytest") == 1


def test_scope_dedupes_existing_expected_test(tmp_path):
    test_cmd = 'python -c "import a; assert a.x == 1"'
    task = Task(id="TASK-1", title="t", description="d", expected_tests=[test_cmd])
    _db, token = _setup(tmp_path, task=task)
    payload = ops.update_task_scope_payload(
        tmp_path, task_id="TASK-1", lease_token=token, expected_tests=[test_cmd],
    )
    assert payload["expected_tests"].count(test_cmd) == 1
    assert payload["rejected_expected_tests"] == []


def test_scope_appends_planned_file(tmp_path, monkeypatch):
    _db, token = _setup(tmp_path)
    monkeypatch.chdir(tmp_path)
    caller = tmp_path / "src" / "caller.py"
    caller.parent.mkdir(parents=True, exist_ok=True)
    caller.write_text("x = 1\n", encoding="utf-8")
    payload = ops.update_task_scope_payload(
        tmp_path,
        task_id="TASK-1",
        lease_token=token,
        # "src/a.py" is already planned -> skipped via the existing-path continue.
        planned_files=["./src/caller.py", "src/ghost.py", "src/a.py"],
    )
    assert payload["ok"] is True
    assert "src/caller.py" in payload["agent_appended_planned_files"]
    assert "src/ghost.py" in payload["rejected_planned_files"]


def test_scope_rejects_secret_and_restricted_planned_files(tmp_path, monkeypatch):
    _db, token = _setup(tmp_path)
    monkeypatch.chdir(tmp_path)
    payload = ops.update_task_scope_payload(
        tmp_path,
        task_id="TASK-1",
        lease_token=token,
        planned_files=[".env", ".git/config"],
    )
    assert payload["ok"] is True
    assert set(payload["rejected_planned_files"]) == {".env", ".git/config"}
    assert payload["agent_appended_planned_files"] == []


# --------------------------------------------------------------------------- #
# append_evidence_payload / get_evidence_payload                              #
# --------------------------------------------------------------------------- #

def test_append_and_list_evidence(tmp_path):
    _db, token = _setup(tmp_path)
    appended = ops.append_evidence_payload(
        tmp_path, task_id="TASK-1", lease_token=token, command="pytest -q", summary="passed", exit_code=0,
    )
    assert appended["ok"] is True
    listed = ops.get_evidence_payload(tmp_path, task_id="TASK-1")
    assert listed["ok"] is True
    assert any(r["command"] == "pytest -q" for r in listed["evidence"])


def test_append_evidence_invalid_lease(tmp_path):
    _setup(tmp_path)
    payload = ops.append_evidence_payload(
        tmp_path, task_id="TASK-1", lease_token="bad", command="c", summary="s",
    )
    assert payload["ok"] is False


def test_get_evidence_filter_limit_and_logs(tmp_path):
    _db, token = _setup(tmp_path)
    # Persist evidence rows with real log files so read_log_file/truncate is exercised.
    stdout_file = tmp_path / "out.log"
    stdout_file.write_text("x" * 30_000, encoding="utf-8")  # forces truncation
    reset_db_cache()
    db = Database(tmp_path / ".devcouncil" / "state.sqlite")
    with db.get_session() as session:
        repo = EvidenceRepository(session)
        repo.save_command_result("TASK-1", CommandResult(
            command="pytest big", exit_code=0, stdout_path=str(stdout_file), stderr_path="", summary="s",
        ))
        repo.save_command_result("TASK-1", CommandResult(
            command="ruff check", exit_code=1, stdout_path="", stderr_path="", summary="s",
        ))
    filtered = ops.get_evidence_payload(tmp_path, task_id="TASK-1", command_filter="pytest", limit=1)
    assert filtered["ok"] is True
    assert len(filtered["evidence"]) == 1
    assert filtered["evidence"][0]["truncated"] is True
    # A filter that skips a non-matching row (no early break) exercises the continue.
    only_ruff = ops.get_evidence_payload(tmp_path, task_id="TASK-1", command_filter="ruff")
    assert len(only_ruff["evidence"]) == 1
    assert only_ruff["evidence"][0]["command"] == "ruff check"
    # No filter returns all rows.
    all_rows = ops.get_evidence_payload(tmp_path, task_id="TASK-1")
    assert len(all_rows["evidence"]) >= 2


# --------------------------------------------------------------------------- #
# policy_check_write_payload                                                   #
# --------------------------------------------------------------------------- #

def test_policy_check_with_task_id_allows_planned(tmp_path):
    _setup(tmp_path)
    payload = ops.policy_check_write_payload(tmp_path, path="src/a.py", task_id="TASK-1")
    assert payload["action"] in {"allow", "warn"}
    assert payload["allowed"] is True
    assert payload["task_id"] == "TASK-1"


def test_policy_check_without_task_uses_running_task(tmp_path):
    task = Task(id="TASK-1", title="t", description="d", status="running",
                planned_files=[PlannedFile(path="src/a.py", reason="e", allowed_change="modify")])
    _setup(tmp_path, task=task, lease=False)
    payload = ops.policy_check_write_payload(tmp_path, path="src/a.py")
    assert payload["task_id"] == "TASK-1"
    assert payload["allowed"] is True


def test_policy_check_no_running_task_denies(tmp_path):
    _setup(tmp_path, lease=False)  # planned task is status=planned, not running
    payload = ops.policy_check_write_payload(tmp_path, path="src/a.py")
    assert payload["task_id"] is None
    assert payload["allowed"] is False


# --------------------------------------------------------------------------- #
# record_command_payload                                                       #
# --------------------------------------------------------------------------- #

def test_record_command_invalid_status(tmp_path):
    payload = ops.record_command_payload(
        tmp_path, task_id="TASK-1", lease_token="x", command="c", status="bogus",
    )
    assert payload["ok"] is False
    assert payload["code"] == "invalid_arguments"


def test_record_command_success(tmp_path):
    _db, token = _setup(tmp_path)
    payload = ops.record_command_payload(
        tmp_path, task_id="TASK-1", lease_token=token, command="pytest", status="finished", exit_code=0,
    )
    assert payload["ok"] is True
    assert payload["recorded"] is True


def test_record_command_invalid_lease(tmp_path):
    _setup(tmp_path)
    payload = ops.record_command_payload(
        tmp_path, task_id="TASK-1", lease_token="bad", command="c", status="started",
    )
    assert payload["ok"] is False


# --------------------------------------------------------------------------- #
# next_task_payload                                                            #
# --------------------------------------------------------------------------- #

def test_next_task_none_available(tmp_path):
    # Only task is planned but leased -> excluded.
    _setup(tmp_path)
    payload = ops.next_task_payload(tmp_path)
    assert payload["ok"] is True
    assert payload["task"] is None
    assert "No unblocked" in payload["reason"]


def test_next_task_selects_unleased_and_orders_by_deps(tmp_path, monkeypatch):
    # depends_on is not persisted by the repository, so drive get_all with in-memory
    # tasks to deterministically exercise dependency ordering and unmet-dep skipping.
    _setup(tmp_path, lease=False)
    tasks = [
        Task(id="TASK-DONE", title="d", description="d", status="verified"),
        Task(id="TASK-DEP", title="dep", description="d", status="planned", depends_on=["TASK-DONE"]),
        Task(id="TASK-FREE", title="free", description="d", status="planned"),
        Task(id="TASK-BLOCKED", title="blk", description="d", status="planned", depends_on=["TASK-MISS"]),
    ]
    monkeypatch.setattr(ops.TaskRepository, "get_all", lambda self: tasks)
    payload = ops.next_task_payload(tmp_path)
    assert payload["ok"] is True
    # TASK-FREE has 0 deps -> sorts ahead of the 1-dep TASK-DEP; TASK-BLOCKED is skipped.
    assert payload["task"]["id"] == "TASK-FREE"
    assert payload["ready_to_checkout"] is True


def test_next_task_counts_blocking_gaps(tmp_path):
    from devcouncil.storage.repositories import GapRepository

    reset_db_cache()
    dev = tmp_path / ".devcouncil"
    dev.mkdir()
    db = Database(dev / "state.sqlite")
    db.create_db_and_tables()
    with db.get_session() as session:
        TaskRepository(session).save(Task(id="TASK-B", title="b", description="d", status="planned"))
        GapRepository(session).save(Gap(
            id="g1", severity="high", gap_type="test_failed", task_id="TASK-B",
            description="d", recommended_fix="f", blocking=True,
        ))
    payload = ops.next_task_payload(tmp_path)
    assert payload["task"]["id"] == "TASK-B"
    assert payload["blocking_gap_count"] == 1
    assert payload["ready_to_checkout"] is False


def test_next_task_status_filter(tmp_path):
    reset_db_cache()
    dev = tmp_path / ".devcouncil"
    dev.mkdir()
    db = Database(dev / "state.sqlite")
    db.create_db_and_tables()
    with db.get_session() as session:
        TaskRepository(session).save(Task(id="TASK-R", title="r", description="d", status="ready"))
    payload = ops.next_task_payload(tmp_path, status_filter="ready", client_id="c1")
    assert payload["task"]["id"] == "TASK-R"


# --------------------------------------------------------------------------- #
# run_command_payload                                                          #
# --------------------------------------------------------------------------- #

def test_run_command_denied_not_in_allowlist(tmp_path):
    _db, token = _setup(tmp_path)  # task has no allowed_commands
    payload = ops.run_command_payload(tmp_path, task_id="TASK-1", lease_token=token, command="rm -rf /")
    assert payload["ok"] is False
    assert payload["code"] == "command_not_allowed"


def test_run_command_success(tmp_path):
    import sys
    cmd = f'{sys.executable} -c "print(1)"'
    task = Task(id="TASK-1", title="t", description="d", allowed_commands=[cmd])
    _db, token = _setup(tmp_path, task=task)
    payload = ops.run_command_payload(tmp_path, task_id="TASK-1", lease_token=token, command=cmd)
    assert payload["ok"] is True
    assert payload["exit_code"] == 0
    assert "1" in payload["stdout"]


def test_run_command_nonzero_exit(tmp_path):
    import sys
    cmd = f'{sys.executable} -c "import sys; sys.exit(3)"'
    task = Task(id="TASK-1", title="t", description="d", allowed_commands=[cmd])
    _db, token = _setup(tmp_path, task=task)
    payload = ops.run_command_payload(tmp_path, task_id="TASK-1", lease_token=token, command=cmd)
    assert payload["ok"] is False
    assert payload["exit_code"] == 3


def test_run_command_file_not_found(tmp_path):
    cmd = "definitely_not_a_real_binary_xyz --do-thing"
    task = Task(id="TASK-1", title="t", description="d", allowed_commands=[cmd])
    _db, token = _setup(tmp_path, task=task)
    payload = ops.run_command_payload(tmp_path, task_id="TASK-1", lease_token=token, command=cmd)
    assert payload["ok"] is False
    assert payload["code"] == "run_failed"


def test_run_command_timeout(tmp_path, monkeypatch):
    import subprocess
    cmd = "sleep 999"
    task = Task(id="TASK-1", title="t", description="d", allowed_commands=[cmd])
    _db, token = _setup(tmp_path, task=task)

    def _raise_timeout(*args, **kwargs):
        raise subprocess.TimeoutExpired(cmd="sleep", timeout=1, output="partial", stderr="e")

    monkeypatch.setattr(ops.subprocess, "run", _raise_timeout)
    payload = ops.run_command_payload(tmp_path, task_id="TASK-1", lease_token=token, command=cmd)
    assert payload["timed_out"] is True
    assert payload["exit_code"] is None


def test_run_command_invalid_lease(tmp_path):
    _setup(tmp_path)
    payload = ops.run_command_payload(tmp_path, task_id="TASK-1", lease_token="bad", command="echo hi")
    assert payload["ok"] is False


def test_run_command_task_not_found(tmp_path, monkeypatch):
    _db, token = _setup(tmp_path)
    monkeypatch.setattr(ops.TaskRepository, "get_by_id", lambda self, tid: None)
    payload = ops.run_command_payload(tmp_path, task_id="TASK-1", lease_token=token, command="echo hi")
    assert payload["code"] == "not_found"


# --------------------------------------------------------------------------- #
# handoff_agent_payload                                                        #
# --------------------------------------------------------------------------- #

def test_handoff_success(tmp_path, monkeypatch):
    _db, token = _setup(tmp_path)

    class _FakeManifest:
        def model_dump(self):
            return {"task_id": "TASK-1"}

    class _FakeHandoffService:
        def __init__(self, root):
            pass

        def create(self, task_id, from_agent, to_agent, *, instruction=""):
            return _FakeManifest(), tmp_path / "handoff.json", "run-123"

    import devcouncil.execution.handoff as handoff_mod
    monkeypatch.setattr(handoff_mod, "HandoffService", _FakeHandoffService)
    payload = ops.handoff_agent_payload(
        tmp_path, task_id="TASK-1", lease_token=token, from_agent="a", to_agent="b", instruction="go",
    )
    assert payload["ok"] is True
    assert payload["run_id"] == "run-123"


def test_handoff_value_error(tmp_path, monkeypatch):
    _db, token = _setup(tmp_path)

    class _BoomService:
        def __init__(self, root):
            pass

        def create(self, *args, **kwargs):
            raise ValueError("no such task")

    import devcouncil.execution.handoff as handoff_mod
    monkeypatch.setattr(handoff_mod, "HandoffService", _BoomService)
    payload = ops.handoff_agent_payload(
        tmp_path, task_id="TASK-1", lease_token=token, from_agent="a", to_agent="b",
    )
    assert payload["ok"] is False
    assert payload["code"] == "handoff_failed"


def test_handoff_invalid_lease(tmp_path):
    _setup(tmp_path)
    payload = ops.handoff_agent_payload(
        tmp_path, task_id="TASK-1", lease_token="bad", from_agent="a", to_agent="b",
    )
    assert payload["ok"] is False
