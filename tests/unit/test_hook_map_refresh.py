"""Post-tool-use hook incremental map refresh."""

from __future__ import annotations

import json
import os
import subprocess
import time

from typer.testing import CliRunner

from devcouncil.cli.commands.hook import (
    MAP_REFRESH_DEBOUNCE_S,
    _enqueue_refresh_paths,
    _extract_written_paths,
    _lock_is_reclaimable,
    _maybe_refresh_map,
    _pid_alive,
    _take_queued_paths,
    _try_acquire_refresh_lock,
    app as hook_app,
)
from devcouncil.indexing.repo_mapper import RepoMapper
from devcouncil.integrations.clients.hooks import _install_claude_hooks


def _git(root, *args):
    subprocess.run(["git", *args], cwd=root, check=True, capture_output=True, text=True)


def _commit(root):
    _git(root, "init")
    _git(root, "-c", "user.email=t@t", "-c", "user.name=t", "add", "-A")
    _git(root, "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", "init")


def _write(tmp_path, files):
    for rel, content in files.items():
        p = tmp_path / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(content, encoding="utf-8")


def test_extract_written_paths_from_write_payload():
    payload = {
        "tool_name": "Write",
        "tool_input": {"file_path": "src/foo.py", "content": "x=1"},
    }
    assert _extract_written_paths(payload) == ["src/foo.py"]


def test_post_tool_use_refresh_best_effort(tmp_path):
    _write(tmp_path, {
        "pkg/__init__.py": "",
        "pkg/a.py": "def foo():\n    return 1\n",
    })
    _commit(tmp_path)
    (tmp_path / ".devcouncil").mkdir(exist_ok=True)
    RepoMapper(tmp_path).map_repo(liveness=False)

    (tmp_path / "pkg" / "a.py").write_text(
        "def foo():\n    return 1\ndef added():\n    return 2\n", encoding="utf-8"
    )
    payload = json.dumps({
        "tool_name": "Edit",
        "tool_input": {"file_path": str(tmp_path / "pkg" / "a.py")},
    })
    runner = CliRunner()
    result = runner.invoke(
        hook_app,
        ["post-tool-use", payload, "--client", "claude", "--project-root", str(tmp_path)],
    )
    assert result.exit_code == 0
    graph_path = tmp_path / ".devcouncil" / "graph" / "code_graph.json"
    assert graph_path.is_file()
    data = json.loads(graph_path.read_text(encoding="utf-8"))
    names = [n.get("name") for n in data.get("nodes") or []]
    assert "added" in names


def test_claude_assist_mode_installs_refresh_only_post_tool_use(tmp_path):
    """Assist mode installs PostToolUse (refresh) but never PreToolUse (gate)."""
    written = _install_claude_hooks(tmp_path, write_gate=False)
    assert written
    settings = json.loads((tmp_path / ".claude" / "settings.local.json").read_text(encoding="utf-8"))
    hooks = settings["hooks"]
    assert "PostToolUse" in hooks
    assert "PreToolUse" not in hooks
    blob = json.dumps(hooks["PostToolUse"])
    assert "post-tool-use" in blob
    assert "pre-tool-use" not in blob


def test_claude_write_gate_adds_pre_tool_use(tmp_path):
    _install_claude_hooks(tmp_path, write_gate=True)
    settings = json.loads((tmp_path / ".claude" / "settings.local.json").read_text(encoding="utf-8"))
    assert "PreToolUse" in settings["hooks"]
    assert "PostToolUse" in settings["hooks"]


def test_refresh_lock_reclaim_when_owner_gone(tmp_path):
    lock = tmp_path / ".devcouncil" / "cache" / "map_refresh.lock"
    lock.parent.mkdir(parents=True, exist_ok=True)
    # Dead PID that cannot be alive on this host.
    lock.write_text(json.dumps({"pid": 2**31 - 2, "started_at": time.time()}), encoding="utf-8")
    assert _lock_is_reclaimable(lock)
    assert _try_acquire_refresh_lock(lock) is True
    assert lock.is_file()
    meta = json.loads(lock.read_text(encoding="utf-8"))
    assert meta["pid"] == os.getpid()


def test_refresh_lock_reclaim_when_past_ttl(tmp_path):
    lock = tmp_path / ".devcouncil" / "cache" / "map_refresh.lock"
    lock.parent.mkdir(parents=True, exist_ok=True)
    # Current process is alive, but timestamp is ancient → reclaim by TTL.
    lock.write_text(
        json.dumps({"pid": os.getpid(), "started_at": time.time() - 10_000}),
        encoding="utf-8",
    )
    assert _lock_is_reclaimable(lock)
    assert _try_acquire_refresh_lock(lock) is True


def test_refresh_queue_on_lock_conflict(tmp_path, monkeypatch):
    _write(tmp_path, {
        "pkg/__init__.py": "",
        "pkg/a.py": "def foo():\n    return 1\n",
    })
    _commit(tmp_path)
    (tmp_path / ".devcouncil").mkdir(exist_ok=True)
    RepoMapper(tmp_path).map_repo(liveness=False)

    lock = tmp_path / ".devcouncil" / "cache" / "map_refresh.lock"
    lock.parent.mkdir(parents=True, exist_ok=True)
    # Non-reclaimable lock held by this process with a fresh timestamp.
    lock.write_text(
        json.dumps({"pid": os.getpid(), "started_at": time.time()}),
        encoding="utf-8",
    )

    refreshed: list[list[str]] = []

    def _fake_refresh(root, paths, **kwargs):  # noqa: ANN001
        refreshed.append(list(paths))
        return None

    monkeypatch.setattr(
        "devcouncil.indexing.graph.build.refresh_map_for_paths",
        _fake_refresh,
    )
    monkeypatch.setattr(
        "devcouncil.cli.commands.hook.MAP_REFRESH_DEBOUNCE_S",
        0.0,
    )

    payload = json.dumps({
        "tool_name": "Edit",
        "tool_input": {"file_path": str(tmp_path / "pkg" / "a.py")},
    })
    _maybe_refresh_map(tmp_path, payload)
    assert refreshed == []  # conflict → enqueue, do not refresh as holder
    queue = tmp_path / ".devcouncil" / "cache" / "map_refresh_queue.json"
    assert queue.is_file()
    data = json.loads(queue.read_text(encoding="utf-8"))
    assert "pkg/a.py" in data["paths"]


def test_refresh_holder_drains_queue(tmp_path, monkeypatch):
    _write(tmp_path, {
        "pkg/__init__.py": "",
        "pkg/a.py": "def foo():\n    return 1\n",
        "pkg/b.py": "def bar():\n    return 2\n",
    })
    _commit(tmp_path)
    (tmp_path / ".devcouncil").mkdir(exist_ok=True)
    RepoMapper(tmp_path).map_repo(liveness=False)

    queue = tmp_path / ".devcouncil" / "cache" / "map_refresh_queue.json"
    _enqueue_refresh_paths(queue, ["pkg/b.py"])

    refreshed: list[list[str]] = []

    def _fake_refresh(root, paths, **kwargs):  # noqa: ANN001
        refreshed.append(sorted(paths))
        return None

    monkeypatch.setattr(
        "devcouncil.indexing.graph.build.refresh_map_for_paths",
        _fake_refresh,
    )
    monkeypatch.setattr(
        "devcouncil.cli.commands.hook.MAP_REFRESH_DEBOUNCE_S",
        0.0,
    )

    payload = json.dumps({
        "tool_name": "Edit",
        "tool_input": {"file_path": str(tmp_path / "pkg" / "a.py")},
    })
    _maybe_refresh_map(tmp_path, payload)
    assert refreshed
    # Initial path + drained queue path in one batch (or sequential drain).
    flat = {p for batch in refreshed for p in batch}
    assert "pkg/a.py" in flat
    assert "pkg/b.py" in flat
    assert not queue.is_file() or not _take_queued_paths(queue)


def test_pid_alive_self():
    assert _pid_alive(os.getpid()) is True
    assert _pid_alive(-1) is False


def test_debounce_constant_is_about_300ms():
    assert abs(MAP_REFRESH_DEBOUNCE_S - 0.3) < 1e-9


def test_take_queued_paths_rename_drain_merges_recreated_queue(tmp_path, monkeypatch):
    """Enqueue-during-drain must not lose paths (rename-to-drain + re-check)."""
    import devcouncil.cli.commands.hook as hook_mod

    queue = tmp_path / "map_refresh_queue.json"
    _enqueue_refresh_paths(queue, ["pkg/a.py"])

    # Simulate concurrent enqueue after rename: inject a new queue file while
    # draining by wrapping os.replace used inside _take_queued_paths.
    real_replace = os.replace
    injected = {"done": False}

    def replace_then_enqueue(src, dst):  # noqa: ANN001
        real_replace(src, dst)
        if not injected["done"] and str(src) == str(queue):
            injected["done"] = True
            _enqueue_refresh_paths(queue, ["pkg/b.py"])

    monkeypatch.setattr(hook_mod.os, "replace", replace_then_enqueue)
    paths = _take_queued_paths(queue)

    assert "pkg/a.py" in paths
    assert "pkg/b.py" in paths
    assert not queue.is_file() or _take_queued_paths(queue) == []


def test_enqueue_during_holder_refresh_merged(tmp_path, monkeypatch):
    """Paths enqueued while the holder refreshes are drained into a follow-up batch."""
    _write(tmp_path, {
        "pkg/__init__.py": "",
        "pkg/a.py": "def foo():\n    return 1\n",
        "pkg/b.py": "def bar():\n    return 2\n",
        "pkg/c.py": "def baz():\n    return 3\n",
    })
    _commit(tmp_path)
    (tmp_path / ".devcouncil").mkdir(exist_ok=True)
    RepoMapper(tmp_path).map_repo(liveness=False)

    refreshed: list[list[str]] = []
    queue = tmp_path / ".devcouncil" / "cache" / "map_refresh_queue.json"

    def _fake_refresh(root, paths, **kwargs):  # noqa: ANN001
        refreshed.append(sorted(paths))
        # Concurrent PostToolUse while we hold the lock.
        if len(refreshed) == 1:
            _enqueue_refresh_paths(queue, ["pkg/c.py"])

    monkeypatch.setattr(
        "devcouncil.indexing.graph.build.refresh_map_for_paths",
        _fake_refresh,
    )
    monkeypatch.setattr(
        "devcouncil.cli.commands.hook.MAP_REFRESH_DEBOUNCE_S",
        0.0,
    )

    payload = json.dumps({
        "tool_name": "Edit",
        "tool_input": {"file_path": str(tmp_path / "pkg" / "a.py")},
    })
    _enqueue_refresh_paths(queue, ["pkg/b.py"])
    _maybe_refresh_map(tmp_path, payload)

    flat = {p for batch in refreshed for p in batch}
    assert "pkg/a.py" in flat
    assert "pkg/b.py" in flat
    assert "pkg/c.py" in flat
    assert not queue.is_file() or _take_queued_paths(queue) == []


def test_map_if_stale_exits_fast_when_fresh(tmp_path):
    from typer.testing import CliRunner
    from devcouncil.cli.main import app

    _write(tmp_path, {
        "pkg/__init__.py": "",
        "pkg/a.py": "def foo():\n    return 1\n",
    })
    _commit(tmp_path)
    runner = CliRunner()
    first = runner.invoke(app, ["map", "--no-wiki", "--project-root", str(tmp_path)])
    assert first.exit_code == 0, first.output
    second = runner.invoke(
        app, ["map", "--if-stale", "--no-wiki", "--project-root", str(tmp_path)]
    )
    assert second.exit_code == 0, second.output
    assert "fresh" in second.output.lower() or "skipping" in second.output.lower()
