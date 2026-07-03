"""Effort/diff plausibility heuristics — is the diff big enough to be the work?

Three deliberately conservative checks, each aimed at a known lazy-agent pattern:

- **undersized diff**: the task plans substantial work (several writable files, or
  file creation) but the diff adds only a handful of code lines.
- **comment-only diff**: files changed, but no added line is actual code, while
  the task has automatable acceptance criteria to prove.
- **test deletion**: more test lines removed than added — the "make the suite
  pass by deleting the test" move. Always high severity.

These are heuristics, so outside hard tasks they surface as advisory
``suspicious_effort`` gaps; the caller decides blocking via the rigor policy.
Never raises.
"""

from __future__ import annotations

import logging
import re
from dataclasses import dataclass
from pathlib import Path
from typing import List, Optional

from devcouncil.domain.requirement import Requirement
from devcouncil.domain.task import Task
from devcouncil.verification.stub_detector import added_lines_by_file

logger = logging.getLogger(__name__)

_AUTOMATABLE_METHODS = {"unit_test", "integration_test", "static_check"}

_COMMENT_PREFIXES = ("#", "//", "/*", "*", "--", ";", "<!--")


@dataclass
class EffortFinding:
    reason: str
    detail: str
    severity: str = "medium"
    file: Optional[str] = None


def _is_code_line(text: str) -> bool:
    stripped = text.strip()
    if not stripped:
        return False
    return not stripped.startswith(_COMMENT_PREFIXES)


def _is_test_path(path: str) -> bool:
    lowered = path.lower()
    name = lowered.rsplit("/", 1)[-1]
    return (
        "/tests/" in f"/{lowered}"
        or lowered.startswith("tests/")
        or name.startswith("test_")
        or re.search(r"(_test|\.test|\.spec)\.[a-z]+$", name) is not None
    )


def _removed_lines_by_file(diff_content: str) -> dict:
    """``{old_path: removed_line_count}`` from a unified diff. Tolerant, never raises."""
    out: dict = {}
    current: Optional[str] = None
    for raw in diff_content.splitlines():
        if raw.startswith("--- "):
            target = raw[4:].strip()
            if target == "/dev/null":
                current = None
            else:
                current = target[2:] if target.startswith(("a/", "b/")) else target
            continue
        if current is None:
            continue
        if raw.startswith("-") and not raw.startswith("---"):
            out[current] = out.get(current, 0) + 1
    return out


def _automatable_criteria_present(task: Task, requirements: Optional[List[Requirement]]) -> bool:
    if not task.acceptance_criterion_ids:
        return False
    if not requirements:
        return True  # criteria exist; assume provable absent contrary evidence
    wanted = set(task.acceptance_criterion_ids)
    for req in requirements:
        for ac in req.acceptance_criteria:
            if ac.id in wanted and ac.verification_method in _AUTOMATABLE_METHODS:
                return True
    return False


def detect_effort_anomalies(
    task: Task,
    diff_content: str,
    requirements: Optional[List[Requirement]] = None,
    *,
    min_added_lines_per_planned_file: int = 5,
) -> List[EffortFinding]:
    """Run all effort heuristics over the task's diff. Never raises."""
    try:
        added = added_lines_by_file(diff_content)
    except Exception:  # pragma: no cover - defensive
        logger.debug("effort heuristics: diff parse failed", exc_info=True)
        return []
    if not added:
        return []  # empty diff is the verifier's task_not_implemented gate, not ours

    findings: List[EffortFinding] = []
    writable = [pf for pf in task.planned_files if pf.allowed_change != "read_only"]
    added_code_lines = sum(
        1 for lines in added.values() for _, text in lines if _is_code_line(text)
    )
    # Declared-stub lines (devcouncil: allow-stub) do not count toward the
    # undersized-diff threshold — scaffolding must not absorb the whole diff.
    declared_stub_lines = sum(
        1 for lines in added.values() for _, text in lines if "devcouncil: allow-stub" in text
    )
    effective_code_lines = max(0, added_code_lines - declared_stub_lines)

    # 1. Undersized diff vs declared scope.
    substantial_scope = len(writable) >= 3 or any(pf.allowed_change == "create" for pf in writable)
    if substantial_scope and writable:
        threshold = min_added_lines_per_planned_file * len(writable)
        if effective_code_lines < threshold:
            findings.append(EffortFinding(
                reason="undersized_diff",
                detail=(
                    f"Task plans {len(writable)} writable file(s) but the diff adds only "
                    f"{effective_code_lines} substantive code line(s) (< {threshold} expected; "
                    f"{declared_stub_lines} declared-stub line(s) excluded). The "
                    "implementation may be superficial or incomplete."
                ),
            ))

    # 2. Comment/whitespace-only diff while automatable criteria exist.
    if added_code_lines == 0 and _automatable_criteria_present(task, requirements):
        findings.append(EffortFinding(
            reason="comment_only_diff",
            detail=(
                "The diff adds no executable code (comments/blank lines only), but the "
                "task has acceptance criteria that require behavioral changes."
            ),
        ))

    # 3. Test deletion: more test lines removed than added in files referenced by
    # expected_tests — the classic "make the suite pass by deleting the test".
    removed = _removed_lines_by_file(diff_content)
    expected = " ".join(task.expected_tests or [])
    referenced_test_paths = {
        path for path in removed if _is_test_path(path) and path in expected
    }
    if not referenced_test_paths and expected:
        # Fall back: basename match when commands use paths like tests/test_a.py.
        for path in removed:
            if _is_test_path(path) and Path(path).name in expected:
                referenced_test_paths.add(path)
    test_removed = sum(removed[p] for p in referenced_test_paths)
    test_added = sum(
        len(lines) for path, lines in added.items()
        if _is_test_path(path) and path in referenced_test_paths
    )
    if test_removed > 0 and test_removed > test_added:
        worst = max(referenced_test_paths, key=lambda p: removed[p])
        findings.append(EffortFinding(
            reason="test_deletion",
            detail=(
                f"The diff removes {test_removed} line(s) from test file(s) referenced by "
                f"expected_tests but adds only {test_added}. Weakening or deleting tests to "
                "make verification pass is never acceptable; restore the tests or justify "
                "the removal explicitly."
            ),
            severity="high",
            file=worst,
        ))

    return findings
