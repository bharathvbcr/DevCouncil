from pydantic import BaseModel
from typing import Literal, List

class CommandResult(BaseModel):
    command: str
    exit_code: int
    stdout_path: str
    stderr_path: str
    summary: str

class DiffEvidence(BaseModel):
    task_id: str
    changed_files: List[str]
    added_files: List[str]
    deleted_files: List[str]
    diff_summary: str

class VerificationEvidence(BaseModel):
    __test__ = False  # Prevent pytest from collecting this as a test class
    requirement_id: str
    acceptance_criterion_id: str
    command: str
    status: Literal["passed", "failed", "not_run"]
    evidence_summary: str

# Backward-compatible alias
TestEvidence = VerificationEvidence
