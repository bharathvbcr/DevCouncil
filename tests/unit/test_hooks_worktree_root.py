"""Every Claude Code hook must act on the repo Claude is *in*, not where it started.

The Claude Code hooks reference (https://code.claude.com/docs/en/hooks.md,
"Worktrees are different", fetched 2026-09-04) is explicit:

* ``${CLAUDE_PROJECT_DIR}`` "stays put": it still points at the project root where
  the session started.
* "``cwd`` follows Claude": the ``cwd`` field in the hook's input JSON is the
  worktree root after Claude enters a worktree.

DevCouncil bakes ``--project-root`` into every hook command at ``dev integrate``
time, so that argument behaves exactly like ``${CLAUDE_PROJECT_DIR}`` — it names the
directory the session started in and never follows Claude into a worktree.
``_effective_root`` is the one place that reconciles the two, and every hook that
reads or writes ``.devcouncil/`` state has to go through it. When only some of them
do, DevCouncil's own state splits across two repos: the PreCompact snapshot lands in
the parent while the SessionStart that reads it looks in the worktree, so the
post-compaction briefing is silently always empty inside a worktree.

Each test below observes the *filesystem artifact* the hook produces rather than the
root it computed, so it fails for the reason that matters to a user.
"""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

from typer.testing import CliRunner

import devcouncil.cli.commands.hook as hook_mod
from devcouncil.cli.commands.hook import app as hook_app

runner = CliRunner()


def _git(root: Path, *args: str) -> None:
    subprocess.run(
        ["git", "-c", "user.email=t@t", "-c", "user.name=t", *args],
        cwd=root,
        check=True,
        capture_output=True,
        text=True,
    )


def _project(root: Path) -> None:
    """A committed git repo carrying the ``.devcouncil`` marker hooks look for."""
    root.mkdir(parents=True, exist_ok=True)
    (root / ".devcouncil").mkdir(exist_ok=True)
    (root / "pkg").mkdir(exist_ok=True)
    (root / "pkg" / "a.py").write_text("def foo():\n    return 1\n", encoding="utf-8")
    _git(root, "init")
    _git(root, "add", "-A")
    _git(root, "commit", "-m", "init")


def _pair(tmp_path: Path) -> tuple[Path, Path]:
    """``(session root, worktree root)`` shaped like a real Claude Code worktree.

    Claude Code places worktrees under ``<project>/.claude/worktrees/<name>``, which
    is the layout that made the original map-refresh bug invisible: resolved against
    the parent the edited file looks like ``.claude/worktrees/...``, a nested
    checkout the indexer skips.
    """
    parent = tmp_path / "parent"
    worktree = parent / ".claude" / "worktrees" / "wt"
    _project(parent)
    _project(worktree)
    return parent, worktree


def _payload(worktree: Path, **extra: object) -> str:
    return json.dumps({"session_id": "s1", "cwd": str(worktree), **extra})


def _traces(root: Path) -> str:
    path = root / ".devcouncil" / "logs" / "traces.jsonl"
    return path.read_text(encoding="utf-8") if path.is_file() else ""


def _run(command: str, payload: str, parent: Path) -> None:
    result = runner.invoke(hook_app, [command, payload, "--project-root", str(parent)])
    # A lifecycle hook must never invent an exit code: only exit 2 blocks, and these
    # events either cannot block or must not.
    assert result.exit_code == 0, f"{command}: {result.output}"


# --------------------------------------------------------------------------
# Telemetry hooks: the trace belongs to the repo Claude is working in
# --------------------------------------------------------------------------


def test_session_end_traces_into_the_worktree(tmp_path):
    parent, worktree = _pair(tmp_path)
    _run("session-end", _payload(worktree, reason="other"), parent)
    assert "session_end" in _traces(worktree)
    assert "session_end" not in _traces(parent)


def test_post_compact_traces_into_the_worktree(tmp_path):
    parent, worktree = _pair(tmp_path)
    _run("post-compact", _payload(worktree, trigger="auto"), parent)
    assert "post_compact" in _traces(worktree)
    assert "post_compact" not in _traces(parent)


def test_notification_traces_into_the_worktree(tmp_path):
    parent, worktree = _pair(tmp_path)
    _run("notification", _payload(worktree, message="needs attention"), parent)
    assert "claude_notification" in _traces(worktree)
    assert "claude_notification" not in _traces(parent)


def test_directory_added_traces_into_the_worktree(tmp_path):
    parent, worktree = _pair(tmp_path)
    added = tmp_path / "other"
    added.mkdir()
    _run(
        "directory-added",
        _payload(worktree, directory=str(added), source="slash_command"),
        parent,
    )
    assert "directory_added" in _traces(worktree)
    assert "directory_added" not in _traces(parent)


# --------------------------------------------------------------------------
# Compaction continuity: PreCompact writes what SessionStart reads
# --------------------------------------------------------------------------


def test_pre_compact_snapshot_lands_where_session_start_reads_it(tmp_path):
    """The snapshot and its only reader must agree on a repo.

    ``session_start`` already resolves through ``_effective_root``, so a snapshot
    written against the session's start directory is one no reader ever opens: inside
    a worktree the post-compaction briefing is empty every time.
    """
    from devcouncil.execution.stop_gate import compact_snapshot_path

    parent, worktree = _pair(tmp_path)
    _run("pre-compact", _payload(worktree, trigger="manual"), parent)
    assert compact_snapshot_path(worktree).is_file()
    assert not compact_snapshot_path(parent).is_file()


# --------------------------------------------------------------------------
# Stop / SubagentStop: the live-review signal belongs to the worktree
# --------------------------------------------------------------------------


def test_stop_signal_is_written_to_the_worktree(tmp_path):
    """A signal filed under the parent is invisible to the worktree's live review."""
    from devcouncil.live.signals import signal_dir

    parent, worktree = _pair(tmp_path)
    _run("agent-response", _payload(worktree), parent)
    assert list(signal_dir(worktree).glob("claude-*.json"))
    assert not list(signal_dir(parent).glob("claude-*.json"))


def test_subagent_stop_signal_is_written_to_the_worktree(tmp_path):
    from devcouncil.live.signals import signal_dir

    parent, worktree = _pair(tmp_path)
    _run("subagent-stop", _payload(worktree), parent)
    assert list(signal_dir(worktree).glob("claude-*.json"))
    assert not list(signal_dir(parent).glob("claude-*.json"))


# --------------------------------------------------------------------------
# UserPromptSubmit: the injected status line describes the worktree
# --------------------------------------------------------------------------


def test_user_prompt_submit_status_describes_the_worktree(tmp_path, monkeypatch):
    """``additionalContext`` here is a status snapshot; the wrong repo is worse than none.

    ``_status_line`` needs an initialized database to say anything, so the root it is
    handed is observed directly rather than through its output.
    """
    parent, worktree = _pair(tmp_path)
    seen: list[Path] = []

    def _record(root: Path) -> str | None:
        seen.append(root)
        return None

    monkeypatch.setattr(hook_mod, "_status_line", _record)
    _run("user-prompt-submit", _payload(worktree, prompt="hello"), parent)
    assert seen == [worktree.resolve()]


# --------------------------------------------------------------------------
# The baked root still wins when cwd says nothing new
# --------------------------------------------------------------------------


def test_cwd_outside_a_devcouncil_project_keeps_the_baked_root(tmp_path):
    """Only another *initialized* project takes ownership; a plain directory doesn't."""
    parent, _ = _pair(tmp_path)
    elsewhere = tmp_path / "not-a-project"
    elsewhere.mkdir()
    _run("session-end", _payload(elsewhere, reason="other"), parent)
    assert "session_end" in _traces(parent)


def test_payload_without_cwd_keeps_the_baked_root(tmp_path):
    parent, _ = _pair(tmp_path)
    _run("post-compact", json.dumps({"session_id": "s1", "trigger": "auto"}), parent)
    assert "post_compact" in _traces(parent)
