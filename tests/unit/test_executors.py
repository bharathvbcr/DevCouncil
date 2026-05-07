import json
import subprocess

from devcouncil.domain.task import PlannedFile, Task
from devcouncil.executors.mini_swe import MiniSWEExecutor
from devcouncil.executors.openhands import OpenHandsExecutor
from devcouncil.executors.coding_cli import CodingCliExecutor
from devcouncil.executors.agent_registry import load_cli_agent_specs


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
        ("warp", ["oz", "agent", "run"]),
        ("oz-cli", ["oz", "agent", "run"]),
    ]:
        result = CodingCliExecutor(tmp_path, alias).run_task(task, [])

        assert result.success
        assert captured["cmd"][:len(expected)] == expected


def test_coding_cli_executor_supports_custom_cli_agents(tmp_path, monkeypatch):
    import yaml

    captured = {}

    def fake_which(_command):
        return f"/usr/bin/{_command}"

    def fake_run(cmd, **kwargs):
        captured["cmd"] = cmd
        captured["input"] = kwargs.get("input")
        captured["env"] = kwargs.get("env")
        return subprocess.CompletedProcess(cmd, 0, stdout="", stderr="")

    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / ".devcouncil" / "config.yaml").write_text(
        yaml.safe_dump({
            "integrations": {
                "cli_agents": {
                    "agents": {
                        "opencode": {
                            "command": "opencode",
                            "args": ["run"],
                            "input_mode": "prompt-file",
                            "prompt_arg": "--prompt-file",
                            "env": {"OPENCODE_MODE": "devcouncil"},
                        }
                    }
                }
            }
        }),
        encoding="utf-8",
    )
    monkeypatch.setattr("shutil.which", fake_which)
    monkeypatch.setattr("subprocess.run", fake_run)

    task = Task(
        id="TASK-001",
        title="BYO CLI",
        description="Implement feature",
        planned_files=[
            PlannedFile(path="src/app.py", reason="logic", allowed_change="modify"),
        ],
    )

    result = CodingCliExecutor(tmp_path, "opencode").run_task(task, [])

    assert result.success
    assert captured["cmd"][:3] == ["opencode", "run", "--prompt-file"]
    assert captured["cmd"][3].endswith("TASK-001-opencode-task.md")
    assert captured["input"] is None
    assert captured["env"]["OPENCODE_MODE"] == "devcouncil"


def test_agent_registry_does_not_let_custom_agents_shadow_builtins(tmp_path):
    import yaml

    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / ".devcouncil" / "config.yaml").write_text(
        yaml.safe_dump({
            "integrations": {
                "cli_agents": {
                    "agents": {
                        "oz": {
                            "command": "custom-oz",
                            "input_mode": "stdin",
                        },
                        "codex": {
                            "command": "custom-codex",
                            "input_mode": "stdin",
                        },
                    }
                }
            }
        }),
        encoding="utf-8",
    )

    specs = load_cli_agent_specs(tmp_path)

    assert specs["warp"].command == "oz"
    assert specs["codex"].command == "codex"
    assert "oz" not in specs


def test_coding_cli_executor_writes_manifest_and_trace_events(tmp_path, monkeypatch):
    captured = {}

    def fake_which(_command):
        return f"/usr/bin/{_command}"

    def fake_run(cmd, **kwargs):
        captured["cmd"] = cmd
        return subprocess.CompletedProcess(cmd, 0, stdout="ok", stderr="")

    monkeypatch.setattr("shutil.which", fake_which)
    monkeypatch.setattr("subprocess.run", fake_run)
    task = Task(
        id="TASK-001",
        title="Coding CLI",
        description="Implement feature",
        planned_files=[
            PlannedFile(path="src/app.py", reason="logic", allowed_change="modify"),
        ],
        allowed_commands=["pytest"],
        expected_tests=["pytest"],
    )

    executor = CodingCliExecutor(tmp_path, "codex")
    result = executor.run_task(task, [])

    assert result.success
    assert executor.last_run_id is not None
    manifest = tmp_path / ".devcouncil" / "runs" / executor.last_run_id / "agent-run.json"
    data = json.loads(manifest.read_text(encoding="utf-8"))
    assert data["task_id"] == "TASK-001"
    assert data["agent"] == "codex"
    assert data["command"] == captured["cmd"]
    traces = (tmp_path / ".devcouncil" / "logs" / "traces.jsonl").read_text(encoding="utf-8")
    assert "agent_run_started" in traces
    assert "agent_run_finished" in traces


def test_coding_cli_executor_redacts_prompt_from_argument_mode_manifest(tmp_path, monkeypatch):
    import yaml

    captured = {}

    def fake_which(_command):
        return f"/usr/bin/{_command}"

    def fake_run(cmd, **kwargs):
        captured["cmd"] = cmd
        return subprocess.CompletedProcess(cmd, 0, stdout="ok", stderr="")

    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / ".devcouncil" / "config.yaml").write_text(
        yaml.safe_dump({
            "integrations": {
                "cli_agents": {
                    "agents": {
                        "argagent": {
                            "command": "argagent",
                            "input_mode": "argument",
                            "prompt_arg": "--prompt",
                        }
                    }
                }
            }
        }),
        encoding="utf-8",
    )
    monkeypatch.setattr("shutil.which", fake_which)
    monkeypatch.setattr("subprocess.run", fake_run)
    task = Task(
        id="TASK-001",
        title="Argument Agent",
        description="DO_NOT_TRACE_THIS_PROMPT_BODY",
        planned_files=[
            PlannedFile(path="src/app.py", reason="logic", allowed_change="modify"),
        ],
    )

    executor = CodingCliExecutor(tmp_path, "argagent")
    result = executor.run_task(task, [])

    assert result.success
    assert "DO_NOT_TRACE_THIS_PROMPT_BODY" in " ".join(captured["cmd"])
    manifest = tmp_path / ".devcouncil" / "runs" / executor.last_run_id / "agent-run.json"
    manifest_text = manifest.read_text(encoding="utf-8")
    traces = (tmp_path / ".devcouncil" / "logs" / "traces.jsonl").read_text(encoding="utf-8")
    assert "DO_NOT_TRACE_THIS_PROMPT_BODY" not in manifest_text
    assert "DO_NOT_TRACE_THIS_PROMPT_BODY" not in traces
    assert "<task prompt>" in manifest_text


def test_coding_cli_executor_redacts_embedded_prompt_from_argument_mode_manifest(tmp_path, monkeypatch):
    import yaml

    captured = {}

    def fake_which(_command):
        return f"/usr/bin/{_command}"

    def fake_run(cmd, **kwargs):
        captured["cmd"] = cmd
        return subprocess.CompletedProcess(cmd, 0, stdout="ok", stderr="")

    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / ".devcouncil" / "config.yaml").write_text(
        yaml.safe_dump({
            "integrations": {
                "cli_agents": {
                    "agents": {
                        "argagent": {
                            "command": "argagent",
                            "args": ["--message={prompt}"],
                            "input_mode": "argument",
                        }
                    }
                }
            }
        }),
        encoding="utf-8",
    )
    monkeypatch.setattr("shutil.which", fake_which)
    monkeypatch.setattr("subprocess.run", fake_run)
    task = Task(
        id="TASK-001",
        title="Embedded Prompt",
        description="DO_NOT_TRACE_EMBEDDED_PROMPT_BODY",
        planned_files=[
            PlannedFile(path="src/app.py", reason="logic", allowed_change="modify"),
        ],
    )

    executor = CodingCliExecutor(tmp_path, "argagent")
    result = executor.run_task(task, [])

    assert result.success
    assert "DO_NOT_TRACE_EMBEDDED_PROMPT_BODY" in " ".join(captured["cmd"])
    manifest = tmp_path / ".devcouncil" / "runs" / executor.last_run_id / "agent-run.json"
    manifest_text = manifest.read_text(encoding="utf-8")
    traces = (tmp_path / ".devcouncil" / "logs" / "traces.jsonl").read_text(encoding="utf-8")
    assert "DO_NOT_TRACE_EMBEDDED_PROMPT_BODY" not in manifest_text
    assert "DO_NOT_TRACE_EMBEDDED_PROMPT_BODY" not in traces
    assert "--message=<task prompt>" in manifest_text


def test_coding_cli_executor_redacts_secrets_from_logs_and_failure_detail(tmp_path, monkeypatch):
    def fake_which(_command):
        return f"/usr/bin/{_command}"

    def fake_run(cmd, **kwargs):
        return subprocess.CompletedProcess(
            cmd,
            1,
            stdout="",
            stderr="api_key=SECRETSECRETSECRET1\n",
        )

    monkeypatch.setattr("shutil.which", fake_which)
    monkeypatch.setattr("subprocess.run", fake_run)

    result = CodingCliExecutor(tmp_path, "codex").run_task(
        Task(
            id="TASK-001",
            title="Secret log",
            description="desc",
            planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
        ),
        [],
    )

    assert not result.success
    assert "SECRETSECRETSECRET1" not in result.message
    log_text = (tmp_path / ".devcouncil" / "logs" / "TASK-001-codex.log").read_text(encoding="utf-8")
    trace_text = (tmp_path / ".devcouncil" / "logs" / "traces.jsonl").read_text(encoding="utf-8")
    assert "SECRETSECRETSECRET1" not in log_text
    assert "SECRETSECRETSECRET1" not in trace_text
    assert "[REDACTED:generic_api_key]" in log_text


def test_coding_cli_executor_profile_changes_prompt_and_timeout(tmp_path, monkeypatch):
    import yaml

    captured = {}

    def fake_which(_command):
        return f"/usr/bin/{_command}"

    def fake_run(_cmd, **kwargs):
        captured["input"] = kwargs.get("input", "")
        captured["timeout"] = kwargs.get("timeout")
        captured["env"] = kwargs.get("env")
        return subprocess.CompletedProcess(_cmd, 0, stdout="", stderr="")

    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / ".devcouncil" / "config.yaml").write_text(
        yaml.safe_dump({
            "integrations": {
                "cli_agents": {
                    "profiles": {
                        "prod": {
                            "timeout_seconds": 77,
                            "prompt_preamble": "Profile: prod test.",
                            "require_explicit_confirmation": True,
                        }
                    }
                }
            }
        }),
        encoding="utf-8",
    )
    monkeypatch.setattr("shutil.which", fake_which)
    monkeypatch.setattr("subprocess.run", fake_run)

    result = CodingCliExecutor(tmp_path, "codex", profile="prod").run_task(
        Task(
            id="TASK-001",
            title="Profile",
            description="desc",
            planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
        ),
        [],
    )

    assert result.success
    assert "Profile: prod test." in captured["input"]
    assert "Ask for confirmation" in captured["input"]
    assert captured["timeout"] == 77
    assert captured["env"]["DEVCOUNCIL_AGENT_PROFILE"] == "prod"


def test_coding_cli_executor_rejects_unknown_profile(tmp_path, monkeypatch):
    monkeypatch.setattr("shutil.which", lambda _command: "/usr/bin/codex")

    result = CodingCliExecutor(tmp_path, "codex", profile="missing-profile").run_task(
        Task(
            id="TASK-001",
            title="Profile",
            description="desc",
            planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
        ),
        [],
    )

    assert not result.success
    assert "Unknown agent profile 'missing-profile'" in result.message


def test_coding_cli_executor_rejects_invalid_input_mode_before_launch(tmp_path, monkeypatch):
    import yaml

    launched = False

    def fake_run(*_args, **_kwargs):
        nonlocal launched
        launched = True
        return subprocess.CompletedProcess([], 0, stdout="", stderr="")

    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / ".devcouncil" / "config.yaml").write_text(
        yaml.safe_dump({
            "integrations": {
                "cli_agents": {
                    "agents": {
                        "badagent": {
                            "command": "badagent",
                            "input_mode": "not-a-mode",
                        }
                    }
                }
            }
        }),
        encoding="utf-8",
    )
    monkeypatch.setattr("shutil.which", lambda _command: "/usr/bin/badagent")
    monkeypatch.setattr("subprocess.run", fake_run)

    result = CodingCliExecutor(tmp_path, "badagent").run_task(
        Task(
            id="TASK-001",
            title="Bad mode",
            description="desc",
            planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
        ),
        [],
    )

    assert not result.success
    assert "Invalid input_mode 'not-a-mode'" in result.message
    assert launched is False


def test_coding_cli_executor_traces_timeout(tmp_path, monkeypatch):
    def fake_which(_command):
        return f"/usr/bin/{_command}"

    def fake_run(cmd, **kwargs):
        raise subprocess.TimeoutExpired(cmd=cmd, timeout=kwargs["timeout"])

    monkeypatch.setattr("shutil.which", fake_which)
    monkeypatch.setattr("subprocess.run", fake_run)

    result = CodingCliExecutor(tmp_path, "gemini", timeout_seconds=1).run_task(
        Task(
            id="TASK-001",
            title="Timeout",
            description="desc",
            planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
        ),
        [],
    )

    assert not result.success
    traces = (tmp_path / ".devcouncil" / "logs" / "traces.jsonl").read_text(encoding="utf-8")
    assert "agent_run_failed" in traces
    assert "timeout_seconds" in traces


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
