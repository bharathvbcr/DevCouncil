from pydantic import BaseModel
from typing import Literal, Optional

class CritiqueFinding(BaseModel):
    id: str
    source_agent: str
    target_plan_id: str
    severity: Literal["low", "medium", "high", "critical"]
    finding_type: Literal[
        "missing_requirement",
        "missing_task",
        "missing_test",
        "bad_assumption",
        "architecture_risk",
        "security_risk",
        "performance_risk",
        "dependency_risk",
        "migration_risk",
        "unverifiable_acceptance_criteria"
    ]
    claim: str
    linked_requirement_id: Optional[str] = None
    suggested_requirement: Optional[str] = None
    suggested_task: Optional[str] = None
    falsifiable_check: str
    status: Literal[
        "open",
        "accepted",
        "rejected",
        "converted",
        "needs_user"
    ] = "open"
