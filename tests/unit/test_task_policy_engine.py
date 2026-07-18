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


def test_allow_lease_bootstrap_commands_without_task(tmp_path: Path):
    engine = TaskPolicyEngine(tmp_path)
    for command in (
        "dev checkout TASK-001 --client-id cursor --json",
        "uv run dev checkout TASK-001 --client-id cursor --json",
        "dev next-task --json",
        "uv run dev next-task --json",
    ):
        decision = engine.evaluate_command(command, None)
        assert decision.action == "allow", command


def test_allow_lease_lifecycle_commands_with_or_without_task(tmp_path: Path):
    engine = TaskPolicyEngine(tmp_path)
    task = _task(allowed_commands=["pytest tests/**"])
    for command in (
        "dev release TASK-001 --lease-token abc --json",
        "uv run dev release TASK-001 --lease-token abc --json",
        "dev lease renew TASK-001 --lease-token abc --json",
        "uv run dev lease list --json",
        "dev map",
        "uv run dev map",
        "dev doctor",
        "uv run dev doctor",
        "dev graph status",
        "uv run dev graph dead --confidence extracted",
        ".venv/bin/dev map",
        str(tmp_path / ".venv" / "bin" / "dev") + " map --force",
        "uv run --project /tmp/repo dev map",
        "cd /tmp/repo",
    ):
        assert engine.evaluate_command(command, None).action == "allow", command
        assert engine.evaluate_command(command, task).action == "allow", command


def test_normalize_path_prefixed_dev_commands(tmp_path: Path):
    from devcouncil.execution.policy_engine import normalize_allowlist_command

    assert normalize_allowlist_command(".venv/bin/dev map") == "dev map"
    assert normalize_allowlist_command("/abs/.venv/bin/devcouncil map") == "dev map"
    assert (
        normalize_allowlist_command("uv run --project /x --directory /y dev map")
        == "uv run dev map"
    )
    assert normalize_allowlist_command("dev map --help >/dev/null") == "dev map --help"
    assert normalize_allowlist_command("dev status 2>&1") == "dev status"
    # Repo folder named DevCouncil must NOT be treated as the CLI binary.
    assert (
        normalize_allowlist_command("cd /Users/bharath/Code/DevCouncil")
        == "cd /Users/bharath/Code/DevCouncil"
    )


def test_lease_lifecycle_does_not_bypass_task_command_gate_for_other_commands(tmp_path: Path):
    engine = TaskPolicyEngine(tmp_path)
    task = _task(allowed_commands=[])
    decision = engine.evaluate_command("npm test", task)
    assert decision.action == "deny"


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


def test_allow_neighbor_subsystem_write(tmp_path: Path, monkeypatch):
    engine = TaskPolicyEngine(tmp_path)
    task = _task(
        planned_files=[
            PlannedFile(path="src/devcouncil/cli/main.py", reason="cli", allowed_change="modify"),
        ],
    )
    repo_map = {
        "subsystems": [
            {"area": "src/devcouncil/cli", "neighbors": ["src/devcouncil/indexing"]},
            {"area": "src/devcouncil/indexing", "neighbors": ["src/devcouncil/cli"]},
        ],
        "files": [],
    }
    map_path = tmp_path / ".devcouncil" / "repo_map.json"
    map_path.parent.mkdir(parents=True)
    map_path.write_text(__import__("json").dumps(repo_map), encoding="utf-8")
    decision = engine.evaluate_file_change("src/devcouncil/indexing/wiring.py", task)
    assert decision.action == "allow"


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
