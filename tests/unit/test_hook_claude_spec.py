"""Claude Code hook-system conformance for DevCouncil's hook surface.

Every test here pins a rule from the Claude Code hooks reference
(https://code.claude.com/docs/en/hooks.md) that DevCouncil's generated hook config
or hook output must satisfy:

* ``systemMessage`` is a *universal top-level* JSON field, not a member of
  ``hookSpecificOutput`` (which requires a non-empty ``hookEventName``).
* Hook output strings are capped at 10,000 characters.
* ``SessionStart`` fires with source ``fork`` since Claude Code v2.1.214.
* A matcher of letters/digits/``_``/``-``/space/``,``/``|`` is an *exact* match,
  so a DevCouncil-owned hook name must live in exactly one matcher group.
* ``${CLAUDE_PROJECT_DIR}`` and a baked ``--project-root`` do not follow Claude
  into a git worktree; the payload's ``cwd`` field does.
"""

from __future__ import annotations

import contextlib
import io
import json
import re
import subprocess
from pathlib import Path

from typer.testing import CliRunner

from devcouncil.cli.commands.hook import (
    HOOK_OUTPUT_MAX_CHARS,
    _effective_root,
    _emit_additional_context,
    _emit_decision,
    _emit_system_message,
    _git_watch_paths,
    app as hook_app,
)
from devcouncil.integrations.clients.hooks import (
    SESSION_START_MATCHER,
    _install_claude_hooks,
    _install_codex_hooks,
    _install_gemini_hooks,
    _install_grok_hooks,
    _upsert_hook,
)

from tests.unit.support_maps import write_stamped_map

runner = CliRunner()


def _capture(fn, *args, **kwargs) -> str:
    buf = io.StringIO()
    with contextlib.redirect_stdout(buf):
        fn(*args, **kwargs)
    return buf.getvalue()


def _git(root: Path, *args: str) -> None:
    subprocess.run(
        ["git", "-c", "user.email=t@t", "-c", "user.name=t", *args],
        cwd=root,
        check=True,
        capture_output=True,
        text=True,
    )


def _repo(root: Path) -> None:
    """A committed git repo with a .devcouncil marker, the shape hooks act on."""
    root.mkdir(parents=True, exist_ok=True)
    (root / ".devcouncil").mkdir(exist_ok=True)
    (root / "pkg").mkdir(exist_ok=True)
    (root / "pkg" / "__init__.py").write_text("", encoding="utf-8")
    (root / "pkg" / "a.py").write_text("def foo():\n    return 1\n", encoding="utf-8")
    _git(root, "init")
    _git(root, "add", "-A")
    _git(root, "commit", "-m", "init")


def _claude_settings(root: Path) -> dict:
    return json.loads((root / ".claude" / "settings.local.json").read_text(encoding="utf-8"))


def _hook_names(settings: dict, event: str) -> list[str]:
    out: list[str] = []
    for group in settings.get("hooks", {}).get(event, []):
        for handler in group.get("hooks", []):
            out.append(handler.get("name", ""))
    return out


# --------------------------------------------------------------------------
# systemMessage placement (spec: "JSON output" universal-field table)
# --------------------------------------------------------------------------


def test_system_message_is_top_level_not_nested():
    """``systemMessage`` nested in hookSpecificOutput is silently dropped."""
    payload = json.loads(_capture(_emit_system_message, "PreCompact", "snapshot saved"))
    assert payload.get("systemMessage") == "snapshot saved"
    assert "systemMessage" not in payload.get("hookSpecificOutput", {})


def test_system_message_single_arg_emits_no_empty_hook_event_name():
    """An empty ``hookEventName`` fails schema validation -> reported hook error."""
    payload = json.loads(_capture(_emit_system_message, "just a message"))
    assert payload.get("systemMessage") == "just a message"
    assert payload.get("hookSpecificOutput", {}).get("hookEventName") != ""


# --------------------------------------------------------------------------
# 10,000-character output cap
# --------------------------------------------------------------------------


def test_additional_context_capped_at_10k():
    payload = json.loads(_capture(_emit_additional_context, "SessionStart", "x" * 25_000))
    context = payload["hookSpecificOutput"]["additionalContext"]
    assert len(context) <= HOOK_OUTPUT_MAX_CHARS


def test_system_message_capped_at_10k():
    payload = json.loads(_capture(_emit_system_message, "PreCompact", "y" * 25_000))
    assert len(payload["systemMessage"]) <= HOOK_OUTPUT_MAX_CHARS


def test_short_output_is_not_truncated():
    payload = json.loads(_capture(_emit_additional_context, "SessionStart", "short"))
    assert payload["hookSpecificOutput"]["additionalContext"] == "short"


# --------------------------------------------------------------------------
# SessionStart matcher covers forked sessions
# --------------------------------------------------------------------------


def test_session_start_matcher_includes_fork():
    """/fork, --fork-session and /branch report source "fork" (v2.1.214+)."""
    assert "fork" in SESSION_START_MATCHER.split("|")


# --------------------------------------------------------------------------
# PreToolUse warn path must reach a human
# --------------------------------------------------------------------------


def test_claude_pretooluse_warn_emits_json_system_message():
    """Plain stdout from PreToolUse goes to the debug log only; nobody sees it."""
    payload = json.loads(_capture(_emit_decision, "claude", "warn", "no active lease"))
    assert "no active lease" in payload["systemMessage"]


def test_codex_pretooluse_warn_stays_system_message_only():
    """Regression guard: Codex PreToolUse accepts only systemMessage."""
    payload = json.loads(_capture(_emit_decision, "codex", "warn", "no active lease"))
    assert set(payload) == {"systemMessage"}


def test_gemini_pretooluse_warn_keeps_its_own_schema():
    """Regression guard: Claude fields must not leak into Gemini's config."""
    payload = json.loads(_capture(_emit_decision, "gemini", "warn", "no active lease"))
    assert payload["decision"] == "allow"
    assert payload["suppressOutput"] is True


# --------------------------------------------------------------------------
# Exact-match matchers: one group per DevCouncil-owned hook name
# --------------------------------------------------------------------------


def test_upsert_hook_migrates_matcher_instead_of_duplicating():
    """A changed matcher must move the hook, not register it under both."""
    settings: dict = {}
    _upsert_hook(settings, "SessionStart", "startup|resume", "cmd", "devcouncil-session-start")
    _upsert_hook(settings, "SessionStart", "startup|resume|clear", "cmd", "devcouncil-session-start")
    groups = settings["hooks"]["SessionStart"]
    registrations = [
        handler
        for group in groups
        for handler in group["hooks"]
        if handler["name"] == "devcouncil-session-start"
    ]
    assert len(registrations) == 1
    assert [g["matcher"] for g in groups] == ["startup|resume|clear"]


def test_upsert_hook_leaves_foreign_hooks_in_stale_group():
    """Only DevCouncil's own name migrates; a user's hook keeps its group."""
    settings: dict = {
        "hooks": {
            "SessionStart": [
                {
                    "matcher": "startup",
                    "hooks": [
                        {"type": "command", "name": "devcouncil-session-start", "command": "old"},
                        {"type": "command", "name": "user-hook", "command": "keep"},
                    ],
                }
            ]
        }
    }
    _upsert_hook(settings, "SessionStart", "startup|resume", "cmd", "devcouncil-session-start")
    groups = settings["hooks"]["SessionStart"]
    stale = [g for g in groups if g["matcher"] == "startup"][0]
    assert [h["name"] for h in stale["hooks"]] == ["user-hook"]
    assert "devcouncil-session-start" in _hook_names(settings, "SessionStart")
    assert len(_hook_names(settings, "SessionStart")) == 2


def test_claude_reinstall_never_duplicates_a_hook_name(tmp_path):
    _repo(tmp_path)
    _install_claude_hooks(tmp_path)
    _install_claude_hooks(tmp_path)
    settings = _claude_settings(tmp_path)
    for event in settings.get("hooks", {}):
        names = [n for n in _hook_names(settings, event) if n.startswith("devcouncil-")]
        assert len(names) == len(set(names)), f"{event} registers a DevCouncil hook twice"


# --------------------------------------------------------------------------
# Worktree correctness: the payload's cwd, not the baked project root
# --------------------------------------------------------------------------


def test_effective_root_prefers_worktree_cwd(tmp_path):
    """A baked --project-root does not follow Claude into a worktree; cwd does."""
    main = tmp_path / "main"
    worktree = tmp_path / "main" / ".claude" / "worktrees" / "wt"
    _repo(main)
    worktree.mkdir(parents=True)
    (worktree / ".devcouncil").mkdir()
    assert _effective_root(main, {"cwd": str(worktree)}) == worktree.resolve()


def test_effective_root_ignores_cwd_that_is_not_a_project(tmp_path):
    main = tmp_path / "main"
    _repo(main)
    stray = tmp_path / "stray"
    stray.mkdir()
    assert _effective_root(main, {"cwd": str(stray)}) == main.resolve()


def test_effective_root_without_cwd_uses_project_root(tmp_path):
    main = tmp_path / "main"
    _repo(main)
    assert _effective_root(main, {}) == main.resolve()


def test_post_tool_use_refreshes_the_worktree_map_not_the_parent(tmp_path, monkeypatch):
    """The live bug: edits inside a worktree were dropped as ".claude/worktrees/...".

    The parent's ``should_skip_path`` filters that prefix, so with the baked root
    the refresh silently never ran.
    """
    main = tmp_path / "main"
    _repo(main)
    worktree = main / ".claude" / "worktrees" / "wt"
    _repo(worktree)
    write_stamped_map(worktree)

    seen: list[Path] = []

    def _fake_refresh(root, output, *args, **kwargs):  # noqa: ANN001
        seen.append(Path(root))
        from devcouncil.indexing.map_artifacts import GraphRefreshResult

        return GraphRefreshResult(map_path=output)

    monkeypatch.setattr(
        "devcouncil.indexing.map_artifacts.refresh_map_artifacts", _fake_refresh
    )
    payload = {
        "hook_event_name": "PostToolUse",
        "cwd": str(worktree),
        "tool_name": "Write",
        "tool_input": {"file_path": str(worktree / "pkg" / "a.py")},
    }
    result = runner.invoke(
        hook_app,
        ["post-tool-use", json.dumps(payload), "--client", "claude", "--project-root", str(main)],
    )
    assert result.exit_code == 0, result.output
    assert seen, "no map refresh ran for an edit made inside the worktree"
    assert seen == [worktree.resolve()], "the parent root must not be rebuilt instead"


# --------------------------------------------------------------------------
# New events: registration and client isolation
# --------------------------------------------------------------------------


def test_claude_installed_event_surface_is_exactly_this(tmp_path):
    """Pin the whole installed set, not just the events a given test happens to name.

    Dropping an ``_upsert_hook`` call is otherwise invisible: every other assertion
    here is keyed on an event it already expects, so a missing registration passes.
    Two exclusions are deliberate and belong in the same list:

    * ``PreToolUse`` — the blocking write gate, installed only with ``--write-gate``.
    * ``WorktreeCreate`` — registering it *replaces* Claude Code's own git-worktree
      behaviour, which DevCouncil has no reason to take over.

    Adding an event to ``_install_claude_hooks`` should fail here until the event has
    a handler and a test; this list is the review gate for that.
    """
    _repo(tmp_path)
    _install_claude_hooks(tmp_path)
    assert set(_claude_settings(tmp_path)["hooks"]) == {
        "PostToolUse",
        "PostToolBatch",
        "Stop",
        "SubagentStop",
        "SessionStart",
        "SessionEnd",
        "UserPromptSubmit",
        "PreCompact",
        "PostCompact",
        "Notification",
        "FileChanged",
        "CwdChanged",
        "DirectoryAdded",
        # StopFailure fires *instead of* Stop on an API-error turn end, so without it
        # the stop gate silently never evaluates that turn; SubagentStart tells a
        # subagent about the task SubagentStop will hold it to.
        "StopFailure",
        "SubagentStart",
    }


def test_write_gate_adds_pre_tool_use_and_nothing_else(tmp_path):
    """``--write-gate`` is the only thing that installs a hook able to block a tool."""
    _repo(tmp_path)
    _install_claude_hooks(tmp_path)
    assist = set(_claude_settings(tmp_path)["hooks"])
    _install_claude_hooks(tmp_path, write_gate=True)
    assert set(_claude_settings(tmp_path)["hooks"]) - assist == {"PreToolUse"}


def test_installed_matchers_stay_on_the_exact_match_path(tmp_path):
    """A matcher of letters/digits/``_``/``-``/space/``,``/``|`` is compared exactly.

    Any other character makes Claude Code treat the value as an *unanchored*
    JavaScript regex, so ``Edit`` would silently also match ``NotebookEdit``.
    """
    _repo(tmp_path)
    _install_claude_hooks(tmp_path, write_gate=True)
    for event, groups in _claude_settings(tmp_path)["hooks"].items():
        for group in groups:
            matcher = group.get("matcher", "")
            assert re.fullmatch(r"[A-Za-z0-9_\- ,|]*", matcher), f"{event}: {matcher!r}"


def test_installed_timeouts_are_plausible_seconds(tmp_path):
    """``timeout`` is in SECONDS. A millisecond value reads as a multi-hour timeout.

    Also holds SessionEnd under the 60-second ceiling Claude Code will raise its
    1.5-second shared budget to; a longer value there cannot take effect.
    """
    _repo(tmp_path)
    _install_claude_hooks(tmp_path, write_gate=True)
    for event, groups in _claude_settings(tmp_path)["hooks"].items():
        for group in groups:
            for handler in group["hooks"]:
                timeout = handler["timeout"]
                assert isinstance(timeout, int) and 0 < timeout <= 600, f"{event}: {timeout}"
                if event == "SessionEnd":
                    assert timeout <= 60, f"SessionEnd timeout {timeout}s exceeds the budget cap"


def test_claude_registers_map_freshness_events(tmp_path):
    _repo(tmp_path)
    _install_claude_hooks(tmp_path)
    settings = _claude_settings(tmp_path)
    for event, name in (
        ("PostToolBatch", "devcouncil-post-tool-batch"),
        ("FileChanged", "devcouncil-file-changed"),
        ("CwdChanged", "devcouncil-cwd-changed"),
        ("DirectoryAdded", "devcouncil-directory-added"),
    ):
        assert name in _hook_names(settings, event), f"{event} not registered"


def test_file_changed_group_has_no_matcher(tmp_path):
    """An omitted matcher matches every watched file and adds nothing to the list.

    A "*" matcher would be registered as a literal file named ``*``.
    """
    _repo(tmp_path)
    _install_claude_hooks(tmp_path)
    groups = _claude_settings(tmp_path)["hooks"]["FileChanged"]
    for group in groups:
        assert not group.get("matcher"), "FileChanged matcher would join the watch list"


def test_worktree_create_is_not_registered(tmp_path):
    """Registering WorktreeCreate replaces Claude Code's own git-worktree behavior."""
    _repo(tmp_path)
    _install_claude_hooks(tmp_path)
    assert "WorktreeCreate" not in _claude_settings(tmp_path).get("hooks", {})


def test_claude_only_events_do_not_leak_into_other_clients(tmp_path):
    _repo(tmp_path)
    _install_codex_hooks(tmp_path)
    _install_gemini_hooks(tmp_path)
    _install_grok_hooks(tmp_path)
    codex = json.loads((tmp_path / ".codex" / "hooks.json").read_text(encoding="utf-8"))
    gemini = json.loads((tmp_path / ".gemini" / "settings.json").read_text(encoding="utf-8"))
    grok = json.loads(
        (tmp_path / ".grok" / "hooks" / "devcouncil.json").read_text(encoding="utf-8")
    )
    for settings in (codex, gemini, grok):
        for event in ("PostToolBatch", "FileChanged", "CwdChanged", "DirectoryAdded"):
            assert event not in settings.get("hooks", {})


# --------------------------------------------------------------------------
# New events: behaviour
# --------------------------------------------------------------------------


def test_git_watch_paths_resolves_a_worktree_gitdir(tmp_path):
    """In a worktree ``.git`` is a file; HEAD lives in the linked gitdir."""
    main = tmp_path / "main"
    _repo(main)
    worktree = tmp_path / "wt"
    _git(main, "worktree", "add", str(worktree), "-b", "feature")
    paths = _git_watch_paths(worktree)
    assert paths, "no watch paths for a linked worktree"
    assert all(Path(p).is_absolute() for p in paths)
    assert any(Path(p).name == "HEAD" and Path(p).exists() for p in paths)


def test_session_start_emits_watch_paths(tmp_path):
    _repo(tmp_path)
    write_stamped_map(tmp_path)
    result = runner.invoke(
        hook_app,
        [
            "session-start",
            json.dumps({"hook_event_name": "SessionStart", "source": "startup", "cwd": str(tmp_path)}),
            "--project-root",
            str(tmp_path),
        ],
    )
    assert result.exit_code == 0, result.output
    payload = json.loads(result.stdout.strip().splitlines()[-1])
    watch = payload["hookSpecificOutput"]["watchPaths"]
    assert watch and all(Path(p).is_absolute() for p in watch)
    assert any(Path(p).name == "HEAD" for p in watch)


def test_post_tool_batch_refreshes_the_whole_batch_once(tmp_path, monkeypatch):
    _repo(tmp_path)
    (tmp_path / "pkg" / "b.py").write_text("def bar():\n    return 2\n", encoding="utf-8")
    _git(tmp_path, "add", "-A")
    _git(tmp_path, "commit", "-m", "b")
    write_stamped_map(tmp_path)

    # A build takes no path list -- the kernel decides what it revisits -- so what
    # these assert is how many builds ran, which is the property each is named for.
    calls: list[int] = []

    def _fake_refresh(root, output, *args, **kwargs):  # noqa: ANN001
        calls.append(1)
        from devcouncil.indexing.map_artifacts import GraphRefreshResult

        return GraphRefreshResult(map_path=output)

    monkeypatch.setattr(
        "devcouncil.indexing.map_artifacts.refresh_map_artifacts", _fake_refresh
    )
    payload = {
        "hook_event_name": "PostToolBatch",
        "cwd": str(tmp_path),
        "tool_calls": [
            {"tool_name": "Write", "tool_input": {"file_path": str(tmp_path / "pkg" / "a.py")}},
            {"tool_name": "Edit", "tool_input": {"file_path": str(tmp_path / "pkg" / "b.py")}},
        ],
    }
    result = runner.invoke(
        hook_app,
        ["post-tool-batch", json.dumps(payload), "--project-root", str(tmp_path)],
    )
    assert result.exit_code == 0, result.output
    assert len(calls) == 1, calls


def test_post_tool_use_defers_to_the_batch_hook(tmp_path, monkeypatch):
    """--defer-batch queues instead of refreshing, so N parallel tools cost one build."""
    _repo(tmp_path)
    write_stamped_map(tmp_path)

    # A build takes no path list -- the kernel decides what it revisits -- so what
    # these assert is how many builds ran, which is the property each is named for.
    calls: list[int] = []

    def _fake_refresh(root, output, *args, **kwargs):  # noqa: ANN001
        calls.append(1)
        from devcouncil.indexing.map_artifacts import GraphRefreshResult

        return GraphRefreshResult(map_path=output)

    monkeypatch.setattr(
        "devcouncil.indexing.map_artifacts.refresh_map_artifacts", _fake_refresh
    )
    payload = {
        "hook_event_name": "PostToolUse",
        "cwd": str(tmp_path),
        "tool_name": "Write",
        "tool_input": {"file_path": str(tmp_path / "pkg" / "a.py")},
    }
    result = runner.invoke(
        hook_app,
        [
            "post-tool-use",
            json.dumps(payload),
            "--client",
            "claude",
            "--project-root",
            str(tmp_path),
            "--defer-batch",
        ],
    )
    assert result.exit_code == 0, result.output
    assert calls == [], "deferred PostToolUse must not build the map itself"

    batch = {"hook_event_name": "PostToolBatch", "cwd": str(tmp_path), "tool_calls": []}
    result = runner.invoke(
        hook_app,
        ["post-tool-batch", json.dumps(batch), "--project-root", str(tmp_path)],
    )
    assert result.exit_code == 0, result.output
    assert len(calls) == 1, f"queued path was never drained: {calls}"


def test_deferred_post_tool_use_falls_back_when_no_batch_hook_drains(tmp_path, monkeypatch):
    """If PostToolBatch never fires, the queue must not strand the map forever."""
    import os
    import time

    from devcouncil.cli.commands.hook import DEFER_FALLBACK_S, _MAP_REFRESH_QUEUE_REL

    _repo(tmp_path)
    write_stamped_map(tmp_path)

    # A build takes no path list -- the kernel decides what it revisits -- so what
    # these assert is how many builds ran, which is the property each is named for.
    calls: list[int] = []

    def _fake_refresh(root, output, *args, **kwargs):  # noqa: ANN001
        calls.append(1)
        from devcouncil.indexing.map_artifacts import GraphRefreshResult

        return GraphRefreshResult(map_path=output)

    monkeypatch.setattr(
        "devcouncil.indexing.map_artifacts.refresh_map_artifacts", _fake_refresh
    )
    args = [
        "post-tool-use",
        json.dumps(
            {
                "hook_event_name": "PostToolUse",
                "cwd": str(tmp_path),
                "tool_name": "Write",
                "tool_input": {"file_path": str(tmp_path / "pkg" / "a.py")},
            }
        ),
        "--client",
        "claude",
        "--project-root",
        str(tmp_path),
        "--defer-batch",
    ]
    assert runner.invoke(hook_app, args).exit_code == 0
    assert calls == [], "first deferred call must only queue"

    # Age the queue past the point a batch hook would have drained it.
    queue = tmp_path / _MAP_REFRESH_QUEUE_REL
    old = time.time() - (DEFER_FALLBACK_S + 30)
    os.utime(queue, (old, old))

    assert runner.invoke(hook_app, args).exit_code == 0
    assert len(calls) == 1, f"stranded queue was never reclaimed: {calls}"


def test_file_changed_refreshes_files_that_moved_with_head(tmp_path, monkeypatch):
    """A branch switch rewrites files with no tool call; PostToolUse cannot see it."""
    _repo(tmp_path)
    write_stamped_map(tmp_path)
    head_before = subprocess.run(
        ["git", "rev-parse", "HEAD"], cwd=tmp_path, capture_output=True, text=True
    ).stdout.strip()
    map_path = tmp_path / ".devcouncil" / "repo_map.json"
    data = json.loads(map_path.read_text(encoding="utf-8"))
    data["generated_head"] = head_before
    map_path.write_text(json.dumps(data), encoding="utf-8")

    (tmp_path / "pkg" / "a.py").write_text("def foo():\n    return 99\n", encoding="utf-8")
    _git(tmp_path, "add", "-A")
    _git(tmp_path, "commit", "-m", "change a")

    # A build takes no path list -- the kernel decides what it revisits -- so what
    # these assert is how many builds ran, which is the property each is named for.
    calls: list[int] = []

    def _fake_refresh(root, output, *args, **kwargs):  # noqa: ANN001
        calls.append(1)
        from devcouncil.indexing.map_artifacts import GraphRefreshResult

        return GraphRefreshResult(map_path=output)

    monkeypatch.setattr(
        "devcouncil.indexing.map_artifacts.refresh_map_artifacts", _fake_refresh
    )
    payload = {
        "hook_event_name": "FileChanged",
        "cwd": str(tmp_path),
        "file_path": str(tmp_path / ".git" / "HEAD"),
        "event": "change",
    }
    result = runner.invoke(
        hook_app,
        ["file-changed", json.dumps(payload), "--project-root", str(tmp_path)],
    )
    assert result.exit_code == 0, result.output
    assert len(calls) == 1, calls


def test_cwd_changed_reseeds_watch_paths_for_the_new_repo(tmp_path):
    main = tmp_path / "main"
    other = tmp_path / "other"
    _repo(main)
    _repo(other)
    payload = {
        "hook_event_name": "CwdChanged",
        "cwd": str(other),
        "old_cwd": str(main),
        "new_cwd": str(other),
    }
    result = runner.invoke(
        hook_app, ["cwd-changed", json.dumps(payload), "--project-root", str(main)]
    )
    assert result.exit_code == 0, result.output
    watch = json.loads(result.stdout.strip().splitlines()[-1])["hookSpecificOutput"]["watchPaths"]
    assert watch, "entering a new repo must re-seed the watch list"
    assert all(str(other.resolve()) in p for p in watch)
    assert not any(str(main.resolve()) in p for p in watch)


def _cwd_changed_watch(main, new_cwd) -> list:
    payload = {
        "hook_event_name": "CwdChanged",
        "cwd": str(new_cwd),
        "old_cwd": str(main),
        "new_cwd": str(new_cwd),
    }
    result = runner.invoke(
        hook_app, ["cwd-changed", json.dumps(payload), "--project-root", str(main)]
    )
    assert result.exit_code == 0, result.output
    return json.loads(result.stdout.strip().splitlines()[-1])["hookSpecificOutput"]["watchPaths"]


def test_cwd_changed_keeps_watching_the_repo_it_never_left(tmp_path):
    """``cd src`` must not disarm FileChanged for the rest of the session.

    ``cwd_changed`` resolved the root itself, requiring ``.devcouncil/`` literally in
    ``new_cwd`` with no walk upward, so a plain subdirectory read as "no project" and
    the empty array cleared git's HEAD/index watch — while every other hook went on
    acting on the same repo through ``_effective_root``. A later branch switch or pull
    then fired nothing. Fails against the pre-fix code with ``watchPaths == []``.
    """
    main = tmp_path / "main"
    _repo(main)
    inside = _cwd_changed_watch(main, main / "pkg")
    assert inside, "a plain subdirectory of the session's own repo must keep the watch"
    assert all(str(main.resolve()) in path for path in inside)
    assert inside == _cwd_changed_watch(main, main)


def test_cwd_changed_keeps_the_session_repo_when_cwd_leaves_it(tmp_path):
    """An unrelated directory is not a repo change: DevCouncil's root did not move.

    ``_effective_root`` keeps the baked root for a stray ``cwd``, so PostToolUse still
    refreshes *this* map — clearing the watch list would disarm FileChanged for a repo
    the session is still maintaining.
    """
    main = tmp_path / "main"
    _repo(main)
    stray = tmp_path / "stray"
    stray.mkdir()
    assert _cwd_changed_watch(main, stray) == _cwd_changed_watch(main, main)


def test_cwd_changed_clears_watch_paths_when_the_resolved_root_has_no_git(tmp_path):
    """An empty array clears the dynamic list -- there is nothing to watch."""
    main = tmp_path / "main"
    (main / ".devcouncil").mkdir(parents=True)
    stray = tmp_path / "stray"
    stray.mkdir()
    assert _cwd_changed_watch(main, stray) == []


def test_directory_added_reports_that_the_map_does_not_cover_it(tmp_path):
    main = tmp_path / "main"
    added = tmp_path / "added"
    _repo(main)
    added.mkdir()
    payload = {
        "hook_event_name": "DirectoryAdded",
        "cwd": str(main),
        "directory": str(added),
        "source": "slash_command",
    }
    result = runner.invoke(
        hook_app, ["directory-added", json.dumps(payload), "--project-root", str(main)]
    )
    assert result.exit_code == 0, result.output
    message = json.loads(result.stdout.strip().splitlines()[-1])["systemMessage"]
    assert str(added) in message
    assert "map" in message.lower()


def test_hook_events_never_exit_nonzero_on_bad_payload(tmp_path):
    """Only exit 2 blocks; a crashing lifecycle hook must not invent an exit code."""
    _repo(tmp_path)
    for command in ("post-tool-batch", "file-changed", "cwd-changed", "directory-added"):
        result = runner.invoke(
            hook_app, [command, "not json at all", "--project-root", str(tmp_path)]
        )
        assert result.exit_code == 0, f"{command}: {result.output}"


# --------------------------------------------------------------------------
# The emitted hook table must name events, slugs and timeouts Claude Code
# actually honors -- a wrong one is *silently ignored* at runtime
# (reproduced: `claude plugin validate` 2.1.259 reports
# "hooks.<Event>: unknown hook event; entry ignored at runtime"), so a writer
# that does not check reports the same success for a dead hook as for a live one.
# --------------------------------------------------------------------------


def _spec_with(**overrides):
    from devcouncil.integrations.clients.hooks import ClaudeHookSpec

    base = {
        "event": "PostToolUse",
        "matcher": "",
        "hook_event": "post-tool-use",
        "name": "devcouncil-test",
    }
    base.update(overrides)
    return ClaudeHookSpec(**base)


def test_settings_writer_refuses_an_event_claude_code_does_not_know(tmp_path, monkeypatch):
    """A typo'd event is dropped at runtime; the install must not report success."""
    from devcouncil.integrations.clients import hooks as hooks_mod

    (tmp_path / ".devcouncil").mkdir(parents=True)
    monkeypatch.setattr(
        hooks_mod, "CLAUDE_HOOK_SPECS", (_spec_with(event="PostToolUseX"),), raising=True
    )
    try:
        hooks_mod._install_claude_hooks(tmp_path)
    except ValueError as exc:
        assert "PostToolUseX" in str(exc)
    else:
        raise AssertionError("writer accepted an unknown Claude Code hook event")
    assert not (tmp_path / ".claude" / "settings.local.json").exists()


def test_settings_writer_refuses_a_slug_with_no_hook_command(tmp_path, monkeypatch):
    """`devcouncil hook <slug>` must exist, or the hook fails on every fire."""
    from devcouncil.integrations.clients import hooks as hooks_mod

    (tmp_path / ".devcouncil").mkdir(parents=True)
    monkeypatch.setattr(
        hooks_mod, "CLAUDE_HOOK_SPECS", (_spec_with(hook_event="no-such-slug"),), raising=True
    )
    try:
        hooks_mod._install_claude_hooks(tmp_path)
    except ValueError as exc:
        assert "no-such-slug" in str(exc)
    else:
        raise AssertionError("writer accepted a hook slug with no CLI command")


def test_settings_writer_refuses_a_non_positive_timeout(tmp_path, monkeypatch):
    """Claude Code's schema is `timeout: number().positive()`; 0 drops the entry."""
    from devcouncil.integrations.clients import hooks as hooks_mod

    (tmp_path / ".devcouncil").mkdir(parents=True)
    monkeypatch.setattr(hooks_mod, "DEFAULT_HOOK_TIMEOUT_SECONDS", 0, raising=True)
    monkeypatch.setattr(hooks_mod, "CLAUDE_HOOK_SPECS", (_spec_with(),), raising=True)
    try:
        hooks_mod._install_claude_hooks(tmp_path)
    except ValueError as exc:
        assert "timeout" in str(exc).lower()
    else:
        raise AssertionError("writer accepted a non-positive hook timeout")


def test_plugin_hooks_json_refuses_an_event_claude_code_does_not_know(tmp_path, monkeypatch):
    """The plugin bundle is the artifact other people install; same gate."""
    from devcouncil.integrations import claude_assets
    from devcouncil.integrations.clients import hooks as hooks_mod

    monkeypatch.setattr(
        hooks_mod, "CLAUDE_HOOK_SPECS", (_spec_with(event="BeforeTool"),), raising=True
    )
    try:
        claude_assets._plugin_hooks_json(tmp_path)
    except ValueError as exc:
        assert "BeforeTool" in str(exc)
    else:
        raise AssertionError("plugin writer accepted an unknown Claude Code hook event")


def test_shipped_hook_table_passes_its_own_gate():
    """The real table must satisfy the check, not just the synthetic ones."""
    from devcouncil.integrations.clients.hooks import (
        CLAUDE_HOOK_EVENTS,
        CLAUDE_HOOK_SPECS,
    )

    assert {spec.event for spec in CLAUDE_HOOK_SPECS} <= CLAUDE_HOOK_EVENTS


# --------------------------------------------------------------------------
# StopFailure: the turn can end without Stop ever firing
# --------------------------------------------------------------------------


def test_stop_failure_hook_is_registered(tmp_path):
    """StopFailure fires *instead of* Stop on an API error, so the stop gate
    never runs -- and nothing recorded that it did not."""
    from devcouncil.integrations.clients.hooks import _install_claude_hooks

    (tmp_path / ".devcouncil").mkdir(parents=True)
    _install_claude_hooks(tmp_path)
    assert "devcouncil-stop-failure" in _hook_names(_claude_settings(tmp_path), "StopFailure")


def test_stop_failure_records_that_the_stop_gate_did_not_evaluate(tmp_path):
    _repo(tmp_path)
    payload = {
        "hook_event_name": "StopFailure",
        "cwd": str(tmp_path),
        "error": "rate_limit",
    }
    result = runner.invoke(
        hook_app, ["stop-failure", json.dumps(payload), "--project-root", str(tmp_path)]
    )
    assert result.exit_code == 0, result.output
    trace = (tmp_path / ".devcouncil" / "logs" / "traces.jsonl").read_text(encoding="utf-8")
    assert "stop_gate_not_evaluated" in trace
    assert "rate_limit" in trace


# --------------------------------------------------------------------------
# SubagentStart: a subagent is gated on stop by rules it was never told
# --------------------------------------------------------------------------


def test_subagent_start_hook_is_registered(tmp_path):
    from devcouncil.integrations.clients.hooks import _install_claude_hooks

    (tmp_path / ".devcouncil").mkdir(parents=True)
    _install_claude_hooks(tmp_path)
    assert "devcouncil-subagent-start" in _hook_names(
        _claude_settings(tmp_path), "SubagentStart"
    )


def test_subagent_start_names_the_task_the_subagent_will_be_gated_on(tmp_path):
    """Exit 0 + additionalContext is the only channel a subagent can read.

    SubagentStop holds the subagent to the active task; this asserts the subagent is
    actually told which task that is, rather than just that *something* was emitted.
    """
    from devcouncil.domain.task import PlannedFile, Task
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import TaskRepository

    _repo(tmp_path)
    # `session_briefing` reads the stop-gate config, so a repo without one falls all
    # the way back to the bare status line and the active task never appears.
    (tmp_path / ".devcouncil" / "config.yaml").write_text(
        "project:\n  name: test\n", encoding="utf-8"
    )
    db = get_db(tmp_path)
    with db.get_session() as session:
        TaskRepository(session).save(
            Task(
                id="TASK-042",
                title="Implement feature",
                description="Do the thing",
                status="running",
                planned_files=[
                    PlannedFile(path="pkg/a.py", reason="logic", allowed_change="modify")
                ],
            )
        )

    payload = {
        "hook_event_name": "SubagentStart",
        "cwd": str(tmp_path),
        "agent_id": "a1",
        "agent_type": "general-purpose",
    }
    result = runner.invoke(
        hook_app, ["subagent-start", json.dumps(payload), "--project-root", str(tmp_path)]
    )
    assert result.exit_code == 0, result.output
    emitted = json.loads(result.stdout.strip().splitlines()[-1])
    assert emitted["hookSpecificOutput"]["hookEventName"] == "SubagentStart"
    assert "TASK-042" in emitted["hookSpecificOutput"]["additionalContext"]


# --------------------------------------------------------------------------
# statusMessage: the blocking hooks make the user wait with no explanation
# --------------------------------------------------------------------------


def test_blocking_hooks_declare_a_status_message(tmp_path):
    """`statusMessage` names the spinner; the stop gate can hold a turn 150s."""
    from devcouncil.integrations.clients.hooks import _install_claude_hooks

    (tmp_path / ".devcouncil").mkdir(parents=True)
    _install_claude_hooks(tmp_path, write_gate=True)
    settings = _claude_settings(tmp_path)
    for event, name in (
        ("Stop", "devcouncil-agent-response-ready"),
        ("SubagentStop", "devcouncil-subagent-stop"),
        ("PreToolUse", "devcouncil-pre-tool-use"),
    ):
        handlers = [
            handler
            for group in settings["hooks"][event]
            for handler in group["hooks"]
            if handler.get("name") == name
        ]
        assert handlers, f"{event}/{name} not installed"
        assert handlers[0].get("statusMessage"), f"{event}/{name} has no statusMessage"
