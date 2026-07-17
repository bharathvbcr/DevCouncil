import asyncio
import importlib.metadata
import json
import runpy
from contextlib import contextmanager
from types import SimpleNamespace

import pytest
import typer
from typer.testing import CliRunner

from devcouncil.artifacts.graph import ArtifactGraph
from devcouncil.cli.commands import (
    baseline as baseline_cmd,
    cost as cost_cmd,
    graph_cmd,
    mcp_server as mcp_server_cmd,
    rollback as rollback_cmd,
    shell as shell_cmd,
    status as status_cmd,
    version as version_cmd,
    watch_fs as watch_fs_cmd,
)
from devcouncil.cli.commands.init import initialize_project
from devcouncil.cli.main import app
from devcouncil.domain.gap import Gap
from devcouncil.domain.requirement import AcceptanceCriterion, Requirement
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import RequirementRepository, TaskRepository

runner = CliRunner()


def _status_payload(blocking_count=0, cost_groups=None):
    return {
        "initialized": True,
        "phase": "TASK_BLOCKED" if blocking_count else "PLAN_APPROVED",
        "coverage_summary": {
            "total_requirements": 3,
            "requirements_without_tasks": 1,
            "total_tasks": 2,
            "tasks_without_requirements": 0,
            "total_ac": 4,
            "ac_without_evidence": 1,
            "total_gaps": blocking_count,
            "blocking_gaps": blocking_count,
        },
        "total_cost": 1.23456,
        "cost_by_task": cost_groups or {},
        "task_status_counts": {"planned": 1, "verified": 2},
        "blocking_gaps": [
            {"id": f"GAP-{idx}", "description": f"blocking gap {idx}"}
            for idx in range(1, blocking_count + 1)
        ],
        "live_review": {
            "cards": {"critical_open": 2},
            "blocking_cards": [{"id": "card-1"}],
            "pending_signals": 3,
        },
    }


def test_status_plain_output_renders_tables_warnings_and_failures(monkeypatch, tmp_path):
    monkeypatch.setattr(
        status_cmd,
        "_status_payload",
        lambda root: _status_payload(
            blocking_count=6,
            cost_groups={
                "TASK-LOW": {"cost": 0.25, "calls": 1},
                "TASK-HIGH": {"cost": 2.0, "calls": 4},
            },
        ),
    )

    result = runner.invoke(app, ["status", "--fail-on-blocking", "--project-root", str(tmp_path)])

    assert result.exit_code == 1
    assert "DevCouncil Status" in result.output
    assert "Task Summary" in result.output
    assert "Cost by Task" in result.output
    assert "TASK-HIGH" in result.output
    assert "WARNING: 6 blocking gap(s)" in result.output
    assert "... and 1 more" in result.output


def test_status_plain_output_handles_uninitialized_project(monkeypatch, tmp_path):
    monkeypatch.setattr(
        status_cmd,
        "_status_payload",
        lambda root: {"initialized": False, "phase": "UNINITIALIZED"},
    )

    result = runner.invoke(app, ["status", "--project-root", str(tmp_path)])

    assert result.exit_code == 0
    assert "DevCouncil state is not available" in result.output


def test_status_json_and_empty_optional_sections(monkeypatch, tmp_path):
    payload = _status_payload(blocking_count=0, cost_groups={})
    payload["task_status_counts"] = {}
    monkeypatch.setattr(status_cmd, "_status_payload", lambda root: payload)

    as_json = runner.invoke(app, ["status", "--json", "--project-root", str(tmp_path)])
    as_plain = runner.invoke(app, ["status", "--project-root", str(tmp_path)])

    assert as_json.exit_code == 0
    assert json.loads(as_json.output) == payload
    assert as_plain.exit_code == 0
    assert "DevCouncil Status" in as_plain.output
    assert "Task Summary" not in as_plain.output
    assert "Cost by Task" not in as_plain.output
    assert "WARNING" not in as_plain.output


def test_status_payload_reports_uninitialized_when_database_missing(monkeypatch, tmp_path):
    monkeypatch.setattr(status_cmd, "initialize_project", lambda root, quiet=True: None)
    monkeypatch.setattr(status_cmd, "get_db", lambda root: None)

    assert status_cmd._status_payload(tmp_path) == {
        "initialized": False,
        "phase": "UNINITIALIZED",
    }


def test_status_payload_assembles_initialized_project(monkeypatch, tmp_path):
    monkeypatch.setattr(status_cmd, "initialize_project", lambda root, quiet=True: None)
    monkeypatch.setattr(status_cmd, "get_db", lambda root: _FakeDb())
    monkeypatch.setattr(status_cmd, "compute_phase", lambda graph, persisted: f"phase:{persisted}")
    monkeypatch.setattr(
        status_cmd,
        "group_cost",
        lambda root: {
            "total_cost": 0.75,
            "by_task": {"TASK-001": {"cost": 0.75, "calls": 2}},
        },
    )
    monkeypatch.setattr(
        status_cmd,
        "live_review_summary",
        lambda root: {"cards": {"critical_open": 0}, "blocking_cards": [], "pending_signals": 0},
    )

    class FakeGap:
        def model_dump(self):
            return {"id": "GAP-1", "description": "gap"}

    class FakeGraph:
        tasks = {
            "TASK-001": SimpleNamespace(status="planned"),
            "TASK-002": SimpleNamespace(status="verified"),
            "TASK-003": SimpleNamespace(status="verified"),
        }

        def coverage_summary(self):
            return {"summary": True}

        def blocking_gaps(self):
            return [FakeGap()]

    class FakeGraphRepository:
        def __init__(self, session):
            self.session = session

        def load_graph(self):
            return FakeGraph()

    class FakeStateRepository:
        def __init__(self, session):
            self.session = session

        def get_state(self):
            return SimpleNamespace(current_phase="TASK_EXECUTING")

    monkeypatch.setattr(status_cmd, "ArtifactGraphRepository", FakeGraphRepository)
    monkeypatch.setattr(status_cmd, "StateRepository", FakeStateRepository)

    payload = status_cmd._status_payload(tmp_path)

    assert payload["initialized"] is True
    assert payload["phase"] == "phase:TASK_EXECUTING"
    assert payload["coverage_summary"] == {"summary": True}
    assert payload["total_cost"] == 0.75
    assert payload["task_status_counts"] == {"planned": 1, "verified": 2}
    assert payload["blocking_gaps"] == [{"id": "GAP-1", "description": "gap"}]


def _seed_show_task(tmp_path):
    initialize_project(tmp_path, quiet=True)
    db = get_db(tmp_path)
    assert db is not None
    with db.get_session() as session:
        RequirementRepository(session).save(
            Requirement(
                id="REQ-001",
                title="Requirement one",
                description="desc",
                priority="high",
                source="user",
                acceptance_criteria=[
                    AcceptanceCriterion(
                        id="AC-001",
                        description="works",
                        verification_method="unit_test",
                    )
                ],
            )
        )
        TaskRepository(session).save(
            Task(
                id="TASK-001",
                title="Build it",
                description="Implement the feature",
                requirement_ids=["REQ-001", "REQ-MISSING"],
                planned_files=[
                    PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")
                ],
                expected_tests=["python -m pytest tests/test_app.py"],
            )
        )


def test_show_json_includes_linked_requirements_only(tmp_path):
    _seed_show_task(tmp_path)

    result = runner.invoke(app, ["show", "TASK-001", "--json", "--project-root", str(tmp_path)])

    assert result.exit_code == 0
    payload = json.loads(result.output)
    assert payload["task"]["id"] == "TASK-001"
    assert [req["id"] for req in payload["linked_requirements"]] == ["REQ-001"]


def test_show_plain_output_lists_missing_requirements_files_and_tests(tmp_path):
    _seed_show_task(tmp_path)

    result = runner.invoke(app, ["show", "TASK-001", "--project-root", str(tmp_path)])

    assert result.exit_code == 0
    assert "Task TASK-001: Build it" in result.output
    assert "Requirement one" in result.output
    assert "REQ-MISSING" in result.output
    assert "Requirement not found" in result.output
    assert "src/app.py (modify): logic" in result.output
    assert "python -m pytest tests/test_app.py" in result.output


def test_show_exits_for_missing_task_and_missing_db(monkeypatch, tmp_path):
    initialize_project(tmp_path, quiet=True)
    missing = runner.invoke(app, ["show", "NOPE", "--project-root", str(tmp_path)])
    assert missing.exit_code == 1
    assert "Task NOPE not found" in missing.output

    monkeypatch.setattr("devcouncil.cli.commands.show.initialize_project", lambda root, quiet=True: None)
    monkeypatch.setattr("devcouncil.cli.commands.show.get_db", lambda root: None)
    no_db = runner.invoke(app, ["show", "TASK-001", "--project-root", str(tmp_path)])
    assert no_db.exit_code == 1

    from devcouncil.cli.commands import show as show_cmd

    show_cmd.show(SimpleNamespace(invoked_subcommand="child"), "TASK-001", project_root=tmp_path)


def test_cost_show_json_and_tables(monkeypatch, tmp_path):
    summary = {
        "total_cost": 3.5,
        "total_calls": 5,
        "by_task": {
            "TASK-LOW": {"cost": 0.5, "calls": 1, "prompt_tokens": 10, "completion_tokens": 20},
            "TASK-HIGH": {"cost": 3.0, "calls": 4, "prompt_tokens": 30, "completion_tokens": 40},
        },
        "by_run": {
            "run-1": {"cost": 3.5, "calls": 5, "prompt_tokens": 40, "completion_tokens": 60}
        },
    }
    monkeypatch.setattr(cost_cmd, "group_cost", lambda root: summary)

    as_json = runner.invoke(app, ["cost", "show", "--json", "--project-root", str(tmp_path)])
    as_table = runner.invoke(app, ["cost", "show", "--project-root", str(tmp_path)])

    assert as_json.exit_code == 0
    assert json.loads(as_json.output) == summary
    assert as_table.exit_code == 0
    assert "Total Cost" in as_table.output
    assert "Cost by Task" in as_table.output
    assert "TASK-HIGH" in as_table.output
    assert "Cost by Run" in as_table.output


def test_cost_show_skips_empty_group_tables(monkeypatch, tmp_path):
    monkeypatch.setattr(
        cost_cmd,
        "group_cost",
        lambda root: {"total_cost": 0.0, "total_calls": 0, "by_task": {}, "by_run": {}},
    )

    result = runner.invoke(app, ["cost", "show", "--project-root", str(tmp_path)])

    assert result.exit_code == 0
    assert "Total Cost" in result.output
    assert "Cost by Task" not in result.output


def test_version_command_reports_installed_and_unknown_versions(monkeypatch):
    monkeypatch.setattr(version_cmd.importlib.metadata, "version", lambda package: "9.9.9")
    installed = runner.invoke(app, ["version"])
    assert installed.exit_code == 0
    assert "9.9.9" in installed.output

    def raise_missing(package):
        raise importlib.metadata.PackageNotFoundError(package)

    monkeypatch.setattr(version_cmd.importlib.metadata, "version", raise_missing)
    missing = runner.invoke(app, ["version"])
    assert missing.exit_code == 0
    assert "unknown" in missing.output

    version_cmd.version(SimpleNamespace(invoked_subcommand="child"))


def test_tasks_json_plain_empty_and_missing_db(monkeypatch, tmp_path):
    initialize_project(tmp_path, quiet=True)
    db = get_db(tmp_path)
    assert db is not None

    empty = runner.invoke(app, ["tasks", "--project-root", str(tmp_path)])
    assert empty.exit_code == 0
    assert "No tasks found" in empty.output

    with db.get_session() as session:
        TaskRepository(session).save(
            Task(
                id="TASK-001",
                title="Task title",
                description="desc",
                requirement_ids=["REQ-001", "REQ-002"],
            )
        )

    as_json = runner.invoke(app, ["tasks", "--json", "--project-root", str(tmp_path)])
    assert as_json.exit_code == 0
    assert json.loads(as_json.output)["tasks"][0]["id"] == "TASK-001"

    as_table = runner.invoke(app, ["tasks", "--project-root", str(tmp_path)])
    assert as_table.exit_code == 0
    assert "DevCouncil Tasks" in as_table.output
    assert "REQ-001, REQ-002" in as_table.output

    monkeypatch.setattr("devcouncil.cli.commands.tasks.initialize_project", lambda root, quiet=True: None)
    monkeypatch.setattr("devcouncil.cli.commands.tasks.get_db", lambda root: None)
    no_db = runner.invoke(app, ["tasks", "--project-root", str(tmp_path)])
    assert no_db.exit_code == 1
    assert "state is unavailable" in no_db.output

    from devcouncil.cli.commands import tasks as tasks_cmd

    tasks_cmd.tasks(SimpleNamespace(invoked_subcommand="child"), project_root=tmp_path)


class _FakeDb:
    @contextmanager
    def get_session(self):
        yield object()


def _patch_shell_task(monkeypatch, task):
    monkeypatch.setattr(shell_cmd, "initialize_project", lambda root, quiet=True: None)
    monkeypatch.setattr(shell_cmd, "get_db", lambda root: _FakeDb())

    class FakeTaskRepository:
        def __init__(self, session):
            self.session = session

        def get_by_id(self, task_id):
            return task

    monkeypatch.setattr(shell_cmd, "TaskRepository", FakeTaskRepository)


def test_shell_exits_when_db_or_task_is_missing(monkeypatch, tmp_path):
    monkeypatch.setattr(shell_cmd, "initialize_project", lambda root, quiet=True: None)
    monkeypatch.setattr(shell_cmd, "get_db", lambda root: None)
    no_db = runner.invoke(app, ["shell", "TASK-001", "--project-root", str(tmp_path)])
    assert no_db.exit_code == 1
    assert "not initialized" in no_db.output

    _patch_shell_task(monkeypatch, None)
    missing_task = runner.invoke(app, ["shell", "TASK-404", "--project-root", str(tmp_path)])
    assert missing_task.exit_code == 1
    assert "TASK-404 not found" in missing_task.output


def test_shell_reports_runner_creation_and_start_errors(monkeypatch, tmp_path):
    task = Task(id="TASK-001", title="Task", description="desc")
    _patch_shell_task(monkeypatch, task)

    class RaisingConstructor:
        def __init__(self, root, task, shell="auto"):
            raise ValueError("bad shell")

    monkeypatch.setattr(shell_cmd, "GuardedShellSession", RaisingConstructor)
    bad_shell = runner.invoke(app, ["shell", "TASK-001", "--project-root", str(tmp_path)])
    assert bad_shell.exit_code == 2
    assert "bad shell" in bad_shell.output

    class RaisingStart:
        def __init__(self, root, task, shell="auto"):
            pass

        def start(self, force=False):
            raise ValueError("lease held")

    monkeypatch.setattr(shell_cmd, "GuardedShellSession", RaisingStart)
    lease = runner.invoke(app, ["shell", "TASK-001", "--project-root", str(tmp_path)])
    assert lease.exit_code == 2
    assert "lease held" in lease.output
    assert "--force" in lease.output


def test_shell_interactive_loop_runs_nonblank_commands_and_finishes(monkeypatch, tmp_path):
    task = Task(id="TASK-001", title="Task", description="desc")
    _patch_shell_task(monkeypatch, task)
    calls = {"commands": [], "started": None, "finished": False}

    class FakeRunner:
        def __init__(self, root, task, shell="auto"):
            self.root = root
            self.task = task
            self.shell = shell

        def start(self, force=False):
            calls["started"] = force

        def run_one(self, command):
            calls["commands"].append(command)
            return 1 if command == "bad" else 0

        def finish(self):
            calls["finished"] = True

    monkeypatch.setattr(shell_cmd, "GuardedShellSession", FakeRunner)

    result = runner.invoke(
        app,
        ["shell", "TASK-001", "--force", "--project-root", str(tmp_path)],
        input="\n echo ok \nbad\nquit\n",
    )

    assert result.exit_code == 0
    assert calls == {"commands": ["echo ok", "bad"], "started": True, "finished": True}
    assert "Guarded shell for TASK-001" in result.output
    assert "Command exited with 1" in result.output


def test_shell_command_mode_uses_runner_exit_code_and_finish(monkeypatch, tmp_path):
    task = Task(id="TASK-001", title="Task", description="desc")
    _patch_shell_task(monkeypatch, task)
    calls = {"commands": [], "finished": False}

    class FakeRunner:
        def __init__(self, root, task, shell="auto"):
            pass

        def start(self, force=False):
            pass

        def run_one(self, command):
            calls["commands"].append(command)
            return 7

        def finish(self):
            calls["finished"] = True

    monkeypatch.setattr(shell_cmd, "GuardedShellSession", FakeRunner)

    result = runner.invoke(
        app,
        ["shell", "TASK-001", "--command", "run me", "--project-root", str(tmp_path)],
    )

    assert result.exit_code == 7
    assert calls == {"commands": ["run me"], "finished": True}


def test_shell_interactive_loop_exits_cleanly_on_eof(monkeypatch, tmp_path):
    task = Task(id="TASK-001", title="Task", description="desc")
    _patch_shell_task(monkeypatch, task)
    calls = {"finished": False}

    class FakeRunner:
        def __init__(self, root, task, shell="auto"):
            pass

        def start(self, force=False):
            pass

        def run_one(self, command):
            raise AssertionError("EOF should not run a command")

        def finish(self):
            calls["finished"] = True

    monkeypatch.setattr(shell_cmd, "GuardedShellSession", FakeRunner)

    result = runner.invoke(app, ["shell", "TASK-001", "--project-root", str(tmp_path)], input="")

    assert result.exit_code == 0
    assert calls["finished"] is True


def test_mcp_server_callback_runs_only_without_subcommand(monkeypatch):
    calls = []
    monkeypatch.setattr(mcp_server_cmd, "run", lambda: "server-coro")
    monkeypatch.setattr(mcp_server_cmd.asyncio, "run", lambda coro: calls.append(coro))

    mcp_server_cmd.mcp_server(SimpleNamespace(invoked_subcommand="child"))
    assert calls == []

    mcp_server_cmd.mcp_server(SimpleNamespace(invoked_subcommand=None))
    assert calls == ["server-coro"]


def test_graph_demo_writes_and_echoes_path(monkeypatch, tmp_path):
    captured = {}

    def fake_write(root, open_browser=True):
        captured["root"] = root
        captured["open_browser"] = open_browser
        return str(root / "demo.html")

    monkeypatch.setattr(graph_cmd, "write_graph_demo", fake_write)

    result = runner.invoke(app, ["graph", "demo", "--no-open", "--project-root", str(tmp_path)])

    assert result.exit_code == 0
    assert captured == {"root": tmp_path.resolve(), "open_browser": False}
    assert str(tmp_path / "demo.html") in result.output


def test_watch_fs_once_outputs_json_and_plain_events(monkeypatch, tmp_path):
    events = [
        {"path": "src/app.py", "allowed": True, "reason": "planned"},
        {"path": "secret.txt", "allowed": False, "reason": "not planned"},
    ]
    monkeypatch.setattr(watch_fs_cmd, "initialize_project", lambda root, quiet=True: None)

    class FakeWatcher:
        def __init__(self, root, task_id, poll_interval=1.0, on_event=None):
            self.root = root
            self.task_id = task_id
            self.poll_interval = poll_interval
            self.on_event = on_event

        def scan_once(self):
            return events

    monkeypatch.setattr(watch_fs_cmd, "FilesystemWatcher", FakeWatcher)

    as_json = runner.invoke(
        app,
        ["watch", "fs", "--task", "TASK-001", "--once", "--json", "--project-root", str(tmp_path)],
    )
    plain = runner.invoke(
        app,
        ["watch", "fs", "--task", "TASK-001", "--once", "--project-root", str(tmp_path)],
    )

    assert as_json.exit_code == 0
    assert json.loads(as_json.output)["events"] == events
    assert plain.exit_code == 0
    assert "src/app.py allowed: planned" in plain.output
    assert "secret.txt denied: not planned" in plain.output


def test_watch_fs_continuous_mode_handles_keyboard_interrupt(monkeypatch, tmp_path):
    monkeypatch.setattr(watch_fs_cmd, "initialize_project", lambda root, quiet=True: None)
    captured = {}

    class FakeWatcher:
        def __init__(self, root, task_id, poll_interval=1.0, on_event=None):
            captured["poll_interval"] = poll_interval
            captured["task_id"] = task_id
            self.on_event = on_event

        def watch(self):
            self.on_event({"path": "src/app.py", "allowed": True, "reason": "planned"})
            raise KeyboardInterrupt

    monkeypatch.setattr(watch_fs_cmd, "FilesystemWatcher", FakeWatcher)

    result = runner.invoke(
        app,
        [
            "watch",
            "fs",
            "--task",
            "TASK-001",
            "--poll-interval",
            "0.25",
            "--project-root",
            str(tmp_path),
        ],
    )

    assert result.exit_code == 0
    assert captured == {"poll_interval": 0.25, "task_id": "TASK-001"}
    assert "Watching filesystem for task TASK-001" in result.output
    assert "src/app.py allowed: planned" in result.output
    assert "Stopped filesystem watcher" in result.output


class _FakeCheckpointService:
    REF_BEFORE = "refs/devcouncil/{task_id}/before"
    REF_AFTER = "refs/devcouncil/{task_id}/after"
    before_exists = False
    after_exists = False
    rollback_message = "rolled back cleanly"
    refs_checked = []

    def __init__(self, root):
        self.root = root

    def _ref_exists(self, ref):
        self.refs_checked.append(ref)
        if ref.endswith("/before"):
            return self.before_exists
        return self.after_exists

    def rollback(self, task_id):
        return SimpleNamespace(message=self.rollback_message)


def _install_fake_checkpoint(monkeypatch, message="rolled back cleanly", before=False, after=False):
    _FakeCheckpointService.rollback_message = message
    _FakeCheckpointService.before_exists = before
    _FakeCheckpointService.after_exists = after
    _FakeCheckpointService.refs_checked = []
    monkeypatch.setattr(rollback_cmd, "CheckpointService", _FakeCheckpointService)
    return _FakeCheckpointService


def test_rollback_requires_patch_file_or_checkpoint_ref(monkeypatch, tmp_path):
    fake_service = _install_fake_checkpoint(monkeypatch, before=False, after=False)

    with pytest.raises(typer.Exit) as exc:
        rollback_cmd.rollback(SimpleNamespace(invoked_subcommand=None), "TASK-001", tmp_path)

    assert exc.value.exit_code == 1
    assert fake_service.refs_checked == [
        "refs/devcouncil/TASK-001/before",
        "refs/devcouncil/TASK-001/after",
    ]


def test_rollback_success_and_failure_guidance_branches(monkeypatch, tmp_path):
    checkpoint_dir = tmp_path / ".devcouncil" / "checkpoints"
    checkpoint_dir.mkdir(parents=True)

    _install_fake_checkpoint(monkeypatch, message="restored from patch", before=True)
    rollback_cmd.rollback(SimpleNamespace(invoked_subcommand=None), "TASK-001", tmp_path)

    (checkpoint_dir / "TASK-BEFORE-before.patch").write_text("before", encoding="utf-8")
    _install_fake_checkpoint(monkeypatch, message="failed to apply before patch")
    with pytest.raises(typer.Exit) as before_exc:
        rollback_cmd.rollback(SimpleNamespace(invoked_subcommand=None), "TASK-BEFORE", tmp_path)
    assert before_exc.value.exit_code == 1

    (checkpoint_dir / "TASK-AFTER-after.patch").write_text("after", encoding="utf-8")
    _install_fake_checkpoint(monkeypatch, message="No checkpoint could be applied")
    with pytest.raises(typer.Exit) as after_exc:
        rollback_cmd.rollback(SimpleNamespace(invoked_subcommand=None), "TASK-AFTER", tmp_path)
    assert after_exc.value.exit_code == 1

    rollback_cmd.rollback(SimpleNamespace(invoked_subcommand="child"), "TASK-SKIP", tmp_path)


def test_baseline_existing_force_and_missing_database(monkeypatch, tmp_path):
    monkeypatch.setattr(baseline_cmd, "initialize_project", lambda root, quiet=True: None)
    monkeypatch.setattr(baseline_cmd, "get_db", lambda root: None)
    no_db = runner.invoke(app, ["baseline", "--project-root", str(tmp_path)])
    assert no_db.exit_code == 1

    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    baseline_path = dev_dir / "baseline.json"
    baseline_path.write_text('{"old": true}', encoding="utf-8")
    monkeypatch.setattr(baseline_cmd, "get_db", lambda root: object())
    exists = runner.invoke(app, ["baseline", "--project-root", str(tmp_path)])
    assert exists.exit_code == 1
    assert "Baseline already exists" in exists.output
    assert json.loads(baseline_path.read_text(encoding="utf-8")) == {"old": True}

    class FakeVerifier:
        def __init__(self, root):
            self.root = root

        def get_changed_files(self):
            return ["README.md", "src/app.py"]

    monkeypatch.setattr(baseline_cmd, "Verifier", FakeVerifier)
    forced = runner.invoke(app, ["baseline", "--force", "--project-root", str(tmp_path)])
    assert forced.exit_code == 0
    payload = json.loads(baseline_path.read_text(encoding="utf-8"))
    assert payload["changed_files"] == ["README.md", "src/app.py"]


def test_event_bus_handles_sync_async_errors_and_empty_events(caplog):
    from devcouncil.app.events import EventBus, EventTypes

    bus = EventBus()
    seen = []

    def sync_callback(payload):
        seen.append(("sync", payload))

    def raising_callback(payload):
        raise RuntimeError(f"boom {payload['value']}")

    async def async_callback(payload):
        seen.append(("async", payload))

    bus.subscribe(EventTypes.MODEL_CALLED, sync_callback)
    bus.subscribe(EventTypes.MODEL_CALLED, raising_callback)
    bus.subscribe(EventTypes.MODEL_CALLED, async_callback)

    asyncio.run(bus.emit(EventTypes.MODEL_CALLED, {"value": 7}))
    asyncio.run(bus.emit("unregistered", {"value": 8}))

    assert seen == [
        ("sync", {"value": 7}),
        ("async", {"value": 7}),
    ]
    assert "Error in event listener for model_called" in caplog.text


def test_compute_phase_covers_project_lifecycle_branches():
    from devcouncil.app.project_status import compute_phase

    assert compute_phase(ArtifactGraph(), persisted_phase="TASK_VERIFYING") == "TASK_VERIFYING"
    assert compute_phase(ArtifactGraph()) == "NEW"

    req_graph = ArtifactGraph()
    req_graph.add_requirement(Requirement(id="REQ-1", title="R", description="d", priority="high", source="user"))
    assert compute_phase(req_graph) == "REQUIREMENTS_DRAFTED"

    blocked_graph = ArtifactGraph()
    blocked_graph.add_task(Task(id="TASK-BLOCKED-GAP", title="T", description="d"))
    blocked_graph.add_gap(
        Gap(
            id="GAP-1",
            severity="high",
            gap_type="missing_test",
            description="gap",
            recommended_fix="fix",
            blocking=True,
        )
    )
    assert compute_phase(blocked_graph) == "TASK_BLOCKED"

    for status, phase in [
        ("running", "TASK_EXECUTING"),
        ("blocked", "TASK_BLOCKED"),
        ("verified", "PROJECT_DONE"),
        ("done", "PROJECT_DONE"),
        ("planned", "PLAN_APPROVED"),
    ]:
        graph = ArtifactGraph()
        graph.add_task(Task(id=f"TASK-{status}", title="T", description="d", status=status))
        assert compute_phase(graph) == phase


def test_github_check_generator_success_and_failure_payloads():
    from devcouncil.reporting.github_check import GitHubCheckGenerator

    success_graph = ArtifactGraph()
    success = GitHubCheckGenerator.generate(success_graph)
    assert success["conclusion"] == "success"
    assert success["output"]["title"] == "DevCouncil: Success"

    failure_graph = ArtifactGraph()
    failure_graph.add_gap(
        Gap(
            id="GAP-1",
            severity="critical",
            gap_type="security_risk",
            description="Do not ship",
            recommended_fix="fix",
            blocking=True,
        )
    )
    failure = GitHubCheckGenerator.generate(failure_graph)
    assert failure["conclusion"] == "failure"
    assert "GAP-1" in failure["output"]["text"]
    assert "Do not ship" in failure["output"]["text"]


def test_github_integration_posts_check_run_and_reraises_errors(monkeypatch, caplog):
    from devcouncil.integrations import github as github_module

    captured = {}

    class FakeResponse:
        def raise_for_status(self):
            captured["raised_for_status"] = True

    class FakeClient:
        async def __aenter__(self):
            return self

        async def __aexit__(self, exc_type, exc, tb):
            return None

        async def post(self, url, headers, json):
            captured["url"] = url
            captured["headers"] = headers
            captured["json"] = json
            return FakeResponse()

    monkeypatch.setattr(github_module.GitHubCheckGenerator, "generate", lambda graph: {"name": "check"})
    monkeypatch.setattr(github_module.httpx, "AsyncClient", FakeClient)

    integration = github_module.GitHubIntegration("token", "owner/repo", "abc123")
    asyncio.run(integration.report_verification(object()))

    assert captured["url"] == "https://api.github.com/repos/owner/repo/check-runs"
    assert captured["headers"]["Authorization"] == "Bearer token"
    assert captured["json"] == {"name": "check", "head_sha": "abc123"}
    assert captured["raised_for_status"] is True

    class FailingClient(FakeClient):
        async def post(self, url, headers, json):
            raise RuntimeError("network down")

    monkeypatch.setattr(github_module.httpx, "AsyncClient", FailingClient)
    with pytest.raises(RuntimeError, match="network down"):
        asyncio.run(integration.report_verification(object()))
    assert "Failed to report to GitHub" in caplog.text


def test_gitnexus_initializes_nexus_and_managed_agent_guides(tmp_path):
    from devcouncil.integrations.gitnexus import AGENT_GUIDE_MARKER, GitNexusIntegration

    (tmp_path / ".devcouncil").mkdir()
    unmanaged = tmp_path / "AGENTS.md"
    unmanaged.write_text("custom guide\n", encoding="utf-8")
    managed = tmp_path / "CLAUDE.md"
    managed.write_text(f"{AGENT_GUIDE_MARKER}\nold\n", encoding="utf-8")

    integration = GitNexusIntegration(tmp_path)
    integration.initialize()
    integration.sync_graph(object())

    assert json.loads((tmp_path / ".devcouncil" / "nexus" / "index_config.json").read_text()) == {
        "mode": "structural",
        "version": "1.0",
    }
    assert unmanaged.read_text(encoding="utf-8") == "custom guide\n"
    updated = managed.read_text(encoding="utf-8")
    assert AGENT_GUIDE_MARKER in updated
    assert "Repo map: `.devcouncil/repo_map.json`" in updated


def test_graphify_initializes_config_and_applies_rules(tmp_path):
    from devcouncil.integrations.graphify import GraphifyIntegration

    (tmp_path / ".devcouncil").mkdir()
    integration = GraphifyIntegration(tmp_path)

    integration.initialize()
    integration.apply_rules()

    content = (tmp_path / ".devcouncil" / "graphify.yaml").read_text(encoding="utf-8")
    assert "engine: internal" in content
    assert "shared_context: true" in content


def test_package_main_invokes_cli_app(monkeypatch):
    import devcouncil.cli.main as cli_main

    called = []
    monkeypatch.setattr(cli_main, "app", lambda: called.append("app"))

    runpy.run_module("devcouncil.__main__", run_name="__main__")

    assert called == ["app"]


def test_package_main_import_as_module_does_not_invoke_cli(monkeypatch):
    import devcouncil.cli.main as cli_main

    called = []
    monkeypatch.setattr(cli_main, "app", lambda: called.append("app"))

    runpy.run_module("devcouncil.__main__", run_name="devcouncil.__main_test__")

    assert called == []
