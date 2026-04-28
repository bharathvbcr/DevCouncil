from pydantic import BaseModel, Field
from typing import Literal, List

class Assumption(BaseModel):
    id: str
    statement: str
    confidence: Literal["low", "medium", "high"]
    impact: Literal["low", "medium", "high"]
    reversible: bool
    requires_user_confirmation: bool
    linked_requirement_ids: List[str] = Field(default_factory=list)
    status: Literal[
        "open",
        "confirmed",
        "rejected",
        "converted_to_requirement"
    ] = "open"
