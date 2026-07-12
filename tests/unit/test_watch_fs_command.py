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
