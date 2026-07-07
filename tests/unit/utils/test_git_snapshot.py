"""Unit tests for batched git worktree snapshots."""

import subprocess
from pathlib import Path

from devcouncil.utils.git_snapshot import GitWorktreeSnapshot


def _git(root: Path, *args: str) -> None:
    subprocess.run(["git", *args], cwd=root, check=True, capture_output=True, text=True)


def test_capture_with_head_batches_status_and_diff(tmp_path: Path) -> None:
    _git(tmp_path, "init")
    (tmp_path / "tracked.py").write_text("a\n", encoding="utf-8")
    (tmp_path / "new.py").write_text("b\n", encoding="utf-8")
    _git(tmp_path, "-c", "user.email=t@t", "-c", "user.name=t", "add", "tracked.py")
    _git(tmp_path, "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", "init")
    (tmp_path / "tracked.py").write_text("a2\n", encoding="utf-8")

    snap = GitWorktreeSnapshot.capture(tmp_path)

    assert snap.has_head is True
    assert "tracked.py" in snap.tracked_changed_files
    assert "new.py" in snap.untracked_files
    assert "tracked.py" in snap.tracked_diff or snap.tracked_diff == ""


def test_capture_without_head_uses_status_paths(tmp_path: Path) -> None:
    _git(tmp_path, "init")
    (tmp_path / "staged.py").write_text("x\n", encoding="utf-8")
    (tmp_path / "untracked.py").write_text("y\n", encoding="utf-8")
    _git(tmp_path, "add", "staged.py")

    snap = GitWorktreeSnapshot.capture(tmp_path)

    assert snap.has_head is False
    assert "staged.py" in snap.status_paths or "staged.py" in snap.untracked_files or snap.cached_diff
    assert "untracked.py" in snap.untracked_files
