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


def test_coding_cli_executor_updates_gitignore_for_runtime_artifacts(tmp_path, monkeypatch):
    gitignore_path = tmp_path / ".gitignore"
    gitignore_path.write_text("existing-rule\n", encoding="utf-8")

    def fake_which(_command):
        return f"/usr/bin/{_command}"

    def fake_run(cmd, **kwargs):
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
    content = gitignore_path.read_text(encoding="utf-8")
    assert "existing-rule" in content
    for expected in [
        ".devcouncil/*",
        "!.devcouncil/config.yaml",
        ".agents/",
        ".codex/",
        ".aider*",
        "logs/",
        "tmp/",
        "scratch/",
        "dumps/",
        "*.log",
        "*.dump",
    ]:
        assert expected in content


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
        ("opencode", ["opencode", "run", "--file"]),
        ("opencode-cli", ["opencode", "run", "--file"]),
        ("antigravity", ["agy", "--print", "--print-timeout", "30m"]),
        ("antigravity-cli", ["agy", "--print", "--print-timeout", "30m"]),
        ("agy", ["agy", "--print", "--print-timeout", "30m"]),
        ("agy-cli", ["agy", "--print", "--print-timeout", "30m"]),
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
                        "custombot": {
                            "command": "custombot",
                            "args": ["run"],
                            "input_mode": "prompt-file",
                            "prompt_arg": "--prompt-file",
                            "env": {"CUSTOMBOT_MODE": "devcouncil"},
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

    result = CodingCliExecutor(tmp_path, "custombot").run_task(task, [])

    assert result.success
    assert captured["cmd"][:3] == ["custombot", "run", "--prompt-file"]
    assert captured["cmd"][3].endswith("TASK-001-custombot-task.md")
    assert captured["input"] is None
    assert captured["env"]["CUSTOMBOT_MODE"] == "devcouncil"


def test_coding_cli_executor_builtin_opencode_uses_attached_prompt_file(tmp_path, monkeypatch):
    captured = {}

    def fake_which(_command):
        return f"/usr/bin/{_command}"

    def fake_run(cmd, **kwargs):
        captured["cmd"] = cmd
        captured["input"] = kwargs.get("input")
        return subprocess.CompletedProcess(cmd, 0, stdout="", stderr="")

    monkeypatch.setattr("shutil.which", fake_which)
    monkeypatch.setattr("subprocess.run", fake_run)

    task = Task(
        id="TASK-001",
        title="OpenCode",
        description="Implement feature",
        planned_files=[
            PlannedFile(path="src/app.py", reason="logic", allowed_change="modify"),
        ],
    )

    result = CodingCliExecutor(tmp_path, "opencode").run_task(task, [])

    assert result.success
    assert captured["cmd"][:3] == ["opencode", "run", "--file"]
    assert captured["cmd"][3].endswith("TASK-001-opencode-task.md")
    assert captured["cmd"][4] == "Execute the DevCouncil task described in the attached prompt file."
    assert captured["input"] is None


def test_coding_cli_executor_builtin_antigravity_uses_task_file_prompt(tmp_path, monkeypatch):
    captured = {}

    def fake_which(_command):
        return f"/usr/bin/{_command}"

    def fake_run(cmd, **kwargs):
        captured["cmd"] = cmd
        captured["input"] = kwargs.get("input")
        return subprocess.CompletedProcess(cmd, 0, stdout="", stderr="")

    monkeypatch.setattr("shutil.which", fake_which)
    monkeypatch.setattr("subprocess.run", fake_run)

    task = Task(
        id="TASK-001",
        title="Antigravity",
        description="Implement feature",
        planned_files=[
            PlannedFile(path="src/app.py", reason="logic", allowed_change="modify"),
        ],
    )

    result = CodingCliExecutor(tmp_path, "agy").run_task(task, [])

    assert result.success
    assert captured["cmd"][:4] == ["agy", "--print", "--print-timeout", "30m"]
    assert captured["cmd"][4].endswith("TASK-001-antigravity-task.md.")
    assert "Read and execute the DevCouncil task prompt" in captured["cmd"][4]
    assert captured["input"] is None
    task_file = tmp_path / ".devcouncil" / "TASK-001-antigravity-task.md"
    assert task_file.exists()


def test_detect_available_coding_cli_prefers_probe_order(tmp_path, monkeypatch):
    def fake_which(command):
        return "/usr/bin/aider" if command == "aider" else None

    monkeypatch.setattr("shutil.which", fake_which)
    from devcouncil.executors.agent_registry import detect_available_coding_cli

    assert detect_available_coding_cli(tmp_path, probe_order=("codex", "aider")) == "aider"


def test_resolve_automated_executor_falls_back_to_detected_cli(tmp_path, monkeypatch):
    def fake_which(command):
        return "/usr/bin/gemini" if command == "gemini" else None

    monkeypatch.setattr("shutil.which", fake_which)
    from devcouncil.executors.agent_registry import resolve_automated_executor

    assert resolve_automated_executor(tmp_path, None) == "gemini"


def test_coding_cli_executor_cursor_resume_uses_create_chat(tmp_path, monkeypatch):
    captured = {}

    def fake_which(command):
        if command in {"cursor-agent", "agent"}:
            return f"/usr/bin/{command}"
        return None

    def fake_run(cmd, **kwargs):
        if cmd[1:] == ["create-chat"]:
            return subprocess.CompletedProcess(cmd, 0, stdout="chat-abc123\n", stderr="")
        captured["cmd"] = cmd
        captured["input"] = kwargs.get("input")
        return subprocess.CompletedProcess(cmd, 0, stdout="", stderr="")

    monkeypatch.setattr("shutil.which", fake_which)
    monkeypatch.setattr("subprocess.run", fake_run)

    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / ".devcouncil" / "config.yaml").write_text(
        "execution:\n  cursor_resume_mode: project\n",
        encoding="utf-8",
    )

    task = Task(
        id="TASK-001",
        title="Cursor",
        description="Implement feature",
        planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
    )

    result = CodingCliExecutor(tmp_path, "cursor").run_task(task, [])

    assert result.success
    assert captured["cmd"][:6] == ["cursor-agent", "--print", "--trust", "--workspace", str(tmp_path), "--resume"]
    assert captured["cmd"][6] == "chat-abc123"
    session = json.loads((tmp_path / ".devcouncil" / "integrations" / "cursor-session.json").read_text(encoding="utf-8"))
    assert session["chat_id"] == "chat-abc123"


def test_coding_cli_executor_stream_mode_uses_live_output(tmp_path, monkeypatch):
    captured = {}

    class FakeStdout:
        def __init__(self, lines):
            self._lines = list(lines)
            self._index = 0

        def readline(self):
            if self._index >= len(self._lines):
                return ""
            line = self._lines[self._index]
            self._index += 1
            return line

        def close(self):
            return None

    class FakeProcess:
        def __init__(self):
            self.stdin = None
            self.stdout = FakeStdout(["line one\n", "line two\n"])
            self.returncode = 0

        def poll(self):
            return 0 if self.stdout._index >= len(self.stdout._lines) else None

        def kill(self):
            return None

    def fake_popen(cmd, **kwargs):
        captured["cmd"] = cmd
        return FakeProcess()

    def fake_which(_command):
        return f"/usr/bin/{_command}"

    monkeypatch.setattr("shutil.which", fake_which)
    monkeypatch.setattr("subprocess.Popen", fake_popen)

    task = Task(
        id="TASK-001",
        title="Codex",
        description="Implement feature",
        planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
    )

    result = CodingCliExecutor(tmp_path, "codex", stream_output=True).run_task(task, [])

    assert result.success
    assert captured["cmd"][:2] == ["codex", "exec"]


def test_coding_cli_executor_builtin_cursor_uses_cursor_agent_print_mode(tmp_path, monkeypatch):
    captured = {}

    def fake_which(command):
        if command in {"cursor-agent", "agent"}:
            return f"/usr/bin/{command}"
        return None

    def fake_run(cmd, **kwargs):
        captured["cmd"] = cmd
        captured["input"] = kwargs.get("input")
        return subprocess.CompletedProcess(cmd, 0, stdout="", stderr="")

    monkeypatch.setattr("shutil.which", fake_which)
    monkeypatch.setattr("subprocess.run", fake_run)

    task = Task(
        id="TASK-001",
        title="Cursor",
        description="Implement feature",
        planned_files=[
            PlannedFile(path="src/app.py", reason="logic", allowed_change="modify"),
        ],
    )

    result = CodingCliExecutor(tmp_path, "cursor-agent").run_task(task, [])

    assert result.success
    assert captured["cmd"][0] == "cursor-agent"
    assert captured["cmd"][:4] == ["cursor-agent", "--print", "--trust", "--workspace"]
    assert captured["cmd"][4] == str(tmp_path)
    assert captured["cmd"][5].endswith("TASK-001-cursor-task.md.")
    assert captured["input"] is None


def test_coding_cli_executor_builtin_aider_uses_message_argument(tmp_path, monkeypatch):
    captured = {}

    def fake_which(_command):
        return f"/usr/bin/{_command}"

    def fake_run(cmd, **kwargs):
        captured["cmd"] = cmd
        captured["input"] = kwargs.get("input")
        return subprocess.CompletedProcess(cmd, 0, stdout="", stderr="")

    monkeypatch.setattr("shutil.which", fake_which)
    monkeypatch.setattr("subprocess.run", fake_run)

    task = Task(
        id="TASK-001",
        title="Aider",
        description="Implement feature",
        planned_files=[
            PlannedFile(path="src/app.py", reason="logic", allowed_change="modify"),
        ],
    )

    result = CodingCliExecutor(tmp_path, "aider").run_task(task, [])

    assert result.success
    assert captured["cmd"][:4] == ["aider", "--yes", "--no-show-model-warnings", "--message"]
    assert "TASK-001" in captured["cmd"][4]
    assert captured["input"] is None


def test_coding_cli_executor_warp_writes_oz_mcp_server_map(tmp_path, monkeypatch):
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
        title="Warp",
        description="Implement feature",
        planned_files=[
            PlannedFile(path="src/app.py", reason="logic", allowed_change="modify"),
        ],
    )

    result = CodingCliExecutor(tmp_path, "warp").run_task(task, [])

    assert result.success
    assert "--mcp" in captured["cmd"]
    config_path = tmp_path / ".devcouncil" / "integrations" / "warp-mcp.json"
    data = json.loads(config_path.read_text(encoding="utf-8"))
    assert data["devcouncil"]["command"] == "devcouncil"
    assert data["devcouncil"]["args"] == ["mcp-server"]
    assert data["devcouncil"]["env"]["DEVCOUNCIL_PROJECT_ROOT"] == str(tmp_path)
    assert "mcpServers" not in data


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
                        "opencode": {
                            "command": "custom-opencode",
                            "input_mode": "stdin",
                        },
                        "agy": {
                            "command": "custom-agy",
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
    assert specs["opencode"].command == "opencode"
    assert specs["antigravity"].command == "agy"
    assert "oz" not in specs
    assert "agy" not in specs


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
