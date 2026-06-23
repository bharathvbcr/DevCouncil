"""Hook active-task resolution (rank 4) and integrate posture/tamper checks (ranks 22, 3)."""

import json
import subprocess

from typer.testing import CliRunner

from devcouncil.cli.commands.hook import _active_task
from devcouncil.cli.main import app
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.integrations.check import build_integration_check_report
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import TaskRepository

runner = CliRunner()


def _init_repo(tmp_path):
    subprocess.run(["git", "init"], cwd=tmp_path, capture_output=True)
    dev = tmp_path / ".devcouncil"
    dev.mkdir()
    (dev / "config.yaml").write_text("project:\n  name: test\n", encoding="utf-8")
    return get_db(tmp_path)


def _task(task_id: str) -> Task:
    return Task(
        id=task_id,
        title="T",
        description="d",
        status="running",
        planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
    )


# --- rank 4: ambiguous active task resolves to None (fail-closed) ------------------


def test_active_task_resolves_single_running(tmp_path):
    db = _init_repo(tmp_path)
    with db.get_session() as session:
        TaskRepository(session).save(_task("TASK-001"))
    task = _active_task(tmp_path)
    assert task is not None and task.id == "TASK-001"


def test_active_task_none_when_ambiguous(tmp_path):
    db = _init_repo(tmp_path)
    with db.get_session() as session:
        repo = TaskRepository(session)
        repo.save(_task("TASK-001"))
        repo.save(_task("TASK-002"))
    # Two running tasks -> ambiguous -> None, so the engine fails closed.
    assert _active_task(tmp_path) is None


def test_active_task_none_when_no_running(tmp_path):
    _init_repo(tmp_path)
    assert _active_task(tmp_path) is None


# --- rank 22: verify-only client warning + matrix posture column ------------------


def test_matrix_shows_enforcement_posture(tmp_path):
    _init_repo(tmp_path)
    result = runner.invoke(app, ["integrate", "matrix", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "Enforcement" in result.output
    assert "pre-action" in result.output
    assert "verify-only" in result.output


def test_aider_apply_warns_no_pre_action_containment(tmp_path):
    _init_repo(tmp_path)
    result = runner.invoke(app, ["integrate", "aider", "--apply", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "No pre-action containment" in result.output


# --- rank 3: hook config tamper tripwire in dev integrate check -------------------


def test_check_flags_tampered_hook_config(tmp_path):
    _init_repo(tmp_path)
    claude_dir = tmp_path / ".claude"
    claude_dir.mkdir()
    # A hook config that no longer references devcouncil = disarmed gate.
    (claude_dir / "settings.local.json").write_text(
        json.dumps({"hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": []}]}}),
        encoding="utf-8",
    )
    report = build_integration_check_report(tmp_path)
    integrity = [r for r in report.checks if r.name == "Claude hook integrity"]
    assert integrity and integrity[0].status == "fail"


def test_check_passes_when_hook_config_references_devcouncil(tmp_path):
    _init_repo(tmp_path)
    claude_dir = tmp_path / ".claude"
    claude_dir.mkdir()
    (claude_dir / "settings.local.json").write_text(
        json.dumps(
            {
                "hooks": {
                    "PreToolUse": [
                        {
                            "matcher": "Bash",
                            "hooks": [{"type": "command", "command": "devcouncil hook pre-tool-use"}],
                        }
                    ]
                }
            }
        ),
        encoding="utf-8",
    )
    report = build_integration_check_report(tmp_path)
    integrity = [r for r in report.checks if r.name == "Claude hook integrity"]
    assert integrity and integrity[0].status == "ok"


def test_check_skips_integrity_when_no_hook_config(tmp_path):
    _init_repo(tmp_path)
    report = build_integration_check_report(tmp_path)
    assert not any(r.name == "Claude hook integrity" for r in report.checks)
