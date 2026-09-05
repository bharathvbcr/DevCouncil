"""The PreToolUse write gate must authorize against the repo the work is in.

SECURITY IMPACT — this test pins a deliberate, user-approved change to an
authorization decision.

`pre_tool_use` is the write gate: the project root it resolves decides whose
lease, whose allowlist and whose active task authorize a tool call. It used to
use the *baked* `--project-root` (the directory the session started in, which
does not follow Claude Code into a git worktree) while all 13 other hooks
followed the payload's `cwd`. Inside a worktree the gate therefore evaluated a
**different repository's** active task than the one being worked on — which can
wrongly block legitimate work, and can wrongly allow work the worktree's own
task scope forbids.

What this widens, stated plainly: a worktree that contains its own initialized
`.devcouncil/` now governs its own writes, so a permissive policy there is no
longer overridden by the parent's. `_effective_root` switches only when the
payload's `cwd` resolves to a directory that is *itself* an initialized
DevCouncil project and differs from the baked root — a plain subdirectory, an
absent `cwd`, or an unrelated tree all keep the baked root. Those negative
cases are asserted below, because they are what stops this from becoming a
general escape from the gate.
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from devcouncil.cli.commands.hook import _effective_root


def _project(base: Path, name: str) -> Path:
    root = base / name
    (root / ".devcouncil").mkdir(parents=True)
    return root.resolve()


def test_an_initialized_worktree_owns_its_own_write_authorization(tmp_path):
    parent = _project(tmp_path, "parent")
    worktree = _project(parent / ".claude" / "worktrees", "wt")

    assert _effective_root(parent, {"cwd": str(worktree)}) == worktree, (
        "the write gate stayed on the parent repo while the session was working "
        "in an initialized worktree, so it authorized against a different "
        "repository's lease and active task"
    )


def test_a_plain_subdirectory_does_not_become_its_own_authority(tmp_path):
    """The guard that keeps this from being a general escape from the gate."""
    parent = _project(tmp_path, "parent")
    plain = parent / "src" / "devcouncil"
    plain.mkdir(parents=True)

    assert _effective_root(parent, {"cwd": str(plain)}) == parent


def test_an_uninitialized_worktree_does_not_become_its_own_authority(tmp_path):
    parent = _project(tmp_path, "parent")
    bare = parent / ".claude" / "worktrees" / "bare"
    bare.mkdir(parents=True)

    assert _effective_root(parent, {"cwd": str(bare)}) == parent, (
        "a directory with no .devcouncil/ was allowed to authorize its own "
        "writes; only an initialized project may"
    )


@pytest.mark.parametrize("payload", [{}, {"cwd": ""}, {"cwd": "   "}, {"cwd": None}, "not-a-dict", None])
def test_a_missing_or_unusable_cwd_keeps_the_baked_root(tmp_path, payload):
    parent = _project(tmp_path, "parent")
    assert _effective_root(parent, payload) == parent


def test_the_gate_command_resolves_the_worktree_root(tmp_path, monkeypatch):
    """End-to-end through `pre_tool_use`, not just the helper.

    Asserting on the helper alone would pass even if the gate never called it —
    which is exactly the state this change fixes.
    """
    from devcouncil.cli.commands import hook as hookmod

    parent = _project(tmp_path, "parent")
    worktree = _project(parent / ".claude" / "worktrees", "wt")
    seen: list[Path] = []

    class _Decision:
        action = "allow"
        reason = "ok"

    class _Policy:
        def __init__(self, project_root: Path):
            seen.append(project_root)

        def evaluate(self, call_data, active_task):
            return _Decision()

    monkeypatch.setattr(hookmod, "HookPolicy", _Policy)
    monkeypatch.setattr(hookmod, "_active_task", lambda root: None)
    monkeypatch.setattr(hookmod, "_emit_decision", lambda *a, **k: None)

    hookmod.pre_tool_use(
        tool_call_json=json.dumps({"cwd": str(worktree), "tool_name": "Write"}),
        client="claude",
        project_root=parent,
        strict=False,
    )

    assert seen == [worktree], (
        f"the gate evaluated {seen} instead of the worktree it was working in"
    )
