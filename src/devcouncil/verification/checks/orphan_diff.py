"""Orphan-diff detection helpers extracted from Verifier."""

from __future__ import annotations

import logging
import subprocess
from pathlib import Path
from typing import Callable, List, Tuple

from devcouncil.domain.gap import Gap
from devcouncil.domain.task import Task
from devcouncil.indexing.wiring import is_test_path
from devcouncil.verification.stub_detector import detect_stubs

logger = logging.getLogger(__name__)


def classify_change_paths(
    project_root: Path,
    changed_files: List[str],
    get_untracked_files: Callable[[], List[str]],
) -> Tuple[List[str], List[str]]:
    """Return (added, deleted) paths intersecting changed_files."""
    changed_set = set(changed_files)
    added = set(get_untracked_files())
    deleted: set[str] = set()
    try:
        from devcouncil.utils.proc import GIT_TIMEOUT

        output = subprocess.check_output(
            ["git", "diff", "HEAD", "--name-status"],
            cwd=project_root,
            stderr=subprocess.DEVNULL,
            timeout=GIT_TIMEOUT,
        ).decode("utf-8", errors="replace").splitlines()
        for line in output:
            parts = line.split("\t")
            if len(parts) < 2:
                continue
            status = parts[0]
            path = parts[-1].replace("\\", "/")
            if status.startswith("A"):
                added.add(path)
            elif status.startswith("D"):
                deleted.add(path)
    except Exception as e:
        logger.debug("Failed to classify changed files: %s", e)
    return sorted(added & changed_set), sorted(deleted & changed_set)


def detect_orphan_diff_gaps(
    *,
    task: Task,
    changed_files: List[str],
    planned_paths: set[str],
    diff_content: str,
    project_root: Path,
    get_untracked_files: Callable[[], List[str]],
    next_gap_id: Callable[[str, str], str],
    classify_fn: Callable[[List[str]], Tuple[List[str], List[str]]] | None = None,
) -> List[Gap]:
    """Detect orphan-diff gaps for files changed outside the task plan."""
    gaps: List[Gap] = []
    if classify_fn is not None:
        orphan_added, _orphan_deleted = classify_fn(changed_files)
    else:
        orphan_added, _orphan_deleted = classify_change_paths(
            project_root, changed_files, get_untracked_files,
        )
    assert_free_test_files: set[str] = set()
    if diff_content:
        for finding in detect_stubs(project_root, diff_content, honor_allow_stub=False):
            if "no assertions" in finding.reason:
                assert_free_test_files.add(finding.file)
    for cf in changed_files:
        if cf not in planned_paths:
            new_test_file = (
                cf in orphan_added
                and is_test_path(cf)
                and cf not in assert_free_test_files
            )
            gaps.append(Gap(
                id=next_gap_id(task.id, f"ORPHAN-{cf}"),
                severity="medium" if new_test_file else "high",
                gap_type="orphan_diff",
                task_id=task.id,
                description=(
                    f"New test file {cf} was added but not planned for this task "
                    "(advisory: added tests cannot change shipped behavior)."
                    if new_test_file else
                    f"File {cf} was modified but not planned for this task."
                ),
                evidence=[cf],
                recommended_fix=(
                    f"Append {cf} with `dev scope update <task_id> --lease-token <token> "
                    f"--planned-file {cf}` (or fold the tests into a planned test file)."
                    if new_test_file else
                    f"Revert changes to {cf} or append it with "
                    f"`dev scope update <task_id> --lease-token <token> --planned-file {cf}`."
                ),
                blocking=not new_test_file,
                file=cf,
            ))
    return gaps
