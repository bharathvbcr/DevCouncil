from pydantic import BaseModel, Field
from typing import Literal, List, Optional

class Gap(BaseModel):
    id: str
    severity: Literal["low", "medium", "high", "critical"]
    gap_type: Literal[
        "requirement_not_planned",
        "task_not_implemented",
        "planned_file_not_changed",
        "orphan_diff",
        "missing_test",
        "test_failed",
        "invalid_verification_command",
        "acceptance_criteria_unproven",
        "diff_not_exercised",
        "assumption_violated",
        "architecture_drift",
        "security_risk",
        "dependency_risk",
        "migration_gap",
        "quality_gate_failed",
        "skipped_verification_command",
        "coarse_acceptance_proof",
        "stub_detected",
        "stub_declared",
        "suspicious_effort",
        "unwired_file",
        "dead_symbol",
        "stranded_code",
        "stale_map",
        "corpus_stale",
        "doc_code_ref",
        "acceptance_corpus",
    ]
    requirement_id: Optional[str] = None
    task_id: Optional[str] = None
    description: str
    evidence: List[str] = Field(default_factory=list)
    recommended_fix: str
    blocking: bool
    # Machine-actionable hints for the agent self-repair loop. Populated at gap
    # creation where known; consumed by the typed next-actions contract (see
    # devcouncil.verification.next_actions). As of schema v4 these are persisted by
    # the gap store and round-tripped on reload, so a reconnecting agent gets the
    # full repair contract rather than a heuristic reconstruction.
    file: Optional[str] = None
    line: Optional[int] = None
    suggested_command: Optional[str] = None
    # The acceptance criterion this gap is about (when applicable), so the agent can
    # tie a failure straight back to the criterion it must satisfy.
    acceptance_criterion_id: Optional[str] = None
    # Paths to the captured stdout/stderr logs (written under .devcouncil/logs) for the
    # failing command behind this gap, so the agent can open the FULL failure output
    # without re-running. Optional and defaulted for backward compatibility; the gap
    # store does not persist these, so they are only present on a fresh verify run.
    stdout_path: Optional[str] = None
    stderr_path: Optional[str] = None
    # Expected verification method (e.g. "unit_test"/"static_check") for an unproven
    # acceptance criterion, so missing-evidence routing is concrete rather than a
    # restatement of the description.
    expected_verification_method: Optional[str] = None
