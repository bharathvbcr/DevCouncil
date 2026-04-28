from typing import List
from pydantic import BaseModel, Field

# Common schemas that might be shared across artifacts
class FileModification(BaseModel):
    path: str
    diff: str

class CoverageMatrix(BaseModel):
    """Represents a requirement to task/test mapping coverage."""
    requirement_id: str
    task_ids: List[str] = Field(default_factory=list)
    test_evidence_ids: List[str] = Field(default_factory=list)
    is_covered: bool = False
    missing_tasks: bool = False
    missing_tests: bool = False

class ReportSchema(BaseModel):
    project_id: str
    tasks_completed: int
    tasks_blocked: int
    open_gaps: int
    coverage_matrix: List[CoverageMatrix]
