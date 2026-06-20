"""Rank 11 — unified path normalization (out-of-root denial) and fail-closed hooks."""

from pathlib import Path

from typer.testing import CliRunner

from devcouncil.cli.main import app
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.execution.hook_policy import HookPolicy
from devcouncil.execution.policy_engine import TaskPolicyEngine, normalize_repo_path

runner = CliRunner()


def _task() -> Task:
    return Task(
        id="TASK-001", title="T", description="d", status="running",
        planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
    )


def test_normalize_repo_path_flags_escapes(tmp_path):
    root = tmp_path
    inside, outside = normalize_repo_path(root, "src/app.py")
    assert inside == "src/app.py" and outside is False
    _, esc = normalize_repo_path(root, "../../etc/passwd")
    assert esc is True
    _, abs_esc = normalize_repo_path(root, "/etc/passwd" if Path("/").exists() else "C:/Windows/x")
    assert abs_esc is True


def test_engine_denies_out_of_root_write(tmp_path):
    engine = TaskPolicyEngine(tmp_path)
    decision = engine.evaluate_file_change("../escape.py", _task())
    assert decision.action == "deny"
    assert "outside the project root" in decision.reason


def test_hook_policy_denies_out_of_root_write(tmp_path):
    decision = HookPolicy(project_root=tmp_path).evaluate(
        {"name": "write_file", "arguments": {"path": "../../secrets.py"}},
        _task(),
    )
    assert decision.action == "deny"


def test_hook_policy_still_allows_planned_inside_root(tmp_path):
    decision = HookPolicy(project_root=tmp_path).evaluate(
        {"name": "write_file", "arguments": {"path": "src/app.py"}},
        _task(),
    )
    assert decision.action == "allow"


# --- fail-closed hook CLI ---

def test_hook_malformed_json_warns_but_allows_by_default():
    result = runner.invoke(app, ["hook", "pre-tool-use", "not json", "--client", "generic"])
    assert result.exit_code == 0  # benign default: warn + allow


def test_hook_malformed_json_blocks_in_strict_mode():
    result = runner.invoke(app, ["hook", "pre-tool-use", "not json", "--client", "generic", "--strict"])
    assert result.exit_code == 2  # fail closed


def test_hook_empty_payload_allows():
    result = runner.invoke(app, ["hook", "pre-tool-use", "   ", "--client", "generic"])
    assert result.exit_code == 0


def test_hook_empty_payload_blocks_in_strict_mode():
    result = runner.invoke(app, ["hook", "pre-tool-use", "", "--client", "generic", "--strict"])
    # Empty stdin under strict fails closed.
    assert result.exit_code == 2
