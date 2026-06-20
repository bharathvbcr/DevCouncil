"""Verify an ad-hoc working-tree diff against an inline requirement — no planning, no keys.

This powers ``dev check``'s evidence-gate mode (the lite entry point): wrap whatever is
in the working tree as a synthetic Requirement→Task, run the *same* deterministic
:class:`~devcouncil.verification.verifier.Verifier` the full workflow uses — orphan-diff,
secret scan, acceptance evidence, and the diff↔coverage gate — and return the verdict
plus the typed next-actions contract. ``router=None`` keeps it provider-key-free so a
newcomer can taste the evidence gate before committing to the full council flow.

The logic lives here (not in the CLI command) so it is unit-testable without Typer and
resilient to churn in the command module.
"""

from __future__ import annotations

import asyncio
from dataclasses import dataclass, field
from pathlib import Path
from typing import List, Optional

from devcouncil.domain.evidence import DiffCoverageEvidence
from devcouncil.domain.gap import Gap
from devcouncil.domain.requirement import AcceptanceCriterion, Requirement
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.llm.router import ModelRouter
from devcouncil.verification.next_actions import NextAction, build_next_actions
from devcouncil.verification.verifier import Verifier

_REQ_ID = "REQ-CHECK"
_AC_ID = "AC-CHECK"
_TASK_ID = "CHECK"

_DEFAULT_CRITERION = "The working-tree changes are correct and exercised by tests."


@dataclass
class AdHocCheckResult:
    requirement: str
    changed_files: List[str] = field(default_factory=list)
    gaps: List[Gap] = field(default_factory=list)
    next_actions: List[NextAction] = field(default_factory=list)
    diff_coverage: Optional[DiffCoverageEvidence] = None
    passed: bool = True
    reason: str = ""

    def to_dict(self) -> dict:
        return {
            "ok": True,
            "verified": self.passed,
            "requirement": self.requirement,
            "changed_files": self.changed_files,
            "reason": self.reason,
            "gap_count": len(self.gaps),
            "blocking_gap_count": len([g for g in self.gaps if g.blocking]),
            "gaps": [g.model_dump() for g in self.gaps],
            "next_actions": [a.model_dump() for a in self.next_actions],
            "diff_coverage": self.diff_coverage.model_dump() if self.diff_coverage else None,
        }


def run_working_tree_check(
    project_root: Path,
    requirement: Optional[str] = None,
    *,
    test_commands: Optional[List[str]] = None,
    enforce_coverage: bool = False,
    min_ratio: float = 0.0,
    router: Optional[ModelRouter] = None,
    verifier: Optional[Verifier] = None,
) -> AdHocCheckResult:
    """Verify the current working-tree diff against a one-line requirement.

    Builds a synthetic task whose planned files are exactly the changed files (so the
    result is about evidence, not scope noise) and whose expected tests are
    ``test_commands``. Diff coverage is always measured; pass ``enforce_coverage`` (or a
    positive ``min_ratio``) to make an unexercised diff blocking.
    """
    verifier = verifier or Verifier(project_root, router=router)

    diff = verifier.get_diff()
    changed_files = verifier.get_changed_files()
    if not diff or not changed_files:
        return AdHocCheckResult(requirement="", passed=True, reason="no_changes")

    criterion = requirement or _DEFAULT_CRITERION
    req = Requirement(
        id=_REQ_ID,
        title=(requirement or "Working-tree change")[:80],
        description=criterion,
        priority="high",
        source="user",
        acceptance_criteria=[
            AcceptanceCriterion(id=_AC_ID, description=criterion, verification_method="unit_test"),
        ],
    )
    untracked = set(verifier._get_untracked_files())
    task = Task(
        id=_TASK_ID,
        title="Ad-hoc working-tree check",
        description=criterion,
        requirement_ids=[_REQ_ID],
        acceptance_criterion_ids=[_AC_ID],
        planned_files=[
            PlannedFile(
                path=path,
                reason="working-tree change",
                allowed_change="create" if path in untracked else "modify",
            )
            for path in changed_files
        ],
        expected_tests=list(test_commands or []),
    )

    # Always measure diff coverage in lite mode; block on it only when asked. A positive
    # --min-coverage implies enforcement so the flag is never silently inert.
    enforce = enforce_coverage or min_ratio > 0
    verifier._diff_coverage_override = (True, enforce, float(min_ratio))

    gaps, evidence = asyncio.run(verifier.verify_task(task, [req]))
    coverage = next((ev for ev in evidence if isinstance(ev, DiffCoverageEvidence)), None)
    blocking = [g for g in gaps if g.blocking]
    return AdHocCheckResult(
        requirement=criterion,
        changed_files=changed_files,
        gaps=gaps,
        next_actions=build_next_actions(gaps),
        diff_coverage=coverage,
        passed=not blocking,
    )
