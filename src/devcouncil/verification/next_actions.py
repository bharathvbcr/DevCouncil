"""Typed, machine-actionable next actions derived from verification gaps.

A human-readable "here is what's wrong" report is fine for a person, but an agent
in a closed loop needs a structured contract it can route on without parsing
prose. ``build_next_actions`` turns the gaps from a verification run into a list of
:class:`NextAction` records — ``{category, action, file, line, missing_evidence,
suggested_command}`` — so a coding agent (over MCP) can self-repair and re-verify
without a human pasting anything.

The mapping is deterministic. Where a gap was created with explicit hints
(``gap.file``/``gap.line``/``gap.suggested_command``) those are used directly; when
a gap is reloaded from the database (which does not persist those hint columns) the
fields are reconstructed best-effort from the gap's evidence and description.
"""

from __future__ import annotations

from typing import List, Optional

from pydantic import BaseModel, Field

from devcouncil.domain.gap import Gap

# Machine-routable buckets an agent can branch on. Kept small and stable so the
# contract is predictable across releases.
Category = str

_CATEGORY_BY_GAP_TYPE = {
    "orphan_diff": "scope",
    "planned_file_not_changed": "scope",
    "dependency_risk": "scope",
    "test_failed": "fix_code",
    "invalid_verification_command": "fix_verification",
    "acceptance_criteria_unproven": "add_test",
    "diff_not_exercised": "add_test",
    "missing_test": "add_test",
    "security_risk": "security",
    "architecture_drift": "review",
    "assumption_violated": "review",
    "migration_gap": "fix_code",
    "requirement_not_planned": "plan",
    "task_not_implemented": "plan",
    "stub_detected": "fix_code",
    "stub_declared": "review",
    "suspicious_effort": "review",
}


class NextAction(BaseModel):
    """One concrete, routable step the agent can take to clear a gap."""

    gap_id: str
    gap_type: str
    category: Category
    severity: str
    blocking: bool
    action: str
    file: Optional[str] = None
    line: Optional[int] = None
    acceptance_criterion_id: Optional[str] = None
    expected_verification_method: Optional[str] = None
    missing_evidence: Optional[str] = None
    suggested_command: Optional[str] = None
    evidence: List[str] = Field(default_factory=list)
    # Paths to the captured stdout/stderr logs for the failing command behind this
    # gap, so the agent can open the full failure output without re-running. Present
    # only on a fresh verify run (the gap store does not persist them).
    stdout_path: Optional[str] = None
    stderr_path: Optional[str] = None


def _looks_like_path(value: str) -> bool:
    value = value.strip()
    if not value or " " in value or "\n" in value:
        return False
    return "/" in value or "." in value


def _derive_file(gap: Gap) -> Optional[str]:
    if gap.file:
        return gap.file
    for item in gap.evidence:
        if isinstance(item, str) and _looks_like_path(item):
            return item
    return None


def _action_text(gap: Gap, file: Optional[str]) -> str:
    target = file or "the affected file"
    if gap.gap_type == "orphan_diff":
        return f"Revert changes to {target} or add it to the task's planned files."
    if gap.gap_type == "planned_file_not_changed":
        return f"Modify {target} as planned, or remove it from the task's planned files."
    if gap.gap_type == "dependency_risk":
        return f"Justify or revert the unplanned dependency/config change in {target}."
    if gap.gap_type == "test_failed":
        cmd = gap.suggested_command
        return f"Fix the failing check, then re-run: {cmd}" if cmd else "Fix the failing verification check, then re-verify."
    if gap.gap_type == "invalid_verification_command":
        return "Replace the unrunnable verification command with a single runnable command, then re-verify."
    if gap.gap_type == "diff_not_exercised":
        loc = f" ({target}{':' + str(gap.line) if gap.line else ''})" if file else ""
        return f"Add or extend a test that executes the changed lines{loc}, then re-verify."
    if gap.gap_type in {"acceptance_criteria_unproven", "missing_test"}:
        return "Provide a passing verification command that proves this acceptance criterion."
    if gap.gap_type == "security_risk":
        return "Remove the detected secret/finding from the diff and rotate any exposed credential."
    if gap.gap_type == "architecture_drift":
        return "Address the flagged change or resolve the open critique card, then re-verify."
    if gap.gap_type == "stub_detected":
        loc = f"{target}{':' + str(gap.line) if gap.line else ''}"
        return (
            f"Replace the stub/placeholder at {loc} with a real implementation, then re-verify. "
            "Do not mark work complete while placeholders remain."
        )
    if gap.gap_type == "stub_declared":
        loc = f"{target}{':' + str(gap.line) if gap.line else ''}"
        return (
            f"Review the intentional stub declared at {loc}; replace it before marking done."
        )
    if gap.gap_type == "suspicious_effort":
        return (
            "The diff looks too small or superficial for the planned scope. Complete the "
            "planned work (or restore removed tests), then re-verify."
        )
    # Fall back to the gap's own recommended fix for anything unmapped.
    return gap.recommended_fix


def _missing_evidence(gap: Gap) -> Optional[str]:
    """A concrete description of WHAT evidence is missing — not a restatement of the
    description.

    For an unproven acceptance criterion we name the criterion and the expected
    verification method (and, where the verifier knew one, the expected check command
    via ``gap.suggested_command``) so the agent can author the right proof rather than
    re-reading prose. Falls back to the gap description for the other add-test gap
    types (diff_not_exercised, missing_test) which already carry concrete locations."""
    if gap.gap_type == "acceptance_criteria_unproven":
        parts: List[str] = []
        ac = gap.acceptance_criterion_id
        method = gap.expected_verification_method
        if ac:
            parts.append(f"No passing evidence for acceptance criterion {ac}")
        else:
            parts.append("No passing acceptance evidence")
        if method:
            parts.append(f"expected verification method: {method}")
        if gap.suggested_command:
            parts.append(f"run/repair check: {gap.suggested_command}")
        elif gap.file:
            loc = f"{gap.file}:{gap.line}" if gap.line else gap.file
            parts.append(f"uncovered: {loc}")
        return "; ".join(parts)
    if gap.gap_type in {"diff_not_exercised", "missing_test"}:
        return gap.description
    return None


def next_action_for(gap: Gap) -> NextAction:
    file = _derive_file(gap)
    return NextAction(
        gap_id=gap.id,
        gap_type=gap.gap_type,
        category=_CATEGORY_BY_GAP_TYPE.get(gap.gap_type, "review"),
        severity=gap.severity,
        blocking=gap.blocking,
        action=_action_text(gap, file),
        file=file,
        line=gap.line,
        acceptance_criterion_id=gap.acceptance_criterion_id,
        expected_verification_method=gap.expected_verification_method,
        missing_evidence=_missing_evidence(gap),
        suggested_command=gap.suggested_command,
        evidence=list(gap.evidence),
        stdout_path=gap.stdout_path,
        stderr_path=gap.stderr_path,
    )


def build_next_actions(gaps: List[Gap], *, blocking_only: bool = True) -> List[NextAction]:
    """Build the next-actions contract from verification gaps.

    By default only *blocking* gaps become next actions — those are what the agent
    must clear to pass verification. Pass ``blocking_only=False`` to include
    advisory signals (non-blocking gaps) as well. Blocking actions are ordered
    first, then by severity.
    """
    selected = [g for g in gaps if g.blocking] if blocking_only else list(gaps)
    severity_rank = {"critical": 0, "high": 1, "medium": 2, "low": 3}
    selected.sort(key=lambda g: (not g.blocking, severity_rank.get(g.severity, 4)))
    return [next_action_for(gap) for gap in selected]


def split_next_actions(gaps: List[Gap]) -> tuple[List[NextAction], List[NextAction]]:
    """Return ``(blocking_actions, advisory_actions)``.

    Blocking actions are what the agent MUST clear to pass the gate. Advisory
    actions are non-blocking signals worth acting on — most importantly the
    diff↔coverage ``diff_not_exercised`` finding ("tests passed but the new code
    was never run") and security/add-test hints, which ``build_next_actions``
    filters out by default. Surfacing them as a distinct array lets an autonomous
    agent improve quality without confusing them with the pass/fail gate.
    """
    all_actions = build_next_actions(gaps, blocking_only=False)
    blocking = [a for a in all_actions if a.blocking]
    advisory = [a for a in all_actions if not a.blocking]
    return blocking, advisory
