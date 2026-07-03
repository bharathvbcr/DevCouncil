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
    # Defaulted: requirements come back through LLM round-trips (arbiter/critic
    # rewrites) where weaker models sometimes drop provenance metadata; a missing
    # source shouldn't invalidate an otherwise-sound requirement. "planner" is the
    # most common origin for machine-generated requirements.
    source: Literal["user", "planner", "critic", "arbiter"] = "planner"
    acceptance_criteria: List[AcceptanceCriterion] = Field(default_factory=list)
