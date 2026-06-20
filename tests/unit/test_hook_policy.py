from devcouncil.domain.task import PlannedFile, Task
from devcouncil.execution.hook_policy import HookPolicy


def _task() -> Task:
    return Task(
        id="TASK-001",
        title="Task",
        description="desc",
        planned_files=[
            PlannedFile(path="src/app.py", reason="logic", allowed_change="modify"),
            PlannedFile(path="package.json", reason="dependency", allowed_change="modify"),
        ],
        status="running",
    )


def test_hook_policy_allows_planned_file_write():
    decision = HookPolicy().evaluate(
        {"name": "write_file", "arguments": {"path": "src/app.py"}},
        _task(),
    )

    assert decision.action == "allow"


def test_hook_policy_accepts_claude_style_tool_input_shape():
    decision = HookPolicy().evaluate(
        {"tool": "Write", "tool_input": {"file_path": "src/app.py"}},
        _task(),
    )

    assert decision.action == "allow"


def test_hook_policy_accepts_gemini_style_tool_input_shape():
    decision = HookPolicy().evaluate(
        {"tool_name": "write_file", "tool_input": {"target_file": "src/app.py"}},
        _task(),
    )

    assert decision.action == "allow"


def test_hook_policy_accepts_codex_style_shell_command_shape():
    decision = HookPolicy().evaluate(
        {"tool_name": "shell_command", "input": {"command": "git commit --no-verify -m test"}},
        _task(),
    )

    assert decision.action == "deny"
    assert "Verification bypass" in decision.reason


def test_hook_policy_blocks_unplanned_file_write():
    decision = HookPolicy().evaluate(
        {"name": "write_file", "arguments": {"path": "src/other.py"}},
        _task(),
    )

    assert decision.action == "deny"
    assert "does not authorize" in decision.reason


def test_hook_policy_blocks_secret_path_even_when_planned():
    task = Task(
        id="TASK-001",
        title="Task",
        description="desc",
        planned_files=[PlannedFile(path=".env", reason="bad", allowed_change="modify")],
        status="running",
    )

    decision = HookPolicy().evaluate({"name": "write_file", "arguments": {"path": ".env"}}, task)

    assert decision.action == "deny"


def test_hook_policy_denies_dangerous_git_commands():
    policy = HookPolicy()

    # Git-safety deny wins regardless of active task.
    assert policy.evaluate_command("git push --force").action == "deny"
    assert policy.evaluate_command("git commit --no-verify -m test").action == "deny"
    assert policy.evaluate_command("git reset --hard origin/main").action == "deny"


def _task_allowing(*commands: str) -> Task:
    return Task(
        id="TASK-001",
        title="Task",
        description="desc",
        allowed_commands=list(commands),
        planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
        status="running",
    )


def test_hook_policy_warns_for_protected_file():
    policy = HookPolicy()

    assert policy.evaluate_file_write("package.json", _task()).action == "warn"


def test_hook_policy_warns_for_direct_push_when_task_allows_it():
    # The push command must be authorized by the task for the protected-branch warn to
    # surface; otherwise the missing lease denies it (fail-closed) before the warn.
    policy = HookPolicy()
    task = _task_allowing("git push origin *")

    assert policy.evaluate_command("git push origin main", task).action == "warn"
    assert policy.evaluate_command("git push origin HEAD:main", task).action == "warn"


def test_hook_policy_denies_direct_push_without_active_task():
    policy = HookPolicy()

    assert policy.evaluate_command("git push origin main", None).action == "deny"
