from pydantic import BaseModel, Field
from typing import Literal, List

class AcceptanceCriterion(BaseModel):
    id: str
    description: str
    verification_method: Literal[
        "unit_test",
        "integration_test",
        "manual",
        "static_check",
        "llm_review"
    ]
    required: bool = True

class Requirement(BaseModel):
    id: str
    title: str
    description: str
    priority: Literal["low", "medium", "high", "critical"]
    source: Literal["user", "planner", "critic", "arbiter"]
    acceptance_criteria: List[AcceptanceCriterion] = Field(default_factory=list)
