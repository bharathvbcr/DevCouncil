"""CLI + helper coverage for `dev run` — task lookup, readiness gate, manual and
unknown-executor branches, and the verification/router helpers.

Heavy dependencies (executors, checkpoints, the verifier) are mocked so the command
dispatch logic is exercised without invoking real coding agents or git."""

import devcouncil.cli.commands.run as run_cmd
import devcouncil.gating.policy as policy
from devcouncil.cli.main import app
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.gating.policy import GateResult
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import TaskRepository
from typer.testing import CliRunner

runner = CliRunner()


def _seed_task(root, task_id="TASK-001"):
    db = get_db(root)
    assert db is not None
    with db.get_session() as session:
        TaskRepository(session).save(
            Task(
                id=task_id,
                title="Task",
                description="desc",
                planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
                expected_tests=["pytest"],
                allowed_commands=["pytest"],
            )
        )


def _pass_gate(monkeypatch):
    monkeypatch.setattr(
        policy.GatePolicy, "check_task_ready",
        lambda self, task, root: GateResult(passed=True, gaps=[]),
    )


def test_run_db_unavailable(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    monkeypatch.setattr(run_cmd, "get_db", lambda root: None)

    result = runner.invoke(app, ["run", "TASK-001"])
    assert result.exit_code == 0
    assert "state is unavailable" in result.output


def test_run_task_not_found(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    result = runner.invoke(app, ["run", "NOPE"])
    assert result.exit_code == 0
    assert "Task NOPE not found" in result.output


def test_run_gate_blocks_execution(tmp_path, monkeypatch):
    from devcouncil.domain.gap import Gap

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    _seed_task(tmp_path)
    blocking = Gap(
        id="G1", severity="high", gap_type="planned_file_not_changed",
        description="Working tree is dirty.", recommended_fix="Commit first.", blocking=True,
    )
    monkeypatch.setattr(
        policy.GatePolicy, "check_task_ready",
        lambda self, task, root: GateResult(passed=False, gaps=[blocking]),
    )

    result = runner.invoke(app, ["run", "TASK-001"])
    assert result.exit_code == 0
    assert "not ready for execution" in result.output
    assert "BLOCKING" in result.output


def test_run_manual_marks_running(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    _seed_task(tmp_path)
    _pass_gate(monkeypatch)

    result = runner.invoke(app, ["run", "TASK-001", "--executor", "manual"])
    assert result.exit_code == 0
    assert "now marked as RUNNING" in result.output
    db = get_db(tmp_path)
    with db.get_session() as session:
        assert TaskRepository(session).get_by_id("TASK-001").status == "running"


def test_run_unknown_executor_lists_available(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    _seed_task(tmp_path)
    _pass_gate(monkeypatch)

    result = runner.invoke(app, ["run", "TASK-001", "--executor", "totally-bogus"])
    assert result.exit_code == 0
    assert "Unknown executor" in result.output
    assert "Available executors" in result.output


def test_run_claude_sdk_falls_back_to_error_without_cli(tmp_path, monkeypatch):
    import shutil

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    _seed_task(tmp_path)
    _pass_gate(monkeypatch)
    # claude_agent_sdk is not installed in the test env; ensure no `claude` CLI either.
    monkeypatch.setattr(shutil, "which", lambda name: None)

    result = runner.invoke(app, ["run", "TASK-001", "--executor", "claude-sdk"])
    assert result.exit_code == 0
    assert "Claude Agent SDK is not installed" in result.output


def test_run_non_coding_executor_warns_ignored_flags(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    _seed_task(tmp_path)
    _pass_gate(monkeypatch)

    result = runner.invoke(app, ["run", "TASK-001", "--executor", "manual", "--profile", "yolo"])
    assert result.exit_code == 0
    assert "only apply to coding CLI executors" in result.output


# --- helper coverage --------------------------------------------------------------


def test_build_verification_router_none_without_config(tmp_path):
    assert run_cmd._build_verification_router(tmp_path) is None


def test_log_exec_outcome_does_not_raise():
    run_cmd._log_exec_outcome("mini", "TASK-001", verified=True)
    run_cmd._log_exec_outcome("mini", "TASK-001", verified=False)


def test_run_live_review_disabled_returns_quietly(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    class _Cfg:
        class integrations:
            class live_review:
                enabled = False

    monkeypatch.setattr(run_cmd, "load_config", lambda root: _Cfg())
    # Should return without attempting to resolve a transcript.
    run_cmd._run_live_review_after_execution(tmp_path, "claude", "TASK-001")


# --- executor branches (mocked executors + verification helpers) ------------------


class _ExecResult:
    def __init__(self, success=True, message=""):
        self.success = success
        self.message = message


def _fake_executor(result):
    class _FakeExecutor:
        last_run_id = None
        last_transcript_path = None

        def __init__(self, *args, **kwargs):
            pass

        def run_task(self, task, reqs):
            return result

    return _FakeExecutor


def _stub_verification(monkeypatch, verified=True):
    monkeypatch.setattr(run_cmd, "_capture_after_patch", lambda *a, **k: None)
    monkeypatch.setattr(run_cmd, "_build_verification_router", lambda root: None)
    monkeypatch.setattr(run_cmd, "_verify_after_execution", lambda *a, **k: verified)
    monkeypatch.setattr(run_cmd, "_record_agent_verification", lambda *a, **k: None)
    monkeypatch.setattr(run_cmd, "_run_live_review_after_execution", lambda *a, **k: None)


def test_run_coding_cli_executor_verified(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    _seed_task(tmp_path)
    _pass_gate(monkeypatch)
    _stub_verification(monkeypatch, verified=True)
    monkeypatch.setattr(run_cmd, "CodingCliExecutor", _fake_executor(_ExecResult(success=True)))

    result = runner.invoke(app, ["run", "TASK-001", "--executor", "claude"])
    assert result.exit_code == 0
    assert "CLAUDE finished and task TASK-001 verified" in result.output


def test_run_coding_cli_executor_blocked(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    _seed_task(tmp_path)
    _pass_gate(monkeypatch)
    _stub_verification(monkeypatch, verified=False)
    monkeypatch.setattr(run_cmd, "CodingCliExecutor", _fake_executor(_ExecResult(success=True)))

    result = runner.invoke(app, ["run", "TASK-001", "--executor", "claude"])
    assert result.exit_code == 0
    assert "blocked by verification gaps" in result.output


def test_run_coding_cli_executor_failure_no_changes(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    _seed_task(tmp_path)
    _pass_gate(monkeypatch)
    _stub_verification(monkeypatch)
    monkeypatch.setattr(run_cmd, "_current_changed_files", lambda root: [])
    monkeypatch.setattr(
        run_cmd, "CodingCliExecutor", _fake_executor(_ExecResult(success=False, message="crashed"))
    )

    result = runner.invoke(app, ["run", "TASK-001", "--executor", "claude"])
    assert result.exit_code == 0
    assert "failed to start or execute" in result.output


def test_run_coding_cli_executor_failure_but_has_changes(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    _seed_task(tmp_path)
    _pass_gate(monkeypatch)
    _stub_verification(monkeypatch, verified=True)
    monkeypatch.setattr(run_cmd, "_current_changed_files", lambda root: ["src/app.py"])
    monkeypatch.setattr(
        run_cmd, "CodingCliExecutor", _fake_executor(_ExecResult(success=False, message="crashed"))
    )

    result = runner.invoke(app, ["run", "TASK-001", "--executor", "claude"])
    assert result.exit_code == 0
    # Rich soft-wraps the long line at the default width; normalize before matching.
    normalized = " ".join(result.output.split())
    assert "verifying TASK-001 against them anyway" in normalized


def test_run_mini_executor_verified(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    _seed_task(tmp_path)
    _pass_gate(monkeypatch)
    _stub_verification(monkeypatch, verified=True)
    monkeypatch.setattr(run_cmd, "MiniSWEExecutor", _fake_executor(_ExecResult(success=True)))

    result = runner.invoke(app, ["run", "TASK-001", "--executor", "mini"])
    assert result.exit_code == 0
    assert "mini-SWE-agent finished and task TASK-001 verified" in result.output


def test_run_mini_executor_failure(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    _seed_task(tmp_path)
    _pass_gate(monkeypatch)
    _stub_verification(monkeypatch)
    monkeypatch.setattr(run_cmd, "MiniSWEExecutor", _fake_executor(_ExecResult(success=False)))

    result = runner.invoke(app, ["run", "TASK-001", "--executor", "mini"])
    assert result.exit_code == 0
    assert "mini-SWE-agent failed to start or execute" in result.output


def test_run_openhands_executor_verified(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    _seed_task(tmp_path)
    _pass_gate(monkeypatch)
    _stub_verification(monkeypatch, verified=True)
    monkeypatch.setattr(run_cmd, "OpenHandsExecutor", _fake_executor(_ExecResult(success=True)))

    result = runner.invoke(app, ["run", "TASK-001", "--executor", "openhands"])
    assert result.exit_code == 0
    assert "OpenHands finished and task TASK-001 verified" in result.output


def test_run_native_executor_without_key_errors(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    _seed_task(tmp_path)
    _pass_gate(monkeypatch)

    def raise_missing_key(provider, root):
        raise ValueError("No API key configured for provider")

    monkeypatch.setattr(run_cmd, "get_api_key", raise_missing_key)
    result = runner.invoke(app, ["run", "TASK-001", "--executor", "native"])
    assert result.exit_code == 0
    assert "No API key configured" in result.output


# --- mini/openhands failure branches ----------------------------------------------


def test_run_mini_executor_blocked(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    _seed_task(tmp_path)
    _pass_gate(monkeypatch)
    _stub_verification(monkeypatch, verified=False)
    monkeypatch.setattr(run_cmd, "MiniSWEExecutor", _fake_executor(_ExecResult(success=True)))

    result = runner.invoke(app, ["run", "TASK-001", "--executor", "mini"])
    assert result.exit_code == 0
    assert "blocked by verification gaps" in result.output


def test_run_openhands_executor_failure(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    _seed_task(tmp_path)
    _pass_gate(monkeypatch)
    _stub_verification(monkeypatch)
    monkeypatch.setattr(run_cmd, "OpenHandsExecutor", _fake_executor(_ExecResult(success=False)))

    result = runner.invoke(app, ["run", "TASK-001", "--executor", "openhands"])
    assert result.exit_code == 0
    assert "OpenHands failed to start or execute" in result.output


# --- claude-sdk fallback + real executor branch -----------------------------------


def test_run_claude_sdk_falls_back_to_claude_cli(tmp_path, monkeypatch):
    import importlib.util
    import shutil

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    _seed_task(tmp_path)
    _pass_gate(monkeypatch)
    _stub_verification(monkeypatch, verified=True)
    # SDK missing but `claude` CLI present → fall back to the CLI executor.
    monkeypatch.setattr(importlib.util, "find_spec", lambda name: None if name == "claude_agent_sdk" else object())
    monkeypatch.setattr(shutil, "which", lambda name: "/usr/bin/claude" if name == "claude" else None)
    monkeypatch.setattr(run_cmd, "CodingCliExecutor", _fake_executor(_ExecResult(success=True)))

    result = runner.invoke(app, ["run", "TASK-001", "--executor", "claude-sdk"])
    assert result.exit_code == 0
    assert "falling back to the" in result.output
    assert "verified" in result.output.lower()


def test_run_claude_sdk_executor_runs_when_sdk_present(tmp_path, monkeypatch):
    import importlib.util

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    _seed_task(tmp_path)
    _pass_gate(monkeypatch)
    _stub_verification(monkeypatch, verified=True)
    monkeypatch.setattr(importlib.util, "find_spec", lambda name: object())

    import devcouncil.executors.claude_sdk as sdk_mod
    import devcouncil.executors.agent_registry as reg_mod
    monkeypatch.setattr(sdk_mod, "ClaudeSdkExecutor", _fake_executor(_ExecResult(success=True)))
    monkeypatch.setattr(reg_mod, "load_agent_profiles", lambda root: {})

    result = runner.invoke(app, ["run", "TASK-001", "--executor", "claude-sdk"])
    assert result.exit_code == 0
    assert "claude-sdk finished and task TASK-001 verified" in result.output


def test_run_claude_sdk_executor_failure(tmp_path, monkeypatch):
    import importlib.util

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    _seed_task(tmp_path)
    _pass_gate(monkeypatch)
    _stub_verification(monkeypatch)
    monkeypatch.setattr(importlib.util, "find_spec", lambda name: object())

    import devcouncil.executors.claude_sdk as sdk_mod
    import devcouncil.executors.agent_registry as reg_mod
    monkeypatch.setattr(sdk_mod, "ClaudeSdkExecutor", _fake_executor(_ExecResult(success=False, message="boom")))
    monkeypatch.setattr(reg_mod, "load_agent_profiles", lambda root: {})

    result = runner.invoke(app, ["run", "TASK-001", "--executor", "claude-sdk"])
    assert result.exit_code == 0
    assert "claude-sdk failed to start or execute" in result.output


# --- native executor success/failure ----------------------------------------------


def _stub_native_provider(monkeypatch):
    monkeypatch.setattr(run_cmd, "get_api_key", lambda provider, root: "key")
    monkeypatch.setattr(run_cmd, "validate_model_provider", lambda provider: None)
    monkeypatch.setattr(run_cmd, "create_provider", lambda *a, **k: object())
    monkeypatch.setattr(run_cmd, "ModelRouter", lambda *a, **k: object())


def test_run_native_executor_verified(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    _seed_task(tmp_path)
    _pass_gate(monkeypatch)
    _stub_verification(monkeypatch, verified=True)
    _stub_native_provider(monkeypatch)
    monkeypatch.setattr(run_cmd, "NativeAgent", _fake_executor(_ExecResult(success=True)))

    result = runner.invoke(app, ["run", "TASK-001", "--executor", "native"])
    assert result.exit_code == 0
    assert "Native agent finished and task TASK-001 verified" in result.output


def test_run_native_executor_failure(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    _seed_task(tmp_path)
    _pass_gate(monkeypatch)
    _stub_verification(monkeypatch)
    _stub_native_provider(monkeypatch)
    monkeypatch.setattr(run_cmd, "NativeAgent", _fake_executor(_ExecResult(success=False)))

    result = runner.invoke(app, ["run", "TASK-001", "--executor", "native"])
    assert result.exit_code == 0
    assert "Native agent failed during execution" in result.output


# --- run artifacts printing (last_run_id present) ---------------------------------


def test_run_coding_cli_prints_run_artifacts(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    _seed_task(tmp_path)
    _pass_gate(monkeypatch)
    _stub_verification(monkeypatch, verified=True)

    run_dir = tmp_path / ".devcouncil" / "runs" / "RUN1"
    run_dir.mkdir(parents=True)
    (run_dir / "run.log").write_text("log", encoding="utf-8")

    class _ExecWithRun:
        last_run_id = "RUN1"
        last_transcript_path = str(tmp_path / "t.jsonl")

        def __init__(self, *a, **k):
            pass

        def run_task(self, task, reqs):
            return _ExecResult(success=True)

    monkeypatch.setattr(run_cmd, "CodingCliExecutor", _ExecWithRun)
    result = runner.invoke(app, ["run", "TASK-001", "--executor", "claude"])
    assert result.exit_code == 0
    assert "Run artifacts" in result.output
    assert "Transcript" in result.output


def test_run_coding_cli_checkpoint_output(tmp_path, monkeypatch):
    from types import SimpleNamespace

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    _seed_task(tmp_path)
    _pass_gate(monkeypatch)
    _stub_verification(monkeypatch, verified=True)
    monkeypatch.setattr(run_cmd, "CodingCliExecutor", _fake_executor(_ExecResult(success=True)))

    import devcouncil.execution.checkpoints as cp

    class _CP:
        def __init__(self, root):
            pass

        def create_before(self, task_id):
            return SimpleNamespace(patch_path="cp.patch", git_ref_created=False, ref=None)

    monkeypatch.setattr(cp, "CheckpointService", _CP)
    result = runner.invoke(app, ["run", "TASK-001", "--executor", "claude"])
    assert result.exit_code == 0
    assert "Created git checkpoint at" in result.output


# --- helper: _current_changed_files + _verify_after_execution ---------------------


def test_current_changed_files_returns_list(tmp_path):
    assert isinstance(run_cmd._current_changed_files(tmp_path), list)


def test_verify_after_execution_saves_evidence_and_verifies(tmp_path, monkeypatch):
    from devcouncil.domain.evidence import CommandResult, DiffEvidence, TestEvidence
    from devcouncil.verification.verifier import Verifier

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    _seed_task(tmp_path)

    async def fake_verify_task(self, task, reqs):
        evidence = [
            CommandResult(command="pytest", exit_code=0, stdout_path="", stderr_path="", summary="ok"),
            DiffEvidence(task_id=task.id, changed_files=["a.py"], added_files=[], deleted_files=[], diff_summary="s"),
            TestEvidence(requirement_id="R1", acceptance_criterion_id="AC1", command="pytest", status="passed", evidence_summary="ok"),
        ]
        return [], evidence

    monkeypatch.setattr(Verifier, "verify_task", fake_verify_task)
    db = get_db(tmp_path)
    with db.get_session() as session:
        task = TaskRepository(session).get_by_id("TASK-001")
        verified = run_cmd._verify_after_execution(session, task, [], router=None, project_root=tmp_path)
    assert verified is True


def test_verify_after_execution_blocking_returns_false(tmp_path, monkeypatch):
    from devcouncil.domain.gap import Gap
    from devcouncil.verification.verifier import Verifier

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    _seed_task(tmp_path)

    async def fake_verify_task(self, task, reqs):
        gap = Gap(
            id="G1", severity="high", gap_type="missing_test", description="no test",
            blocking=True, recommended_fix="test", task_id=task.id,
        )
        return [gap], []

    monkeypatch.setattr(Verifier, "verify_task", fake_verify_task)
    db = get_db(tmp_path)
    with db.get_session() as session:
        task = TaskRepository(session).get_by_id("TASK-001")
        verified = run_cmd._verify_after_execution(session, task, [], router=None, project_root=tmp_path)
    assert verified is False


# --- helper: _capture_* + _record_agent_verification + _build_verification_router -


def test_capture_after_patch_swallows_errors(tmp_path, monkeypatch):
    import devcouncil.execution.checkpoints as cp

    def boom(root):
        raise RuntimeError("no git")

    monkeypatch.setattr(cp, "CheckpointService", boom)
    # Best-effort: must not raise.
    run_cmd._capture_after_patch("TASK-001", tmp_path)


def test_capture_before_snapshot(tmp_path, monkeypatch):
    from types import SimpleNamespace
    import devcouncil.execution.checkpoints as cp

    seen = {}

    class _CP:
        def __init__(self, root):
            pass

        def create_before(self, task_id):
            seen["task"] = task_id
            return SimpleNamespace()

    monkeypatch.setattr(cp, "CheckpointService", _CP)
    run_cmd._capture_before_snapshot("TASK-9", tmp_path)
    assert seen["task"] == "TASK-9"


def test_record_agent_verification_does_not_raise(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    run_cmd._record_agent_verification(tmp_path, "TASK-001", "claude", "RUN1", True)


def test_build_verification_router_success(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    monkeypatch.setattr(run_cmd, "validate_model_provider", lambda provider: None)
    monkeypatch.setattr(run_cmd, "get_api_key", lambda provider, root: "key")
    monkeypatch.setattr(run_cmd, "create_provider", lambda *a, **k: object())
    monkeypatch.setattr(run_cmd, "ModelRouter", lambda *a, **k: "ROUTER")
    assert run_cmd._build_verification_router(tmp_path) == "ROUTER"


# --- helper: _verify_executor_output_if_present -----------------------------------


def test_verify_executor_output_no_changes_returns_false(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    _seed_task(tmp_path)
    monkeypatch.setattr(run_cmd, "_current_changed_files", lambda root: [])
    db = get_db(tmp_path)
    with db.get_session() as session:
        task = TaskRepository(session).get_by_id("TASK-001")
        result = run_cmd._verify_executor_output_if_present(
            session, task, [], root=tmp_path, executor_label="X",
        )
    assert result is False


def test_verify_executor_output_present_verified(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    _seed_task(tmp_path)
    monkeypatch.setattr(run_cmd, "_current_changed_files", lambda root: ["a.py"])
    monkeypatch.setattr(run_cmd, "_build_verification_router", lambda root: None)
    monkeypatch.setattr(run_cmd, "_verify_after_execution", lambda *a, **k: True)
    monkeypatch.setattr(run_cmd, "_record_agent_verification", lambda *a, **k: None)
    monkeypatch.setattr(run_cmd, "_run_live_review_after_execution", lambda *a, **k: None)
    db = get_db(tmp_path)
    with db.get_session() as session:
        task = TaskRepository(session).get_by_id("TASK-001")
        result = run_cmd._verify_executor_output_if_present(
            session, task, [], root=tmp_path, executor_label="X", cli_client="claude", cli_executor=None,
        )
    assert result is True


def test_verify_executor_output_present_blocked(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    _seed_task(tmp_path)
    monkeypatch.setattr(run_cmd, "_current_changed_files", lambda root: ["a.py"])
    monkeypatch.setattr(run_cmd, "_build_verification_router", lambda root: None)
    monkeypatch.setattr(run_cmd, "_verify_after_execution", lambda *a, **k: False)
    db = get_db(tmp_path)
    with db.get_session() as session:
        task = TaskRepository(session).get_by_id("TASK-001")
        result = run_cmd._verify_executor_output_if_present(
            session, task, [], root=tmp_path, executor_label="X",
        )
    assert result is True


# --- blocked (verified=False) executor branches -----------------------------------


def test_run_openhands_executor_blocked(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    _seed_task(tmp_path)
    _pass_gate(monkeypatch)
    _stub_verification(monkeypatch, verified=False)
    monkeypatch.setattr(run_cmd, "OpenHandsExecutor", _fake_executor(_ExecResult(success=True)))
    result = runner.invoke(app, ["run", "TASK-001", "--executor", "openhands"])
    assert result.exit_code == 0
    assert "blocked by verification gaps" in result.output


def test_run_claude_sdk_executor_blocked(tmp_path, monkeypatch):
    import importlib.util

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    _seed_task(tmp_path)
    _pass_gate(monkeypatch)
    _stub_verification(monkeypatch, verified=False)
    monkeypatch.setattr(importlib.util, "find_spec", lambda name: object())
    import devcouncil.executors.claude_sdk as sdk_mod
    import devcouncil.executors.agent_registry as reg_mod
    monkeypatch.setattr(sdk_mod, "ClaudeSdkExecutor", _fake_executor(_ExecResult(success=True)))
    monkeypatch.setattr(reg_mod, "load_agent_profiles", lambda root: {})
    result = runner.invoke(app, ["run", "TASK-001", "--executor", "claude-sdk"])
    assert result.exit_code == 0
    assert "blocked by verification gaps" in result.output


def test_run_native_executor_blocked(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    _seed_task(tmp_path)
    _pass_gate(monkeypatch)
    _stub_verification(monkeypatch, verified=False)
    _stub_native_provider(monkeypatch)
    monkeypatch.setattr(run_cmd, "NativeAgent", _fake_executor(_ExecResult(success=True)))
    result = runner.invoke(app, ["run", "TASK-001", "--executor", "native"])
    assert result.exit_code == 0
    assert "blocked by verification gaps" in result.output
