import json
import subprocess
import uuid

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


def test_openhands_executor_includes_repair_rules_on_repair_run(tmp_path, monkeypatch):
    captured = {}

    def fake_run(cmd, **kwargs):
        captured["cmd"] = cmd
        return subprocess.CompletedProcess(cmd, 0, stdout="", stderr="")

    monkeypatch.setattr("subprocess.run", fake_run)
    monkeypatch.setattr(
        "devcouncil.planning.correction_manifest.repair_prompt_prefix",
        lambda project_root, task_id: "# DevCouncil Correction Manifest\n\n{}\n\n## Repair rules (non-negotiable)\n".format(
            '{"root_cause": "stub detected"}'
        ),
    )
    task = Task(
        id="TASK-001",
        title="OpenHands",
        description="Fix the stub",
        planned_files=[
            PlannedFile(path="src/app.py", reason="logic", allowed_change="modify"),
        ],
    )

    result = OpenHandsExecutor(tmp_path).run_task(task, [])

    assert result.success
    prompt = (tmp_path / ".devcouncil" / "TASK-001-openhands-task.md").read_text(encoding="utf-8")
    assert "Repair rules (non-negotiable)" in prompt
    assert "stub detected" in prompt


def test_mini_swe_executor_includes_repair_rules_on_repair_run(tmp_path, monkeypatch):
    captured = {}

    def fake_run(cmd, **kwargs):
        captured["cmd"] = cmd
        return subprocess.CompletedProcess(cmd, 0, stdout="", stderr="")

    monkeypatch.setattr("subprocess.run", fake_run)
    monkeypatch.setattr(
        "devcouncil.planning.correction_manifest.repair_prompt_prefix",
        lambda project_root, task_id: "# DevCouncil Correction Manifest\n\n{}\n\n## Repair rules (non-negotiable)\n".format(
            '{"root_cause": "effort gap"}'
        ),
    )
    task = Task(
        id="TASK-001",
        title="mini",
        description="Complete the work",
        planned_files=[
            PlannedFile(path="src/app.py", reason="logic", allowed_change="modify"),
        ],
    )

    result = MiniSWEExecutor(tmp_path).run_task(task, [])

    assert result.success
    prompt = (tmp_path / ".devcouncil" / "TASK-001-mini-swe-task.md").read_text(encoding="utf-8")
    assert "Repair rules (non-negotiable)" in prompt
    assert "effort gap" in prompt


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
    assert captured["cmd"][:4] == ["cursor-agent", "--print", "--trust", "--workspace"]
    assert captured["cmd"][4] == str(tmp_path)
    assert captured["cmd"][5:7] == ["--output-format", "json"]
    assert "--resume" in captured["cmd"]
    assert captured["cmd"][captured["cmd"].index("--resume") + 1] == "chat-abc123"
    session = json.loads((tmp_path / ".devcouncil" / "integrations" / "cursor-session.json").read_text(encoding="utf-8"))
    assert session["chat_id"] == "chat-abc123"


def test_coding_cli_executor_claude_assigns_and_persists_session_id(tmp_path, monkeypatch):
    captured = {}

    def fake_which(_command):
        return f"/usr/bin/{_command}"

    def fake_run(cmd, **kwargs):
        captured["cmd"] = cmd
        return subprocess.CompletedProcess(cmd, 0, stdout="", stderr="")

    monkeypatch.setattr("shutil.which", fake_which)
    monkeypatch.setattr("subprocess.run", fake_run)

    task = Task(
        id="TASK-777",
        title="Claude",
        description="Implement feature",
        planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
    )

    executor = CodingCliExecutor(tmp_path, "claude")
    result = executor.run_task(task, [])

    assert result.success
    cmd = captured["cmd"]
    assert cmd[:4] == ["claude", "-p", "--permission-mode", "acceptEdits"]
    assert "--resume" not in cmd
    assert "--session-id" in cmd
    session_id = cmd[cmd.index("--session-id") + 1]
    uuid.UUID(session_id)  # a valid UUID or this raises
    assert executor.last_agent_session_id == session_id

    persisted = json.loads(
        (tmp_path / ".devcouncil" / "sessions" / "TASK-777-claude.json").read_text(encoding="utf-8")
    )
    assert persisted["session_id"] == session_id

    manifest = json.loads(
        (tmp_path / ".devcouncil" / "runs" / executor.last_run_id / "agent-run.json").read_text(encoding="utf-8")
    )
    assert manifest["agent_session_id"] == session_id


def test_coding_cli_executor_claude_resumes_prior_task_session(tmp_path, monkeypatch):
    captured = {}

    def fake_which(_command):
        return f"/usr/bin/{_command}"

    def fake_run(cmd, **kwargs):
        captured["cmd"] = cmd
        return subprocess.CompletedProcess(cmd, 0, stdout="", stderr="")

    monkeypatch.setattr("shutil.which", fake_which)
    monkeypatch.setattr("subprocess.run", fake_run)

    sessions = tmp_path / ".devcouncil" / "sessions"
    sessions.mkdir(parents=True)
    (sessions / "TASK-777-claude.json").write_text(
        json.dumps({"session_id": "11111111-2222-3333-4444-555555555555"}) + "\n",
        encoding="utf-8",
    )

    task = Task(
        id="TASK-777",
        title="Claude",
        description="Repair the earlier attempt",
        planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
    )

    executor = CodingCliExecutor(tmp_path, "claude")
    result = executor.run_task(task, [])

    assert result.success
    cmd = captured["cmd"]
    assert "--session-id" not in cmd
    assert "--resume" in cmd
    assert cmd[cmd.index("--resume") + 1] == "11111111-2222-3333-4444-555555555555"
    assert executor.last_agent_session_id == "11111111-2222-3333-4444-555555555555"


def test_coding_cli_executor_claude_captures_json_result(tmp_path, monkeypatch):
    captured = {}

    def fake_which(_command):
        return f"/usr/bin/{_command}"

    def fake_run(cmd, **kwargs):
        captured["cmd"] = cmd
        payload = {
            "type": "result",
            "session_id": "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
            "total_cost_usd": 0.0421,
            "num_turns": 3,
            "is_error": False,
            "result": "Implemented the feature and ran the tests.",
            "usage": {"input_tokens": 1200, "output_tokens": 340, "cache_read_input_tokens": 800},
        }
        return subprocess.CompletedProcess(cmd, 0, stdout=json.dumps(payload), stderr="")

    monkeypatch.setattr("shutil.which", fake_which)
    monkeypatch.setattr("subprocess.run", fake_run)

    task = Task(
        id="TASK-778",
        title="Claude",
        description="Implement feature",
        planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
    )

    executor = CodingCliExecutor(tmp_path, "claude")
    result = executor.run_task(task, [])

    assert result.success
    cmd = captured["cmd"]
    assert cmd[:6] == ["claude", "-p", "--permission-mode", "acceptEdits", "--output-format", "json"]

    # The reported session id wins over the pre-assigned one and is recorded.
    assert executor.last_agent_session_id == "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"

    manifest = json.loads(
        (tmp_path / ".devcouncil" / "runs" / executor.last_run_id / "agent-run.json").read_text(encoding="utf-8")
    )
    assert manifest["agent_session_id"] == "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
    # The resume pointer is rewritten to the id Claude actually reported (not the assigned
    # one), so a later --resume targets the real session.
    persisted = json.loads(
        (tmp_path / ".devcouncil" / "sessions" / "TASK-778-claude.json").read_text(encoding="utf-8")
    )
    assert persisted["session_id"] == "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
    agent_result = manifest["agent_result"]
    assert agent_result["total_cost_usd"] == 0.0421
    assert agent_result["num_turns"] == 3
    assert agent_result["input_tokens"] == 1200
    assert agent_result["output_tokens"] == 340


def test_render_claude_stream_event_summarizes_events():
    render = CodingCliExecutor._render_claude_stream_event

    # Assistant text is shown; thinking blocks are dropped from the same message.
    text_event = json.dumps({
        "type": "assistant",
        "message": {"content": [
            {"type": "thinking", "thinking": "internal reasoning"},
            {"type": "text", "text": "Editing the file"},
        ]},
    })
    assert render(text_event) == "Editing the file\n"

    # Tool use renders as an arrow line with the target.
    tool_event = json.dumps({
        "type": "assistant",
        "message": {"content": [
            {"type": "tool_use", "name": "Edit", "input": {"file_path": "src/app.py"}},
        ]},
    })
    assert render(tool_event) == "→ Edit src/app.py\n"

    # Advisor server tool surfaces as an Advising line.
    advisor_event = json.dumps({
        "type": "assistant",
        "message": {"content": [
            {"type": "server_tool_use", "name": "advisor", "input": {"model": "opus"}},
        ]},
    })
    assert render(advisor_event) == "Advising (opus)\n"

    # The terminal result event shows turns and cost.
    result_event = json.dumps({"type": "result", "num_turns": 2, "total_cost_usd": 0.0512})
    assert render(result_event) == "✓ 2 turns, $0.0512\n"

    # System / rate-limit noise is suppressed; non-JSON passes through verbatim.
    assert render(json.dumps({"type": "system", "subtype": "init"})) is None
    assert render("plain log line\n") == "plain log line\n"


def test_coding_cli_executor_claude_stream_captures_telemetry(tmp_path, monkeypatch):
    captured = {}
    ndjson = [
        json.dumps({"type": "system", "subtype": "init", "session_id": "sess-x"}) + "\n",
        json.dumps({"type": "assistant", "message": {"content": [{"type": "text", "text": "Working"}]}}) + "\n",
        json.dumps({"type": "assistant", "message": {"content": [
            {"type": "tool_use", "name": "Edit", "input": {"file_path": "src/app.py"}}]}}) + "\n",
        json.dumps({
            "type": "result",
            "session_id": "99999999-8888-7777-6666-555555555555",
            "total_cost_usd": 0.05,
            "num_turns": 2,
            "is_error": False,
            "usage": {"input_tokens": 100, "output_tokens": 50},
        }) + "\n",
    ]

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
            self.stdout = FakeStdout(ndjson)
            self.returncode = 0

        def poll(self):
            return 0 if self.stdout._index >= len(self.stdout._lines) else None

        def wait(self, timeout=None):
            return self.returncode

        def kill(self):
            return None

    def fake_popen(cmd, **kwargs):
        captured["cmd"] = cmd
        return FakeProcess()

    monkeypatch.setattr("shutil.which", lambda _c: f"/usr/bin/{_c}")
    monkeypatch.setattr("subprocess.Popen", fake_popen)

    task = Task(
        id="TASK-779",
        title="Claude",
        description="Implement feature",
        planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
    )

    result = CodingCliExecutor(tmp_path, "claude", stream_output=True).run_task(task, [])

    assert result.success
    cmd = captured["cmd"]
    assert cmd[:7] == ["claude", "-p", "--permission-mode", "acceptEdits", "--output-format", "stream-json", "--verbose"]

    run_dirs = list((tmp_path / ".devcouncil" / "runs").iterdir())
    transcript = run_dirs[0] / "transcript.txt"
    # Raw NDJSON is preserved in the transcript even though the console saw readable lines.
    assert '"tool_use"' in transcript.read_text(encoding="utf-8")

    manifest = json.loads((run_dirs[0] / "agent-run.json").read_text(encoding="utf-8"))
    assert manifest["agent_session_id"] == "99999999-8888-7777-6666-555555555555"
    assert manifest["agent_result"]["total_cost_usd"] == 0.05
    assert manifest["agent_result"]["input_tokens"] == 100


def test_coding_cli_executor_stream_mode_writes_transcript(tmp_path, monkeypatch):
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
            self.stdout = FakeStdout(["line one\n"])
            self.returncode = 0

        def poll(self):
            return 0 if self.stdout._index >= len(self.stdout._lines) else None

        def wait(self, timeout=None):
            return self.returncode

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
        planned_files=[
            PlannedFile(path="src/app.py", reason="logic", allowed_change="modify"),
        ],
    )

    result = CodingCliExecutor(tmp_path, "codex", stream_output=True).run_task(task, [])

    assert result.success
    run_dirs = list((tmp_path / ".devcouncil" / "runs").iterdir())
    assert run_dirs
    transcript = run_dirs[0] / "transcript.txt"
    assert transcript.exists()
    assert "line one" in transcript.read_text(encoding="utf-8")
    manifest = json.loads((run_dirs[0] / "agent-run.json").read_text(encoding="utf-8"))
    assert manifest.get("stream") is True
    assert manifest.get("transcript")


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

        def wait(self, timeout=None):
            return self.returncode

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
    assert captured["cmd"][5:7] == ["--output-format", "json"]
    assert captured["cmd"][7].endswith("TASK-001-cursor-task.md.")
    assert captured["input"] is None


def test_coding_cli_executor_cursor_yolo_profile_adds_force(tmp_path, monkeypatch):
    captured = {}

    def fake_which(command):
        if command in {"cursor-agent", "agent"}:
            return f"/usr/bin/{command}"
        return None

    def fake_run(cmd, **kwargs):
        captured["cmd"] = cmd
        return subprocess.CompletedProcess(cmd, 0, stdout="", stderr="")

    monkeypatch.setattr("shutil.which", fake_which)
    monkeypatch.setattr("subprocess.run", fake_run)

    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / ".devcouncil" / "config.yaml").write_text(
        "integrations:\n  cli_agents:\n    profiles:\n      yolo:\n        permission_mode: auto\n",
        encoding="utf-8",
    )

    task = Task(
        id="TASK-001",
        title="Cursor",
        description="Implement feature",
        planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
    )

    result = CodingCliExecutor(tmp_path, "cursor", profile="yolo").run_task(task, [])

    assert result.success
    assert captured["cmd"][1] == "--force"


def test_coding_cli_executor_cursor_stream_uses_stream_json(tmp_path, monkeypatch):
    captured = {}

    def fake_which(command):
        if command in {"cursor-agent", "agent"}:
            return f"/usr/bin/{command}"
        return None

    class FakeStdout:
        def readline(self):
            return ""

        def close(self):
            return None

    class FakeProcess:
        def __init__(self):
            self.stdin = None
            self.stdout = FakeStdout()
            self.returncode = 0

        def wait(self, timeout=None):
            return self.returncode

        def kill(self):
            return None

    def fake_popen(cmd, **kwargs):
        captured["cmd"] = cmd
        return FakeProcess()

    monkeypatch.setattr("shutil.which", fake_which)
    monkeypatch.setattr("subprocess.Popen", fake_popen)

    task = Task(
        id="TASK-001",
        title="Cursor",
        description="Implement feature",
        planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
    )

    result = CodingCliExecutor(tmp_path, "cursor", stream_output=True).run_task(task, [])

    assert result.success
    assert captured["cmd"][5:8] == ["--output-format", "stream-json", "--stream-partial-output"]


def test_coding_cli_executor_grok_headless_command_shape(tmp_path, monkeypatch):
    captured = {}

    def fake_which(command):
        if command == "grok":
            return "/usr/bin/grok"
        return None

    def fake_run(cmd, **kwargs):
        captured["cmd"] = cmd
        return subprocess.CompletedProcess(cmd, 0, stdout="", stderr="")

    monkeypatch.setattr("shutil.which", fake_which)
    monkeypatch.setattr("subprocess.run", fake_run)

    task = Task(
        id="TASK-001",
        title="Grok",
        description="Implement feature",
        planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
    )

    result = CodingCliExecutor(tmp_path, "grok").run_task(task, [])

    assert result.success
    assert captured["cmd"][:2] == ["grok", "-p"]
    assert captured["cmd"][3:5] == ["--directory", str(tmp_path)]
    assert captured["cmd"][5:7] == ["--output-format", "json"]
    assert any("TASK-001-grok-task.md" in part for part in captured["cmd"])


def test_coding_cli_executor_grok_yolo_permission_mode(tmp_path, monkeypatch):
    captured = {}

    def fake_which(command):
        if command == "grok":
            return "/usr/bin/grok"
        return None

    def fake_run(cmd, **kwargs):
        captured["cmd"] = cmd
        return subprocess.CompletedProcess(cmd, 0, stdout="", stderr="")

    monkeypatch.setattr("shutil.which", fake_which)
    monkeypatch.setattr("subprocess.run", fake_run)

    task = Task(
        id="TASK-001",
        title="Grok",
        description="Implement feature",
        planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
    )

    result = CodingCliExecutor(tmp_path, "grok", profile="yolo").run_task(task, [])

    assert result.success
    assert "--permission-mode" in captured["cmd"]
    assert captured["cmd"][captured["cmd"].index("--permission-mode") + 1] == "acceptEdits"


def test_coding_cli_executor_grok_resume_uses_project_session(tmp_path, monkeypatch):
    captured = {}

    def fake_which(command):
        if command == "grok":
            return "/usr/bin/grok"
        return None

    def fake_run(cmd, **kwargs):
        captured["cmd"] = cmd
        return subprocess.CompletedProcess(cmd, 0, stdout="", stderr="")

    monkeypatch.setattr("shutil.which", fake_which)
    monkeypatch.setattr("subprocess.run", fake_run)

    integrations = tmp_path / ".devcouncil" / "integrations"
    integrations.mkdir(parents=True)
    (integrations / "grok-session.json").write_text(
        json.dumps({"session_id": "sess-xyz"}),
        encoding="utf-8",
    )
    (tmp_path / ".devcouncil" / "config.yaml").write_text(
        "execution:\n  grok_resume_mode: project\n",
        encoding="utf-8",
    )

    task = Task(
        id="TASK-001",
        title="Grok",
        description="Implement feature",
        planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
    )

    result = CodingCliExecutor(tmp_path, "grok").run_task(task, [])

    assert result.success
    assert "--resume" in captured["cmd"]
    assert captured["cmd"][captured["cmd"].index("--resume") + 1] == "sess-xyz"


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


def test_coding_cli_executor_manifest_records_completion_metadata(tmp_path, monkeypatch):
    def fake_which(_command):
        return f"/usr/bin/{_command}"

    def fake_run(*args, **kwargs):
        return subprocess.CompletedProcess(args[0], 0, stdout="done\nsecret=redacted\n", stderr="")

    monkeypatch.setattr("shutil.which", fake_which)
    monkeypatch.setattr("subprocess.run", fake_run)

    task = Task(
        id="TASK-001",
        title="Codex",
        description="Implement feature",
        planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
    )

    result = CodingCliExecutor(tmp_path, "codex").run_task(task, [])

    assert result.success
    run_dirs = list((tmp_path / ".devcouncil" / "runs").iterdir())
    manifest = json.loads((run_dirs[0] / "agent-run.json").read_text(encoding="utf-8"))
    assert manifest["artifact_version"] == 1
    assert manifest["status"] == "finished"
    assert manifest["returncode"] == 0
    assert manifest["duration_seconds"] is not None
    assert manifest["stdout_preview"]


def test_coding_cli_executor_manifest_records_unexpected_exception(tmp_path, monkeypatch):
    def fake_which(_command):
        return f"/usr/bin/{_command}"

    def fake_run(*args, **kwargs):
        raise OSError("process launch failed")

    monkeypatch.setattr("shutil.which", fake_which)
    monkeypatch.setattr("subprocess.run", fake_run)

    task = Task(
        id="TASK-001",
        title="Codex",
        description="Implement feature",
        planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
    )

    result = CodingCliExecutor(tmp_path, "codex").run_task(task, [])

    assert not result.success
    run_dirs = list((tmp_path / ".devcouncil" / "runs").iterdir())
    manifest = json.loads((run_dirs[0] / "agent-run.json").read_text(encoding="utf-8"))
    assert manifest["status"] == "failed"
    assert manifest["returncode"] is None
    assert manifest["duration_seconds"] is not None
    assert manifest["stderr_preview"] == ["process launch failed"]


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


def _git(args, cwd):
    subprocess.run(["git", "-c", "user.email=t@t", "-c", "user.name=t", *args],
                   cwd=cwd, check=True, capture_output=True)


def test_coding_cli_scope_gate_reverts_orphan_keeps_planned(tmp_path):
    # The opt-in pre-verify scope gate reverts a file the task didn't authorize while
    # leaving the planned change intact (so non-hook executor drift never persists).
    subprocess.run(["git", "init", "-q"], cwd=tmp_path, check=True)
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "a.py").write_text("x = 1\n", encoding="utf-8")
    _git(["add", "-A"], tmp_path)
    _git(["commit", "-qm", "seed"], tmp_path)

    # Simulate the subprocess: modify the planned file AND drop an orphan file.
    (tmp_path / "src" / "a.py").write_text("x = 2\n", encoding="utf-8")
    (tmp_path / "src" / "orphan.py").write_text("y = 1\n", encoding="utf-8")

    task = Task(id="T1", title="t", description="d",
                planned_files=[PlannedFile(path="src/a.py", reason="x", allowed_change="modify")])
    reverted = CodingCliExecutor(tmp_path, "codex")._enforce_file_scope(task)

    assert [p for p, _ in reverted] == ["src/orphan.py"]
    assert not (tmp_path / "src" / "orphan.py").exists()        # orphan reverted (deleted)
    assert (tmp_path / "src" / "a.py").read_text() == "x = 2\n"  # planned change preserved


def test_coding_cli_scope_gate_off_by_default(tmp_path):
    # Without opting in, the gate is inert (no config -> disabled).
    assert CodingCliExecutor(tmp_path, "codex")._scope_enforcement_enabled() is False


def test_coding_cli_scope_gate_keeps_planned_file_deletion(tmp_path):
    # Deleting a file the task OWNS is within scope -> must NOT be restored.
    subprocess.run(["git", "init", "-q"], cwd=tmp_path, check=True)
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "a.py").write_text("x = 1\n", encoding="utf-8")
    (tmp_path / "src" / "gone.py").write_text("g = 1\n", encoding="utf-8")
    _git(["add", "-A"], tmp_path)
    _git(["commit", "-qm", "seed"], tmp_path)

    (tmp_path / "src" / "gone.py").unlink()  # executor deletes its own planned file
    task = Task(id="T1", title="t", description="d", planned_files=[
        PlannedFile(path="src/a.py", reason="x", allowed_change="modify"),
        PlannedFile(path="src/gone.py", reason="remove", allowed_change="modify"),
    ])
    reverted = CodingCliExecutor(tmp_path, "codex")._enforce_file_scope(task)
    assert reverted == []                               # nothing out of scope
    assert not (tmp_path / "src" / "gone.py").exists()  # deletion preserved


def test_coding_cli_scope_gate_never_reverts_scaffolding(tmp_path):
    # DevCouncil-managed scaffolding (AGENTS.md) must survive the gate even when it is
    # not in planned_files and not yet in the baseline snapshot.
    subprocess.run(["git", "init", "-q"], cwd=tmp_path, check=True)
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "a.py").write_text("x = 1\n", encoding="utf-8")
    _git(["add", "-A"], tmp_path)
    _git(["commit", "-qm", "seed"], tmp_path)

    (tmp_path / "AGENTS.md").write_text("# guide\n", encoding="utf-8")
    task = Task(id="T1", title="t", description="d",
                planned_files=[PlannedFile(path="src/a.py", reason="x", allowed_change="modify")])
    reverted = CodingCliExecutor(tmp_path, "codex")._enforce_file_scope(task)
    assert reverted == []
    assert (tmp_path / "AGENTS.md").exists()


def test_coding_cli_revert_handles_repo_with_no_commits(tmp_path):
    # In a repo with NO commits, an orphan new file must still be removed (no HEAD to
    # checkout) — the gate must not silently leave it on disk.
    subprocess.run(["git", "init", "-q"], cwd=tmp_path, check=True)
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "orphan.py").write_text("y = 1\n", encoding="utf-8")
    _git(["add", "-A"], tmp_path)  # staged but never committed -> no HEAD

    task = Task(id="T1", title="t", description="d",
                planned_files=[PlannedFile(path="src/a.py", reason="x", allowed_change="modify")])
    reverted = CodingCliExecutor(tmp_path, "codex")._enforce_file_scope(task)
    assert [p for p, _ in reverted] == ["src/orphan.py"]
    assert not (tmp_path / "src" / "orphan.py").exists()


def test_openhands_retries_transient_failure(tmp_path, monkeypatch):
    calls = {"count": 0}

    def fake_run(cmd, **kwargs):
        calls["count"] += 1
        if calls["count"] == 1:
            return subprocess.CompletedProcess(cmd, 1, stdout="", stderr="rate limit exceeded")
        return subprocess.CompletedProcess(cmd, 0, stdout="ok", stderr="")

    monkeypatch.setattr("subprocess.run", fake_run)
    monkeypatch.setattr("devcouncil.executors.transient_retry.time.sleep", lambda _s: None)
    task = Task(
        id="TASK-001",
        title="OpenHands retry",
        description="desc",
        planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
    )

    result = OpenHandsExecutor(tmp_path).run_task(task, [])

    assert result.success
    assert calls["count"] == 2
