"""Shared project-tree walk helpers with standard ignore rules.

NOTE (Phase 4 legacy audit): still used by ``semantic_index`` and
``repo_mapper`` — not superseded by ``indexing/graph/``. Keep.
"""

from __future__ import annotations

from collections.abc import Iterator
from pathlib import Path

IGNORED_DIR_NAMES = frozenset({
    ".git",
    ".devcouncil",
    "__pycache__",
    ".venv",
    "venv",
    "node_modules",
    "dist",
    "build",
    "target",
    "vendor",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    ".tox",
    "htmlcov",
    ".eggs",
})


# Nested working trees of this same repository (Claude Code agent worktrees).
# Their files are copies of the main tree: indexing them duplicates symbols and
# belongs to the worktree checkout, never to the enclosing root's graph.
NESTED_CHECKOUT_SEGMENTS: tuple[tuple[str, ...], ...] = ((".claude", "worktrees"),)


def _under_nested_checkout(parts: tuple[str, ...]) -> bool:
    for segment in NESTED_CHECKOUT_SEGMENTS:
        span = len(segment)
        for i in range(len(parts) - span + 1):
            if parts[i : i + span] == segment:
                return True
    return False


def should_skip_path(path: Path | str) -> bool:
    """Return True when ``path`` is under a directory that should be pruned from walks."""
    parts = Path(path).parts
    if any(part in IGNORED_DIR_NAMES for part in parts):
        return True
    return _under_nested_checkout(parts)


def iter_project_files(project_root: Path) -> Iterator[Path]:
    """Yield regular files under ``project_root``, skipping ignored directories."""
    root = project_root.resolve()
    for dirpath, dirnames, filenames in root.walk(on_error=lambda _: None):
        rel_dir = dirpath.relative_to(root)
        dirnames[:] = [
            name
            for name in dirnames
            if name not in IGNORED_DIR_NAMES
            and not _under_nested_checkout((*rel_dir.parts, name))
        ]
        if should_skip_path(rel_dir):
            continue
        for name in filenames:
            file_path = dirpath / name
            rel = file_path.relative_to(root)
            if should_skip_path(rel):
                continue
            if file_path.is_file():
                yield file_path
