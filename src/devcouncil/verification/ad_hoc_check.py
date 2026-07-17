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
import logging
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

logger = logging.getLogger(__name__)


@dataclass
class AdHocCheckResult:
    requirement: str
    changed_files: List[str] = field(default_factory=list)
    gaps: List[Gap] = field(default_factory=list)
    next_actions: List[NextAction] = field(default_factory=list)
    diff_coverage: Optional[DiffCoverageEvidence] = None
    passed: bool = True
    reason: str = ""
    verification_mode: str = "coarse"

    def to_dict(self) -> dict:
        return {
            "ok": True,
            "verified": self.passed,
            "requirement": self.requirement,
            "changed_files": self.changed_files,
            "reason": self.reason,
            "verification_mode": self.verification_mode,
            "gap_count": len(self.gaps),
            "blocking_gap_count": len([g for g in self.gaps if g.blocking]),
            "gaps": [g.model_dump() for g in self.gaps],
            "next_actions": [a.model_dump() for a in self.next_actions],
            "diff_coverage": self.diff_coverage.model_dump() if self.diff_coverage else None,
        }


def _load_verify_router(project_root: Path) -> Optional[ModelRouter]:
    """Best-effort router for compiled acceptance checks during ad-hoc verify."""
    try:
        from devcouncil.app.config import get_api_key, load_config
        from devcouncil.llm.provider import create_provider, validate_model_provider

        config = load_config(project_root)
        validate_model_provider(config.models.provider)
        api_key = get_api_key(config.models.provider, project_root)
        provider = create_provider(
            config.models.provider,
            api_key,
            project_root=project_root,
            provider_prefs=config.provider,
        )
        role_config = {name: role.model_dump() for name, role in config.models.roles.items()}
        return ModelRouter(provider, role_config, project_root=project_root)
    except Exception as exc:
        logger.debug("Ad-hoc check: no model router (%s)", exc)
        return None


def _persist_ad_hoc_result(
    project_root: Path,
    *,
    requirement: Requirement,
    task: Task,
    gaps: List[Gap],
    evidence: list,
) -> None:
    """Write CHECK task gaps/evidence so ``dev report`` can export CI artifacts."""
    from devcouncil.domain.evidence import CommandResult, DiffEvidence, TestEvidence
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import (
        EvidenceRepository,
        GapRepository,
        RequirementRepository,
        TaskRepository,
    )

    db = get_db(project_root)
    if not db:
        return
    with db.get_session() as session:
        RequirementRepository(session).save(requirement)
        gap_repo = GapRepository(session)
        ev_repo = EvidenceRepository(session)
        gap_repo.delete_for_task(task.id)
        ev_repo.delete_for_task(task.id)
        for gap in gaps:
            gap_repo.save(gap)
        for ev in evidence:
            if isinstance(ev, CommandResult):
                ev_repo.save_command_result(task.id, ev)
            elif isinstance(ev, DiffCoverageEvidence):
                ev_repo.save_diff_coverage_evidence(ev)
            elif isinstance(ev, DiffEvidence):
                ev_repo.save_diff_evidence(ev)
            elif isinstance(ev, TestEvidence):
                ev_repo.save_test_evidence(ev, task.id)
        task.status = "blocked" if any(g.blocking for g in gaps) else "verified"
        TaskRepository(session).save(task)


def run_working_tree_check(
    project_root: Path,
    requirement: Optional[str] = None,
    *,
    base: Optional[str] = None,
    test_commands: Optional[List[str]] = None,
    enforce_coverage: bool = False,
    min_ratio: float = 0.0,
    persist: bool = False,
    router: Optional[ModelRouter] = None,
    verifier: Optional[Verifier] = None,
) -> AdHocCheckResult:
    """Verify the current working-tree diff against a one-line requirement.

    Builds a synthetic task whose planned files are exactly the changed files (so the
    result is about evidence, not scope noise) and whose expected tests are
    ``test_commands``. Diff coverage is always measured; pass ``enforce_coverage`` (or a
    positive ``min_ratio``) to make an unexercised diff blocking.

    When ``base`` is set, the diff is ``git diff <base>`` (PR/base scope) instead of
    uncommitted changes vs HEAD. Pass ``persist=True`` to write gaps/evidence for
    ``dev report`` artifact export (typical in CI).
    """
    verifier = verifier or Verifier(project_root, router=router or _load_verify_router(project_root))
    if base:
        verifier._git_fallback.diff_base = base

    diff = verifier.get_diff()
    changed_files = verifier.get_changed_files()
    if not diff or not changed_files:
        scope = f"against {base}" if base else "in working tree"
        logger.info("Ad-hoc check: no changes %s; passing trivially", scope)
        return AdHocCheckResult(
            requirement=requirement or "",
            passed=True,
            reason="no_changes",
        )
    logger.info("Ad-hoc check: %d changed file(s), enforce_coverage=%s", len(changed_files), enforce_coverage or min_ratio > 0)

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
        difficulty="easy",
    )

    # Always measure diff coverage in lite mode; block on it only when asked. A positive
    # --min-coverage implies enforcement so the flag is never silently inert.
    enforce = enforce_coverage or min_ratio > 0
    verifier._diff_coverage_override = (True, enforce, float(min_ratio))

    gaps, evidence = asyncio.run(verifier.verify_task(task, [req]))
    coverage = next((ev for ev in evidence if isinstance(ev, DiffCoverageEvidence)), None)
    blocking = [g for g in gaps if g.blocking]
    outcome_mode = verifier.last_outcome.mode if verifier.last_outcome else (
        "compiled" if verifier.acceptance_compiler else "coarse"
    )
    if persist:
        _persist_ad_hoc_result(
            project_root,
            requirement=req,
            task=task,
            gaps=gaps,
            evidence=evidence,
        )
    logger.info("Ad-hoc check result: passed=%s (%d gap(s), %d blocking)", not blocking, len(gaps), len(blocking))
    return AdHocCheckResult(
        requirement=criterion,
        changed_files=changed_files,
        gaps=gaps,
        next_actions=build_next_actions(gaps),
        diff_coverage=coverage,
        passed=not blocking,
        verification_mode=outcome_mode,
    )
