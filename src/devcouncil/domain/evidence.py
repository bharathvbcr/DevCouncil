from pydantic import BaseModel, Field
from typing import Literal, List, Dict

class CommandResult(BaseModel):
    command: str
    exit_code: int
    stdout_path: str
    stderr_path: str
    summary: str
    timed_out: bool = False

class DiffEvidence(BaseModel):
    task_id: str
    changed_files: List[str]
    added_files: List[str]
    deleted_files: List[str]
    diff_summary: str

class DiffCoverageEvidence(BaseModel):
    """Proof that the changed lines were (or were not) exercised by the tests.

    This is the executable evidence behind DevCouncil's core promise: a passing
    suite is only acceptance evidence if the lines the diff changed were actually
    run. ``measured`` is False when no reliable signal could be computed (no
    coverage tool, no instrumentable test command, or no changed executable
    lines), in which case it must never be read as a defect.
    """

    task_id: str
    tool: str = ""
    measured: bool = False
    changed_lines: int = 0
    covered_lines: int = 0
    coverage_ratio: float = 0.0
    uncovered_by_file: Dict[str, List[int]] = Field(default_factory=dict)
    absent_files: List[str] = Field(default_factory=list)
    summary: str = ""

class VerificationEvidence(BaseModel):
    __test__ = False  # Prevent pytest from collecting this as a test class
    requirement_id: str
    acceptance_criterion_id: str
    command: str
    status: Literal["passed", "failed", "not_run"]
    evidence_summary: str
    # HOW the criterion was proven, for auditing the gate's rigor (distinct from the
    # pass/fail status). ``compiled`` = one DevCouncil per-criterion check passed;
    # ``vote`` = a majority of independent checks passed (self-consistency);
    # ``coarse`` = proven only by a passing acceptance-capable command, not a check tied
    # to the criterion (weakest). Empty for legacy/unspecified evidence. Persisted in the
    # evidence JSON blob, so adding it needs no migration; old rows default to "".
    mode: Literal["compiled", "vote", "coarse", ""] = ""

# Backward-compatible alias
TestEvidence = VerificationEvidence
