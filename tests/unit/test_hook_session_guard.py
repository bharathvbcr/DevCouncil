"""SessionStart is where a session learns it is not alone in this repository.

The "Continuity — …" clause of the status line is the first thing a session
reads, so it is where the warning about other live checkouts and unmerged
branches belongs. These tests pin three things: the warning reaches that line,
only SessionStart pays for the git probes that produce it, and neither a
failing probe nor an enormous one can damage the hook's output.
"""

from __future__ import annotations

import json
import subprocess
from pathlib import Path
from types import SimpleNamespace

import pytest
from typer.testing import CliRunner

import devcouncil.cli.commands.hook as hook
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


def _repo_with_dirty_sibling(tmp_path: Path) -> Path:
    main = tmp_path / "main"
    main.mkdir()
    _git(main, "init", "-b", "main")
    (main / "a.txt").write_text("one\n", encoding="utf-8")
    _git(main, "add", "-A")
    _git(main, "commit", "-m", "init")
    sibling = tmp_path / "wt"
    _git(main, "worktree", "add", "-b", "claude/other", str(sibling))
    (sibling / "a.txt").write_text("the other session is editing this\n", encoding="utf-8")
    return main


# --- the warning reaches the Continuity clause ---------------------------------


def test_session_guard_hints_name_a_live_sibling(tmp_path: Path) -> None:
    main = _repo_with_dirty_sibling(tmp_path)

    hints = hook._session_guard_hints(main)

    assert hints, hints
    assert "live sibling" in hints[0]
    assert "claude/other" in hints[0]


def test_session_start_context_folds_the_warning_into_continuity(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    main = _repo_with_dirty_sibling(tmp_path)
    monkeypatch.setattr(
        hook, "_status_line", lambda root: "DevCouncil — phase: x. Continuity — repo map stale."
    )
    monkeypatch.setattr("devcouncil.execution.stop_gate.session_briefing", lambda root, payload: None)

    ctx = hook._session_start_context(main, {})

    assert ctx is not None
    assert ctx.count("Continuity —") == 1, ctx
    assert "repo map stale" in ctx
    assert "live sibling" in ctx
    assert ctx.endswith(".")


def test_continuity_is_appended_when_the_base_line_has_none() -> None:
    line = hook._with_continuity("DevCouncil — phase: x.", ["1 live sibling checkout: wt"])

    assert line == "DevCouncil — phase: x. Continuity — 1 live sibling checkout: wt."


def test_no_findings_leaves_the_line_untouched() -> None:
    assert hook._with_continuity("BASE.", []) == "BASE."


# --- only SessionStart pays for it ---------------------------------------------


def test_user_prompt_submit_does_not_run_the_session_guard(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    calls: list[Path] = []
    monkeypatch.setattr(hook, "_session_guard_hints", lambda root: calls.append(root) or [])
    monkeypatch.setattr(hook, "_status_line", lambda root: "ctx line")

    result = runner.invoke(hook_app, ["user-prompt-submit", "{}", "--project-root", str(tmp_path)])

    assert result.exit_code == 0
    assert calls == []


def test_post_tool_use_does_not_run_the_session_guard(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from devcouncil.utils import git_siblings

    calls: list[Path] = []

    def refuse(root, **kwargs):  # pragma: no cover - the assertion is that it never runs
        calls.append(root)
        raise AssertionError("PostToolUse must not probe git for siblings")

    monkeypatch.setattr(git_siblings, "inspect_session_siblings", refuse)
    payload = json.dumps({"tool_name": "Edit", "tool_input": {"file_path": str(tmp_path / "x.py")}})

    result = runner.invoke(hook_app, ["post-tool-use", payload, "--project-root", str(tmp_path)])

    assert result.exit_code == 0
    assert calls == []


def test_session_start_runs_the_session_guard(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    calls: list[Path] = []
    monkeypatch.setattr(hook, "_status_line", lambda root: "BASE.")
    monkeypatch.setattr(
        hook, "_session_guard_hints", lambda root: calls.append(root) or ["1 live sibling checkout: wt"]
    )
    monkeypatch.setattr("devcouncil.execution.stop_gate.session_briefing", lambda root, payload: None)
    monkeypatch.setattr(hook, "TraceLogger", lambda root: SimpleNamespace(log_event=lambda *a, **k: None))

    result = runner.invoke(hook_app, ["session-start", "{}", "--project-root", str(tmp_path)])

    assert result.exit_code == 0
    assert len(calls) == 1
    ctx = json.loads(result.stdout)["hookSpecificOutput"]["additionalContext"]
    assert "1 live sibling checkout: wt" in ctx


# --- a failing or enormous probe cannot damage the hook -------------------------


def test_a_probe_that_raises_costs_nothing(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    from devcouncil.utils import git_siblings

    def boom(root, **kwargs):
        raise RuntimeError("git exploded")

    monkeypatch.setattr(git_siblings, "inspect_session_siblings", boom)

    assert hook._session_guard_hints(tmp_path) == []


def test_session_start_output_stays_within_the_hook_cap(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(hook, "_status_line", lambda root: "BASE.")
    monkeypatch.setattr(hook, "_session_guard_hints", lambda root: ["x" * 50_000])
    monkeypatch.setattr("devcouncil.execution.stop_gate.session_briefing", lambda root, payload: None)
    monkeypatch.setattr(hook, "TraceLogger", lambda root: SimpleNamespace(log_event=lambda *a, **k: None))

    result = runner.invoke(hook_app, ["session-start", "{}", "--project-root", str(tmp_path)])

    assert result.exit_code == 0
    ctx = json.loads(result.stdout)["hookSpecificOutput"]["additionalContext"]
    assert len(ctx) <= hook.HOOK_OUTPUT_MAX_CHARS
