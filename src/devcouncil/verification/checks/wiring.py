"""Unwired-file gate: flag newly added modules nothing (non-test) imports.

Diff-scoped — only files this task *added*. Requires an importer outside the
task's added set that is not a test file. Shares exemptions/entry-roots with
``indexing/wiring.py``. Never raises.
"""

from __future__ import annotations

import logging
from pathlib import Path
from typing import Callable, List, Optional, Set

from devcouncil.domain.gap import Gap
from devcouncil.domain.task import Task
from devcouncil.indexing.wiring import (
    ALLOW_UNWIRED,
    build_dynamic_import_index,
    entry_roots,
    has_allow_unwired,
    is_liveness_code_file,
    is_test_path,
    reference_cleared,
    structural_exemptions,
)
from devcouncil.verification.checks.orphan_diff import classify_change_paths

logger = logging.getLogger(__name__)


def _norm(path: str) -> str:
    s = path.replace("\\", "/")
    while s.startswith("./"):
        s = s[2:]
    return s


def added_files_from_diff(diff_content: str) -> Set[str]:
    """Paths introduced via ``--- /dev/null`` / ``+++ b/...`` headers."""
    added: Set[str] = set()
    pending_new = False
    for raw in (diff_content or "").splitlines():
        if raw.startswith("--- "):
            target = raw[4:].strip()
            pending_new = target == "/dev/null"
            continue
        if raw.startswith("+++ ") and pending_new:
            target = raw[4:].strip()
            pending_new = False
            if target == "/dev/null":
                continue
            path = target[2:] if target.startswith(("a/", "b/")) else target
            added.add(_norm(path))
    return added


def collect_added_files(
    *,
    project_root: Path,
    changed_files: List[str],
    diff_content: str,
    get_untracked_files: Callable[[], List[str]],
    classify_fn: Callable[[List[str]], tuple[List[str], List[str]]] | None = None,
) -> Set[str]:
    """Union of classify_change_paths added set and committed-diff ``/dev/null`` headers.

    Restricted to paths that appear in the change set or are currently untracked
    (new) so stray diff headers outside the task's change set are dropped.
    """
    if classify_fn is not None:
        classified, _ = classify_fn(changed_files)
        added = set(classified)
    else:
        classified, _ = classify_change_paths(project_root, changed_files, get_untracked_files)
        added = set(classified)
    added |= added_files_from_diff(diff_content)
    changed_norm = {_norm(p) for p in changed_files}
    try:
        untracked_norm = {_norm(p) for p in get_untracked_files()}
    except Exception:
        untracked_norm = set()
    return {
        _norm(p) for p in added
        if _norm(p) in changed_norm or _norm(p) in untracked_norm
    }


def detect_unwired_file_gaps(
    *,
    task: Task,
    project_root: Path,
    changed_files: List[str],
    diff_content: str,
    get_untracked_files: Callable[[], List[str]],
    next_gap_id: Callable[[str, str], str],
    unwired_enabled: bool = True,
    unwired_blocking: bool = False,
    classify_fn: Callable[[List[str]], tuple[List[str], List[str]]] | None = None,
    git_files: Optional[List[str]] = None,
) -> List[Gap]:
    """Flag added Python/JS/TS files with no external non-test importer."""
    gaps: List[Gap] = []
    if not unwired_enabled:
        return gaps
    try:
        added = collect_added_files(
            project_root=project_root,
            changed_files=changed_files,
            diff_content=diff_content,
            get_untracked_files=get_untracked_files,
            classify_fn=classify_fn,
        )
        candidates = sorted(p for p in added if is_liveness_code_file(p))
        if not candidates:
            return gaps

        from devcouncil.indexing.repo_mapper import RepoMapper

        mapper = RepoMapper(project_root)
        # Include untracked/new files in the edge graph so same-task imports resolve.
        if git_files is None:
            try:
                tracked = mapper.get_git_files()
            except Exception:
                tracked = []
        else:
            tracked = list(git_files)
        all_files = sorted(set(tracked) | added | {_norm(p) for p in changed_files})
        dependents = mapper.dependents_for(all_files)
        roots = set(entry_roots(project_root, all_files))
        added_set = set(candidates)
        dyn_index = build_dynamic_import_index(project_root, all_files)

        for path in candidates:
            if path in roots or structural_exemptions(path):
                continue

            if has_allow_unwired(project_root, path):
                # Marker always clears the blocking gap; still emit an advisory so
                # intentional unwired files stay visible in the repair loop.
                gaps.append(Gap(
                    id=next_gap_id(task.id, "UNWIREDDECL"),
                    severity="medium",
                    gap_type="unwired_file",
                    task_id=task.id,
                    description=(
                        f"Intentional unwired file declared at {path} "
                        f"({ALLOW_UNWIRED})."
                    ),
                    evidence=[path, ALLOW_UNWIRED],
                    recommended_fix=(
                        "Review declared unwired files before marking done; wire each "
                        "into its intended caller when scaffolding is complete."
                    ),
                    blocking=False,
                    file=path,
                ))
                continue

            importers = dependents.get(path, set())
            external_non_test = {
                i for i in importers
                if i not in added_set and not is_test_path(i)
            }
            if external_non_test:
                continue

            if reference_cleared(
                project_root,
                path,
                skip_files=added_set,
                git_files=all_files,
                dynamic_index=dyn_index,
            ):
                continue

            only_test = {i for i in importers if is_test_path(i) and i not in added_set}
            only_same_task = {i for i in importers if i in added_set}
            if only_test and not only_same_task:
                desc = (
                    f"New file {path} is only imported by test files "
                    f"({', '.join(sorted(only_test)[:3])}); production code must import it."
                )
            elif only_same_task and not only_test:
                desc = (
                    f"New file {path} is only imported by other files added in this task "
                    f"({', '.join(sorted(only_same_task)[:3])}); a pre-existing non-test "
                    "caller must import it (same-task island rule)."
                )
            elif importers:
                desc = (
                    f"New file {path} has no external non-test importer "
                    f"(importers: {', '.join(sorted(importers)[:5])})."
                )
            else:
                desc = f"New file {path} is never imported by any other module."

            gaps.append(Gap(
                id=next_gap_id(task.id, "UNWIRED"),
                severity="high" if unwired_blocking else "medium",
                gap_type="unwired_file",
                task_id=task.id,
                description=desc,
                evidence=[path],
                recommended_fix=(
                    f"Import or register `{path}` from its intended non-test caller "
                    f"(append the caller with `dev scope update {task.id} --lease-token "
                    f"<token> --planned-file <caller>` if needed), or delete the unused file."
                ),
                blocking=unwired_blocking,
                file=path,
            ))
    except Exception:
        logger.debug("detect_unwired_file_gaps failed; degrading to zero gaps", exc_info=True)
        return []
    return gaps
