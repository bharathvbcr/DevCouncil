"""Stub detection helpers extracted from Verifier."""

from __future__ import annotations

from pathlib import Path
from typing import Callable, List

from devcouncil.domain.gap import Gap
from devcouncil.domain.task import Task
from devcouncil.verification.stub_detector import (
    detect_stub_declarations,
    detect_stubs,
    task_allows_scaffolding,
)


def detect_stub_gaps(
    *,
    task: Task,
    diff_content: str,
    project_root: Path,
    stub_enabled: bool,
    stub_blocking: bool,
    next_gap_id: Callable[[str, str], str],
) -> List[Gap]:
    """Scan diff-added lines for stubs/TODOs and declared scaffolding markers."""
    gaps: List[Gap] = []
    if not stub_enabled or not diff_content:
        return gaps
    scaffolding_ok = task_allows_scaffolding(task)
    for finding in detect_stubs(project_root, diff_content, honor_allow_stub=scaffolding_ok):
        gaps.append(Gap(
            id=next_gap_id(task.id, f"STUB-{finding.file}:{finding.line}"),
            severity="high" if stub_blocking else "medium",
            gap_type="stub_detected",
            task_id=task.id,
            description=(
                f"Placeholder/incomplete code added at {finding.file}:{finding.line}: "
                f"{finding.reason}."
            ),
            evidence=[f"{finding.file}:{finding.line}", finding.snippet],
            recommended_fix=(
                f"Replace the stub/placeholder at {finding.file}:{finding.line} with a "
                "real implementation, then re-verify. Intentional scaffolding requires "
                "the task description to mention 'scaffolding' and the line to carry "
                "'devcouncil: allow-stub'."
            ),
            blocking=stub_blocking,
            file=finding.file,
            line=finding.line,
        ))
    for decl in detect_stub_declarations(diff_content):
        gaps.append(Gap(
            id=next_gap_id(task.id, f"STUBDECL-{decl.file}:{decl.line}"),
            severity="medium",
            gap_type="stub_declared",
            task_id=task.id,
            description=(
                f"Intentional stub declared at {decl.file}:{decl.line} "
                f"({decl.snippet[:80]})."
            ),
            evidence=[f"{decl.file}:{decl.line}", decl.snippet],
            recommended_fix=(
                "Review declared stubs before marking the task done; replace each "
                "placeholder with a real implementation when scaffolding is complete."
            ),
            blocking=False,
            file=decl.file,
            line=decl.line,
        ))
    return gaps
