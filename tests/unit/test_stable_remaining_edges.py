import builtins
import json
import logging
import subprocess
from pathlib import Path
from types import SimpleNamespace

import pytest

from devcouncil.app.errors import ExecutionError
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.execution import shell_session as shell_mod
from devcouncil.execution import stop_gate
from devcouncil.execution.permissions import PermissionManager, PermissionPolicy
from devcouncil.execution.shell_session import GuardedShellSession, ShellWrappedBackend
from devcouncil.execution.task_runner import TaskRunner
from devcouncil.gating.checks.clean_git import CleanGitCheck
from devcouncil.gating.checks.planned_files_check import PlannedFilesCheck
from devcouncil.repo import sca as sca_mod
from devcouncil.storage import db as db_mod
from devcouncil.telemetry import logging_setup
from devcouncil.telemetry import traces


def _task(
    *,
    planned_files: list[PlannedFile] | None = None,
    allowed_commands: list[str] | None = None,
) -> Task:
    return Task(
        id="TASK-EDGE",
        title="Edge task",
        description="Exercise helper branches",
        planned_files=(
            planned_files
            if planned_files is not None
            else [PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")]
        ),
        allowed_commands=allowed_commands if allowed_commands is not None else ["pytest -q"],
    )


def test_stop_gate_snapshot_and_briefing_fallback_edges(monkeypatch, tmp_path):
    snapshot_path = stop_gate.compact_snapshot_path(tmp_path)
    snapshot_path.parent.mkdir(parents=True)
    snapshot_path.write_text("[]", encoding="utf-8")
    assert stop_gate.read_compact_snapshot(tmp_path) is None
    snapshot_path.write_text("{bad", encoding="utf-8")
    assert stop_gate.read_compact_snapshot(tmp_path) is None
    snapshot_path.write_text(json.dumps({"written_at": "not-a-date"}), encoding="utf-8")
    assert stop_gate.compact_snapshot_recent(tmp_path, 60) is False

    blocking = [SimpleNamespace(description=f"gap {idx}\nmore") for idx in range(6)]
    summary = stop_gate._blocking_gaps_summary(blocking)
    assert summary.startswith("6 blocking gap(s): gap 0;")
    assert "(+1 more)" in summary
    assert stop_gate._last_sentence("First. Second?") == "Second?"
    assert stop_gate._last_sentence("x" * 300) == "x" * 240

    monkeypatch.setattr(stop_gate, "active_task_id", lambda root: "TASK-EDGE")
    monkeypatch.setattr(stop_gate, "_phase_and_blocking_from_db", lambda root: ("TASK_EXECUTING", "1 blocking gap"))
    monkeypatch.setattr(stop_gate, "_last_stop_gate_event", lambda root: {"summary": "agent stopped"})
    monkeypatch.setattr(stop_gate, "_last_assistant_sentence", lambda root, session_id=None: "Continue here.")
    built = stop_gate.build_compact_snapshot(tmp_path, {"sessionId": 123})
    assert built["session_id"] == "123"
    assert built["last_stop_gate_event"]["summary"] == "agent stopped"

    snapshot_path.unlink()
    monkeypatch.setattr(stop_gate, "_status_line", lambda root: "DevCouncil - phase: TASK_EXECUTING. Use the CLI.")
    assert "Active task: TASK-EDGE" in stop_gate.session_briefing(tmp_path)
    db_brief = stop_gate.compact_briefing(tmp_path)
    assert "TASK_EXECUTING" in db_brief

    monkeypatch.setattr(stop_gate, "_status_line", lambda root: None)
    transcript_brief = stop_gate.compact_briefing(tmp_path, {"session_id": "s1"})
    assert transcript_brief.endswith("Continue here.")
    monkeypatch.setattr(stop_gate, "_last_assistant_sentence", lambda root, session_id=None: None)
    assert stop_gate.compact_briefing(tmp_path) is None


def test_task_runner_patch_command_and_error_edges(monkeypatch, tmp_path):
    runner = TaskRunner(tmp_path, PermissionManager(PermissionPolicy(), tmp_path))
    runner.patch_engine.apply_patch = lambda patch: True

    create_task = _task(planned_files=[PlannedFile(path="src/new.py", reason="new", allowed_change="create")])
    create_patch = """diff --git a/src/new.py b/src/new.py
--- /dev/null
+++ b/src/new.py
@@ -0,0 +1 @@
+new
"""
    assert runner.apply_patch(create_patch, create_task) is True

    delete_task = _task(planned_files=[PlannedFile(path="src/old.py", reason="old", allowed_change="delete")])
    delete_patch = """diff --git a/src/old.py b/src/old.py
--- a/src/old.py
+++ /dev/null
@@ -1 +0,0 @@
-old
"""
    assert runner._extract_patch_changes(delete_patch) == {"src/old.py": "delete"}
    with pytest.raises(ExecutionError):
        runner._extract_patch_changes("no file headers")

    def fake_run(argv, **kwargs):
        assert argv == ["pytest", "-q"]
        assert kwargs["timeout"] == 300
        return SimpleNamespace(returncode=0, stdout="ok api_key=abcdef1234567890", stderr="warn")

    monkeypatch.setattr("devcouncil.execution.task_runner.subprocess.run", fake_run)
    result = runner.run_command("pytest -q", _task())
    assert result.exit_code == 0
    assert "[REDACTED:generic_api_key]" in result.summary
    assert Path(result.stdout_path).exists()

    monkeypatch.setattr(
        "devcouncil.execution.task_runner.subprocess.run",
        lambda *a, **k: (_ for _ in ()).throw(OSError("boom")),
    )
    with pytest.raises(ExecutionError, match="Command execution failed"):
        runner.run_command("pytest -q", _task())

    blocked_path = tmp_path / "readonly"
    blocked_path.write_text("", encoding="utf-8")
    monkeypatch.setattr(Path, "write_text", lambda *a, **k: (_ for _ in ()).throw(OSError("no write")))
    writable_task = _task(planned_files=[PlannedFile(path="readonly", reason="edge", allowed_change="modify")])
    with pytest.raises(ExecutionError, match="Failed to write file"):
        runner.write_file("readonly", "content", writable_task)


def test_guarded_shell_session_backend_denial_and_evidence(monkeypatch, tmp_path):
    with pytest.raises(ValueError, match="Unknown shell backend"):
        ShellWrappedBackend("fish")
    monkeypatch.setattr(shell_mod.shutil, "which", lambda exe: None)
    with pytest.raises(ValueError, match="not installed"):
        ShellWrappedBackend("bash")

    task = _task(allowed_commands=["pytest -q"])
    session = GuardedShellSession(tmp_path, task)
    monkeypatch.setattr(shell_mod, "get_db", lambda root: None)
    assert session.run_one("rm -rf /") == 1

    class RaisingBackend(shell_mod.ShellBackend):
        def run_command(self, command, cwd, env=None):
            raise OSError("no executable")

    session.backend = RaisingBackend()
    assert session.run_one("pytest -q") == 1
    assert session.finish() is None

    recorded = {"commands": [], "evidence": []}

    class FakeCommandRepo:
        def __init__(self, session):
            pass

        def record(self, task_id, command, status, **kwargs):
            recorded["commands"].append((task_id, command, status, kwargs.get("exit_code")))

    class FakeEvidenceRepo:
        def __init__(self, session):
            pass

        def save_command_result(self, task_id, result):
            recorded["evidence"].append((task_id, result.command, result.summary))

    class PassingBackend(shell_mod.ShellBackend):
        def run_command(self, command, cwd, env=None):
            return SimpleNamespace(returncode=0, stdout="passed", stderr="")

    session.backend = PassingBackend()
    monkeypatch.setattr(shell_mod, "get_db", lambda root: SimpleNamespace(get_session=lambda: _session_cm()))
    monkeypatch.setattr(shell_mod, "ShellCommandRepository", FakeCommandRepo)
    monkeypatch.setattr(shell_mod, "EvidenceRepository", FakeEvidenceRepo)
    assert session.run_one("pytest -q") == 0
    assert recorded["commands"][-1] == ("TASK-EDGE", "pytest -q", "finished", 0)
    assert recorded["evidence"] == [("TASK-EDGE", "pytest -q", "passed")]


class _session_cm:
    def __enter__(self):
        return "session"

    def __exit__(self, exc_type, exc, tb):
        return False


def test_logging_setup_failure_and_repoint_edges(monkeypatch, tmp_path):
    root = logging.getLogger()
    saved_handlers = root.handlers[:]
    saved_level = root.level
    root.handlers[:] = [h for h in root.handlers if getattr(h, "_devcouncil_tag", None) is None]
    try:
        monkeypatch.setenv("DEVCOUNCIL_LOG_LEVEL", "INFO")
        log_path = logging_setup.configure_logging(tmp_path)
        console = next(h for h in root.handlers if getattr(h, "_devcouncil_tag", None) == "devcouncil.console")
        assert console.level == logging.INFO

        same_path = logging_setup.set_log_dir(tmp_path)
        assert same_path == tmp_path / logging_setup.LOG_RELATIVE_PATH

        monkeypatch.setattr(
            logging_setup,
            "RotatingFileHandler",
            lambda *a, **k: (_ for _ in ()).throw(OSError("readonly")),
        )
        assert logging_setup.set_log_dir(tmp_path / "other") is None

        file_handler = next(h for h in root.handlers if getattr(h, "_devcouncil_tag", None) == "devcouncil.file")
        root.removeHandler(file_handler)
        file_handler.close()
        assert logging_setup.configure_logging(tmp_path / "readonly") is None
        assert log_path.exists()
    finally:
        for handler in root.handlers:
            if getattr(handler, "_devcouncil_tag", None):
                handler.close()
        root.handlers[:] = saved_handlers
        root.setLevel(saved_level)


def test_trace_reader_invalid_lines_and_write_failure(monkeypatch, tmp_path):
    current = traces.TraceEvent.from_legacy(
        {"schema": traces.TRACE_SCHEMA_VERSION, "type": "current", "details": {"ok": True}}
    )
    assert current.type == "current"

    trace_file = tmp_path / ".devcouncil" / "logs" / "traces.jsonl"
    trace_file.parent.mkdir(parents=True)
    trace_file.write_text(
        "\n".join(
            [
                "",
                "{bad",
                json.dumps({"type": "legacy", "details": {"summary": "old", "task_id": "TASK-1"}}),
            ]
        )
        + "\n",
        encoding="utf-8",
    )
    events = list(traces.read_trace_events(tmp_path))
    assert [event.type for event in events] == ["legacy"]

    trace_file.write_text(json.dumps({"type": "complete", "details": {}}) + "\n{\"type\":", encoding="utf-8")
    events, cursor = traces.read_trace_events_since(tmp_path, 999)
    assert [event.type for event in events] == ["complete"]
    assert cursor == len(json.dumps({"type": "complete", "details": {}}) + "\n")

    real_open = builtins.open

    def raising_open(path, *args, **kwargs):
        if Path(path) == tmp_path / ".devcouncil" / "logs" / "traces.jsonl":
            raise OSError("no write")
        return real_open(path, *args, **kwargs)

    monkeypatch.setattr(builtins, "open", raising_open)
    event = traces.TraceLogger(tmp_path).log_event("write_failed", {})
    assert event.type == "write_failed"


def test_db_session_reset_and_cache_disposal_edges(tmp_path):
    class BadEngine:
        def dispose(self):
            raise OSError("already closed")

    db_mod._db_instances[tmp_path.resolve()] = SimpleNamespace(engine=BadEngine())
    db_mod._dedup_done.add(tmp_path.resolve())
    db_mod.reset_db_cache()
    assert db_mod._db_instances == {}
    assert db_mod._dedup_done == set()

    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    db = db_mod.get_db(tmp_path)
    with pytest.raises(ValueError, match="rollback me"):
        with db.get_session():
            raise ValueError("rollback me")
    db_mod.reset_db_cache()


def test_sca_detection_runner_parser_and_convenience_edges(monkeypatch, tmp_path):
    scanner = sca_mod.ScaScanner(tmp_path, which=lambda exe: "/bin/tool")
    assert scanner.available_auditors() == []
    (tmp_path / "requirements.txt").write_text("pkg==1\n", encoding="utf-8")
    assert sca_mod.ScaScanner(tmp_path, which=lambda exe: None).available_auditors() == []
    assert scanner._check_cached_lockfile("requirements.txt") is False
    fresh_scanner = sca_mod.ScaScanner(tmp_path, which=lambda exe: "/bin/tool")
    assert fresh_scanner._check_cached_lockfile("requirements.txt") is True
    (tmp_path / "requirements.txt").unlink()
    assert fresh_scanner._check_cached_lockfile("requirements.txt") is True

    monkeypatch.setattr(
        sca_mod.subprocess,
        "run",
        lambda *a, **k: (_ for _ in ()).throw(subprocess.TimeoutExpired(cmd="tool", timeout=1)),
    )
    assert sca_mod._default_runner(timeout=1)(["tool"], tmp_path).stderr == "timed out"

    unknown = sca_mod._Auditor(name="custom", executable="custom-tool", stack="any", lockfiles=("requirements.txt",))
    assert sca_mod.ScaScanner._argv_for(unknown) == ["custom-tool"]
    assert sca_mod.ScaScanner(tmp_path, auditor_runner=lambda *a: (_ for _ in ()).throw(RuntimeError("bad"))).scan() == []

    assert sca_mod._parse_pip_audit([{"name": "pkg", "version": "1", "vulns": [{"id": None, "severity": None}]}])[0].advisory_id == "UNKNOWN"
    assert sca_mod._parse_pip_audit({"dependencies": "bad"}) == []
    assert sca_mod._parse_npm_audit({"vulnerabilities": {"pkg": {"via": ["not-dict"]}}})[0].advisory_id == "UNKNOWN"
    assert sca_mod._parse_osv_scanner({"results": ["bad", {"packages": "bad"}]}) == []
    assert sca_mod._osv_severity({"severity": "critical"}) == "critical"
    assert sca_mod._osv_severity({"severity": []}) == "unknown"

    monkeypatch.setattr(sca_mod, "ScaScanner", lambda *a, **k: (_ for _ in ()).throw(RuntimeError("bad init")))
    assert sca_mod.scan_dependency_risks(tmp_path) == []


def test_gating_checks_clean_git_and_planned_files_edges(monkeypatch, tmp_path):
    clean = CleanGitCheck()
    monkeypatch.setattr(
        "devcouncil.gating.checks.clean_git.subprocess.check_output",
        lambda *a, **k: b"?? .devcouncil/state.sqlite\n M src/app.py\n",
    )
    dirty = clean.check(tmp_path, "TASK-1")
    assert dirty and dirty[0].id.endswith("DIRTY-GIT")

    monkeypatch.setattr(
        "devcouncil.gating.checks.clean_git.subprocess.check_output",
        lambda *a, **k: b"?? .devcouncil/state.sqlite\n M .gitignore\n",
    )
    assert clean.check(tmp_path, "TASK-1") == []

    monkeypatch.setattr(
        "devcouncil.gating.checks.clean_git.subprocess.check_output",
        lambda *a, **k: (_ for _ in ()).throw(FileNotFoundError()),
    )
    assert clean.check(tmp_path, "TASK-1")[0].id.endswith("NO-GIT")
    monkeypatch.setattr(
        "devcouncil.gating.checks.clean_git.subprocess.check_output",
        lambda *a, **k: (_ for _ in ()).throw(subprocess.CalledProcessError(1, ["git"])),
    )
    assert clean.check(tmp_path, "TASK-1")[0].id.endswith("GIT-ERROR")

    planned = PlannedFilesCheck()
    assert planned.check(_task(planned_files=[]))[0].id.endswith("NO-FILES")
    read_only = _task(planned_files=[PlannedFile(path="README.md", reason="context", allowed_change="read_only")])
    assert planned.check(read_only)[0].id.endswith("READ-ONLY")
