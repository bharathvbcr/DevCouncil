"""CLI coverage for `dev watch fs` (filesystem attribution watcher).

The real :class:`FilesystemWatcher` polls the tree and consults the policy engine;
here it is replaced with a fake so the command's ``--once``, JSON, and interactive
(Ctrl-C) paths are exercised deterministically.
"""

import json

import devcouncil.cli.commands.watch_fs as watch_fs_cmd
from devcouncil.cli.main import app
from typer.testing import CliRunner

runner = CliRunner()


class _FakeWatcher:
    events = [
        {"path": "src/a.py", "allowed": True, "reason": "in scope"},
        {"path": "src/secret.py", "allowed": False, "reason": "out of scope"},
    ]

    def __init__(self, root, task_id, poll_interval=1.0, on_event=None):
        self.root = root
        self.task_id = task_id
        self.on_event = on_event

    def scan_once(self):
        return list(self.events)

    def watch(self):
        raise KeyboardInterrupt


def test_watch_fs_once_human(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr(watch_fs_cmd, "FilesystemWatcher", _FakeWatcher)

    result = runner.invoke(app, ["watch", "fs", "--task", "TASK-001", "--once", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "src/a.py" in result.output
    assert "allowed" in result.output
    assert "denied" in result.output


def test_watch_fs_once_json(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr(watch_fs_cmd, "FilesystemWatcher", _FakeWatcher)

    result = runner.invoke(
        app, ["watch", "fs", "--task", "TASK-001", "--once", "--json", "--project-root", str(tmp_path)]
    )
    assert result.exit_code == 0
    data = json.loads(result.stdout)
    assert len(data["events"]) == 2
    assert data["events"][0]["path"] == "src/a.py"


def test_watch_fs_continuous_stops_on_keyboard_interrupt(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr(watch_fs_cmd, "FilesystemWatcher", _FakeWatcher)

    result = runner.invoke(app, ["watch", "fs", "--task", "TASK-001", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "Watching filesystem" in result.output
    assert "Stopped filesystem watcher" in result.output


# --- `--json` contract: exactly one JSON object on stdout, diagnostics on stderr ---


class _EmittingWatcher(_FakeWatcher):
    """A fake that fires ``on_event`` during the scan, as the real watcher does.

    ``FilesystemWatcher._record_event`` calls ``self.on_event(event)`` for every recorded
    path, so ``scan_once()`` under ``--once --json`` emitted one line per changed file
    ahead of the payload. ``_FakeWatcher`` above never calls it, which is why the
    pre-existing tests could not see this.
    """

    def scan_once(self):
        events = list(self.events)
        for event in events:
            if self.on_event is not None:
                self.on_event(event)
        return events


def test_watch_fs_once_json_live_feed_stays_off_stdout(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr(watch_fs_cmd, "FilesystemWatcher", _EmittingWatcher)

    result = runner.invoke(
        app, ["watch", "fs", "--task", "TASK-001", "--once", "--json", "--project-root", str(tmp_path)]
    )

    assert result.exit_code == 0
    data = json.loads(result.stdout)
    assert [e["path"] for e in data["events"]] == ["src/a.py", "src/secret.py"]
    # The live feed reported both files — on stderr, where it cannot corrupt the payload.
    assert "src/a.py" in result.stderr
    assert "out of scope" in result.stderr


def test_watch_fs_once_human_still_renders_events_on_stdout(tmp_path, monkeypatch):
    """Human mode keeps its result on stdout; only the live feed moved."""
    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr(watch_fs_cmd, "FilesystemWatcher", _EmittingWatcher)

    result = runner.invoke(
        app, ["watch", "fs", "--task", "TASK-001", "--once", "--project-root", str(tmp_path)]
    )

    assert result.exit_code == 0
    assert "src/a.py" in result.stdout
    assert "out of scope" in result.stdout


def test_watch_fs_follow_json_leaves_stdout_empty(tmp_path, monkeypatch):
    """Follow mode never reaches a payload, so stdout must stay empty under `--json`."""
    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr(watch_fs_cmd, "FilesystemWatcher", _FakeWatcher)

    result = runner.invoke(
        app, ["watch", "fs", "--task", "TASK-001", "--json", "--project-root", str(tmp_path)]
    )

    assert result.exit_code == 0
    assert result.stdout == ""
    assert "Watching filesystem for task TASK-001" in result.stderr
