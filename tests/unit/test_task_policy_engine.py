from pathlib import Path

from devcouncil.domain.task import PlannedFile, Task
from devcouncil.execution.policy_engine import TaskPolicyEngine


def _task(**kwargs) -> Task:
    defaults = {
        "id": "TASK-001",
        "title": "Test",
        "description": "Test",
        "allowed_commands": ["pytest tests/**"],
        "planned_files": [
            PlannedFile(path="src/app.py", reason="impl", allowed_change="modify"),
            PlannedFile(path="pyproject.toml", reason="deps", allowed_change="modify"),
        ],
        "forbidden_changes": ["src/locked.py"],
    }
    defaults.update(kwargs)
    return Task(**defaults)


def test_deny_shell_commands_with_no_active_task(tmp_path: Path):
    engine = TaskPolicyEngine(tmp_path)
    decision = engine.evaluate_command("npm test", None)
    assert decision.action == "deny"


def test_allow_read_only_commands_without_task(tmp_path: Path):
    engine = TaskPolicyEngine(tmp_path)
    decision = engine.evaluate_command("git status", None)
    assert decision.action == "allow"


def test_allow_task_command_matching_allowed_commands(tmp_path: Path):
    engine = TaskPolicyEngine(tmp_path)
    task = _task()
    decision = engine.evaluate_command("pytest tests/unit", task)
    assert decision.action == "allow"


def test_allow_global_verification_command(tmp_path: Path):
    engine = TaskPolicyEngine(tmp_path, global_allowed_commands=["ruff check ."])
    task = _task(allowed_commands=[])
    decision = engine.evaluate_command("ruff check .", task)
    assert decision.action == "allow"


def test_deny_unplanned_file_write(tmp_path: Path):
    engine = TaskPolicyEngine(tmp_path)
    task = _task()
    decision = engine.evaluate_file_change("src/other.py", task)
    assert decision.action == "deny"


def test_deny_forbidden_changes_even_when_planned(tmp_path: Path):
    engine = TaskPolicyEngine(tmp_path)
    task = _task(
        planned_files=[
            PlannedFile(path="src/locked.py", reason="locked", allowed_change="modify"),
        ],
        forbidden_changes=["src/locked.py"],
    )
    decision = engine.evaluate_file_change("src/locked.py", task)
    assert decision.action == "deny"


def test_warn_planned_protected_file_changes(tmp_path: Path):
    engine = TaskPolicyEngine(tmp_path)
    task = _task()
    decision = engine.evaluate_file_change("pyproject.toml", task)
    assert decision.action == "warn"


def test_hook_denies_no_verify_and_force_push(tmp_path: Path):
    engine = TaskPolicyEngine(tmp_path)
    assert engine.evaluate_hook_command("git commit --no-verify").action == "deny"
    assert engine.evaluate_hook_command("git push origin main --force").action == "deny"
    assert engine.evaluate_hook_command("git reset --hard main").action == "deny"
