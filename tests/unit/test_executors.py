import subprocess

from devcouncil.domain.task import PlannedFile, Task
from devcouncil.executors.mini_swe import MiniSWEExecutor
from devcouncil.executors.openhands import OpenHandsExecutor
from devcouncil.executors.coding_cli import CodingCliExecutor


def test_openhands_executor_uses_task_file_not_giant_argv(tmp_path, monkeypatch):
    captured = {}

    def fake_run(cmd, **kwargs):
        captured["cmd"] = cmd
        return subprocess.CompletedProcess(cmd, 0, stdout="", stderr="")

    monkeypatch.setattr("subprocess.run", fake_run)
    task = Task(
        id="TASK-001",
        title="OpenHands",
        description="A" * 10_000,
        planned_files=[
            PlannedFile(path="src/app.py", reason="logic", allowed_change="modify"),
        ],
    )

    result = OpenHandsExecutor(tmp_path).run_task(task, [])

    assert result.success
    command = captured["cmd"]
    assert "--task-file" in command
    assert not any("A" * 100 in str(part) for part in command)
    task_file = tmp_path / ".devcouncil" / "TASK-001-openhands-task.md"
    assert task_file.exists()
    assert "A" * 100 in task_file.read_text(encoding="utf-8")


def test_mini_swe_executor_uses_task_scoped_instruction_file(tmp_path, monkeypatch):
    captured = {}

    def fake_run(cmd, **kwargs):
        captured["cmd"] = cmd
        return subprocess.CompletedProcess(cmd, 0, stdout="", stderr="")

    monkeypatch.setattr("subprocess.run", fake_run)
    task = Task(
        id="TASK-001",
        title="mini",
        description="desc",
        planned_files=[
            PlannedFile(path="src/app.py", reason="logic", allowed_change="modify"),
        ],
    )

    result = MiniSWEExecutor(tmp_path).run_task(task, [])

    assert result.success
    command = captured["cmd"]
    task_file = tmp_path / ".devcouncil" / "TASK-001-mini-swe-task.md"
    assert "--instruction-file" in command
    assert str(task_file) in command
    assert task_file.exists()


def test_coding_cli_executor_uses_stdin_prompt(tmp_path, monkeypatch):
    captured = {}

    def fake_which(_command):
        return f"/usr/bin/{_command}"

    def fake_run(cmd, **kwargs):
        captured["cmd"] = cmd
        captured["input"] = kwargs.get("input", "")
        return subprocess.CompletedProcess(cmd, 0, stdout="", stderr="")

    monkeypatch.setattr("shutil.which", fake_which)
    monkeypatch.setattr("subprocess.run", fake_run)
    task = Task(
        id="TASK-001",
        title="Coding CLI",
        description="Implement feature",
        planned_files=[
            PlannedFile(path="src/app.py", reason="logic", allowed_change="modify"),
        ],
    )

    result = CodingCliExecutor(tmp_path, "codex").run_task(task, [])

    assert result.success
    assert captured["cmd"] == ["codex", "exec", "-"]
    assert task.description in captured["input"]
    assert "TASK-001" in captured["input"]


def test_coding_cli_executor_normalizes_aliases(tmp_path, monkeypatch):
    captured = {}

    def fake_which(_command):
        return f"/usr/bin/{_command}"

    def fake_run(cmd, **kwargs):
        captured["cmd"] = cmd
        return subprocess.CompletedProcess(cmd, 0, stdout="", stderr="")

    monkeypatch.setattr("shutil.which", fake_which)
    monkeypatch.setattr("subprocess.run", fake_run)

    task = Task(
        id="TASK-001",
        title="Coding CLI",
        description="Implement feature",
        planned_files=[
            PlannedFile(path="src/app.py", reason="logic", allowed_change="modify"),
        ],
    )

    for alias, expected in [
        ("codex", ["codex", "exec", "-"]),
        ("codex_cli", ["codex", "exec", "-"]),
        ("codex-cli", ["codex", "exec", "-"]),
        ("gemini-cli", ["gemini"]),
        ("gemini_cli", ["gemini"]),
        ("claude-code", ["claude", "-p"]),
        ("claude_cli", ["claude", "-p"]),
    ]:
        result = CodingCliExecutor(tmp_path, alias).run_task(task, [])

        assert result.success
        assert captured["cmd"] == expected


def test_coding_cli_executor_reports_missing_binary(tmp_path, monkeypatch):
    def fake_which(_command):
        return None

    monkeypatch.setattr("shutil.which", fake_which)

    result = CodingCliExecutor(tmp_path, "gemini").run_task(
        Task(
            id="TASK-001",
            title="Coding CLI",
            description="desc",
            planned_files=[
                PlannedFile(path="src/app.py", reason="logic", allowed_change="modify"),
            ],
        ),
        [],
    )

    assert not result.success
    assert "not installed or not on PATH" in result.message
