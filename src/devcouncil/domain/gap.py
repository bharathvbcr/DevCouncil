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
        "acceptance_criteria_unproven",
        "assumption_violated",
        "architecture_drift",
        "security_risk",
        "dependency_risk",
        "migration_gap"
    ]
    requirement_id: Optional[str] = None
    task_id: Optional[str] = None
    description: str
    evidence: List[str] = Field(default_factory=list)
    recommended_fix: str
    blocking: bool
