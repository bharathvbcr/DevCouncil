"""The session guard: who else is working in this repository right now?

Two agent sessions ran the same goal on this repository at the same time — one
in the main checkout, one in a worktree — and neither knew about the other for
hours. Each test below builds a real temporary repository (worktrees and all)
and asks the detector the question a starting session needs answered, including
the question "what happens when git cannot answer at all".
"""

from __future__ import annotations

import os
import subprocess
import time
from pathlib import Path

from devcouncil.utils.git_siblings import (
    DivergentBranch,
    SiblingCheckout,
    SessionGuardReport,
    inspect_session_siblings,
    session_guard_hints,
)


def _git(root: Path, *args: str) -> str:
    proc = subprocess.run(
        ["git", "-c", "user.email=t@t", "-c", "user.name=t", *args],
        cwd=root,
        check=True,
        capture_output=True,
        text=True,
    )
    return proc.stdout


def _repo(root: Path) -> Path:
    root.mkdir(parents=True, exist_ok=True)
    _git(root, "init", "-b", "main")
    (root / "a.txt").write_text("one\n", encoding="utf-8")
    _git(root, "add", "-A")
    _git(root, "commit", "-m", "init")
    return root


def _age(path: Path, seconds: float) -> None:
    """Backdate everything under *path* so nothing there looks recently touched."""
    stamp = time.time() - seconds
    targets = [path, *path.rglob("*")]
    for target in targets:
        try:
            os.utime(target, (stamp, stamp))
        except OSError:
            pass


def _age_checkout(main: Path, worktree: Path, seconds: float) -> None:
    """Backdate a linked worktree *and* the git metadata git keeps for it."""
    _age(worktree, seconds)
    gitdir_file = worktree / ".git"
    if gitdir_file.is_file():
        text = gitdir_file.read_text(encoding="utf-8").strip()
        if text.startswith("gitdir:"):
            _age(Path(text.split(":", 1)[1].strip()), seconds)
    _age(main / ".git", seconds)


def test_dirty_sibling_worktree_is_reported_live(tmp_path: Path) -> None:
    main = _repo(tmp_path / "main")
    sibling = tmp_path / "wt"
    _git(main, "worktree", "add", "-b", "claude/side", str(sibling))
    # Backdate first, so the *only* live signal left is the dirty file: this
    # pins the `git status` tier rather than the cheap git-metadata tier.
    _age_checkout(main, sibling, seconds=6 * 3600)
    (sibling / "a.txt").write_text("edited by the other session\n", encoding="utf-8")

    report = inspect_session_siblings(main)

    assert report.unavailable == ()
    paths = [Path(s.path).name for s in report.live_siblings]
    assert paths == ["wt"], report
    only = report.live_siblings[0]
    assert only.branch == "claude/side"
    assert "uncommitted" in only.reason


def test_recently_active_but_clean_sibling_is_reported_live(tmp_path: Path) -> None:
    main = _repo(tmp_path / "main")
    sibling = tmp_path / "wt"
    _git(main, "worktree", "add", "-b", "claude/side", str(sibling))

    report = inspect_session_siblings(main)

    assert [Path(s.path).name for s in report.live_siblings] == ["wt"], report
    assert report.live_siblings[0].reason == "recent git activity"


def test_a_file_touched_without_any_git_command_still_counts(tmp_path: Path) -> None:
    main = _repo(tmp_path / "main")
    sibling = tmp_path / "wt"
    _git(main, "worktree", "add", "-b", "quiet", str(sibling))
    _age_checkout(main, sibling, seconds=6 * 3600)
    # Same bytes, new mtime: not dirty, no git command run, still touched.
    os.utime(sibling / "a.txt", None)

    report = inspect_session_siblings(main)

    assert [s.reason for s in report.live_siblings] == ["recently edited"], report


def test_clean_and_untouched_sibling_is_not_live(tmp_path: Path) -> None:
    main = _repo(tmp_path / "main")
    sibling = tmp_path / "wt"
    _git(main, "worktree", "add", "-b", "quiet", str(sibling))
    _age_checkout(main, sibling, seconds=6 * 3600)

    report = inspect_session_siblings(main, window_minutes=30)

    assert report.unavailable == ()
    assert report.live_siblings == (), report


def test_current_checkout_is_never_its_own_sibling(tmp_path: Path) -> None:
    main = _repo(tmp_path / "main")
    (main / "a.txt").write_text("dirty right here\n", encoding="utf-8")

    report = inspect_session_siblings(main)

    assert report.live_siblings == ()


def test_a_subdirectory_is_still_this_checkout(tmp_path: Path) -> None:
    main = _repo(tmp_path / "main")
    (main / "pkg").mkdir()
    (main / "pkg" / "b.txt").write_text("dirty right here\n", encoding="utf-8")

    report = inspect_session_siblings(main / "pkg")

    assert report.live_siblings == (), report
    assert report.current_branch == "main"


def test_a_nested_worktree_is_not_confused_with_its_parent(tmp_path: Path) -> None:
    """A worktree living *inside* the main checkout is the trap this repository has."""
    main = _repo(tmp_path / "main")
    nested = main / ".claude" / "worktrees" / "wt"
    _git(main, "worktree", "add", "-b", "claude/nested", str(nested))
    (nested / "deep").mkdir()

    report = inspect_session_siblings(nested / "deep")

    assert report.current_branch == "claude/nested"
    assert [Path(s.path).name for s in report.live_siblings] == ["main"], report


def test_branch_three_ahead_of_default_is_divergent(tmp_path: Path) -> None:
    main = _repo(tmp_path / "main")
    _git(main, "checkout", "-b", "claude/x")
    for i in range(3):
        (main / f"f{i}.txt").write_text(f"{i}\n", encoding="utf-8")
        _git(main, "add", "-A")
        _git(main, "commit", "-m", f"c{i}")
    _git(main, "checkout", "main")

    report = inspect_session_siblings(main)

    assert report.default_branch == "main"
    assert report.unavailable == ()
    assert [(b.name, b.ahead) for b in report.divergent_branches] == [("claude/x", 3)]
    assert report.divergent_branches[0].tip_age_seconds is not None


def test_a_git_without_ahead_behind_falls_back_to_rev_list(tmp_path: Path, monkeypatch) -> None:
    """`%(ahead-behind:)` needs git 2.41; the fallback must give the same answer."""
    from devcouncil.utils import git_siblings

    main = _repo(tmp_path / "main")
    _git(main, "checkout", "-b", "claude/x")
    for i in range(3):
        (main / f"f{i}.txt").write_text(f"{i}\n", encoding="utf-8")
        _git(main, "add", "-A")
        _git(main, "commit", "-m", f"c{i}")
    _git(main, "checkout", "main")
    for i in range(2):
        (main / f"m{i}.txt").write_text(f"{i}\n", encoding="utf-8")
        _git(main, "add", "-A")
        _git(main, "commit", "-m", f"m{i}")
    _git(main, "checkout", "-b", "side")

    real = git_siblings._git_text

    def old_git(args, cwd, *, timeout=git_siblings.GIT_PROBE_TIMEOUT):
        if any("ahead-behind" in arg for arg in args):
            return None, "git for-each-ref exited 128: fatal: unknown field name: ahead-behind"
        return real(args, cwd, timeout=timeout)

    monkeypatch.setattr(git_siblings, "_git_text", old_git)
    report = inspect_session_siblings(main)

    assert [(b.name, b.ahead) for b in report.divergent_branches] == [("claude/x", 3)]
    assert report.behind == 0  # `side` was branched from the tip of main
    assert report.unavailable == ()


def test_more_branches_than_the_cap_declares_the_truncation(tmp_path: Path) -> None:
    from devcouncil.utils import git_siblings

    main = _repo(tmp_path / "main")
    for i in range(git_siblings.MAX_BRANCHES + 3):
        _git(main, "branch", f"claude/b{i:02d}")

    report = inspect_session_siblings(main)

    assert report.truncated, "a capped listing must say it was capped"
    assert f"first {git_siblings.MAX_BRANCHES}" in report.truncated[0]
    assert "partial:" in "; ".join(session_guard_hints(report))


def test_the_branch_you_are_on_is_not_reported_as_divergent(tmp_path: Path) -> None:
    main = _repo(tmp_path / "main")
    _git(main, "checkout", "-b", "claude/x")
    (main / "f.txt").write_text("1\n", encoding="utf-8")
    _git(main, "add", "-A")
    _git(main, "commit", "-m", "c")

    report = inspect_session_siblings(main)

    assert report.divergent_branches == ()


def test_head_two_behind_default_is_reported(tmp_path: Path) -> None:
    main = _repo(tmp_path / "main")
    _git(main, "checkout", "-b", "side")
    _git(main, "checkout", "main")
    for i in range(2):
        (main / f"m{i}.txt").write_text(f"{i}\n", encoding="utf-8")
        _git(main, "add", "-A")
        _git(main, "commit", "-m", f"m{i}")
    _git(main, "checkout", "side")

    report = inspect_session_siblings(main)

    assert report.behind == 2
    assert report.default_branch == "main"


def test_up_to_date_checkout_is_behind_zero(tmp_path: Path) -> None:
    main = _repo(tmp_path / "main")

    report = inspect_session_siblings(main)

    assert report.behind == 0
    assert report.live_siblings == ()
    assert report.divergent_branches == ()
    assert report.unavailable == ()


def test_detached_head_still_answers_behind(tmp_path: Path) -> None:
    main = _repo(tmp_path / "main")
    first = _git(main, "rev-parse", "HEAD").strip()
    for i in range(2):
        (main / f"m{i}.txt").write_text(f"{i}\n", encoding="utf-8")
        _git(main, "add", "-A")
        _git(main, "commit", "-m", f"m{i}")
    _git(main, "checkout", "--detach", first)

    report = inspect_session_siblings(main)

    assert report.current_branch is None
    assert report.behind == 2, report
    assert report.unavailable == ()


def test_a_repository_without_a_default_branch_says_so(tmp_path: Path) -> None:
    root = tmp_path / "odd"
    root.mkdir()
    _git(root, "init", "-b", "trunk")
    (root / "a.txt").write_text("one\n", encoding="utf-8")
    _git(root, "add", "-A")
    _git(root, "commit", "-m", "init")

    report = inspect_session_siblings(root)

    assert report.default_branch is None
    assert report.behind is None
    assert any("no default branch" in reason for reason in report.unavailable), report
    assert "could not check" in "; ".join(session_guard_hints(report))


def test_git_that_cannot_run_says_so_instead_of_none(tmp_path: Path, monkeypatch) -> None:
    main = _repo(tmp_path / "main")
    # No git on PATH: the probe cannot run. "No siblings" would be a lie.
    monkeypatch.setenv("PATH", str(tmp_path / "empty-bin"))

    report = inspect_session_siblings(main)

    assert report.unavailable, report
    assert report.live_siblings == ()
    assert report.divergent_branches == ()
    assert report.behind is None
    hints = session_guard_hints(report)
    assert hints and "could not check" in hints[0]


def test_a_directory_that_is_not_a_repository_is_an_answer_not_a_failure(tmp_path: Path) -> None:
    plain = tmp_path / "plain"
    plain.mkdir()

    report = inspect_session_siblings(plain)

    assert report.unavailable == ()
    assert report.live_siblings == ()
    assert session_guard_hints(report) == []


def test_hints_are_capped_in_count_and_length() -> None:
    siblings = tuple(
        SiblingCheckout(
            path=f"/very/long/path/that/goes/on/and/on/checkout-{i}-{'x' * 40}",
            branch=f"claude/branch-{i}-{'y' * 40}",
            reason="uncommitted changes",
            age_seconds=60.0,
        )
        for i in range(9)
    )
    branches = tuple(
        DivergentBranch(name=f"claude/b{i}-{'z' * 60}", ahead=i + 1, tip_age_seconds=3600.0)
        for i in range(9)
    )
    report = SessionGuardReport(
        live_siblings=siblings,
        divergent_branches=branches,
        behind=4,
        default_branch="main",
    )

    hints = session_guard_hints(report)

    assert hints
    for hint in hints:
        assert len(hint) <= 200, hint
    joined = "; ".join(hints)
    assert "+7 more" in joined
    assert "4 behind main" in joined


def test_oversized_git_output_is_bounded() -> None:
    """A git that prints megabytes must not put megabytes into the report."""
    from devcouncil.utils import git_siblings

    bounded = git_siblings._bound_text("#" * (git_siblings.MAX_GIT_OUTPUT_CHARS * 4))

    assert len(bounded) <= git_siblings.MAX_GIT_OUTPUT_CHARS


def test_a_flood_of_worktrees_is_capped_and_says_it_was(tmp_path: Path) -> None:
    from devcouncil.utils import git_siblings

    porcelain = "".join(
        f"worktree {tmp_path / f'wt{i}'}\nHEAD {'0' * 40}\nbranch refs/heads/b{i}\n\n"
        for i in range(git_siblings.MAX_SIBLINGS * 5)
    )

    entries, truncated = git_siblings._parse_worktrees(porcelain)

    assert len(entries) == git_siblings.MAX_SIBLINGS
    assert truncated is True
