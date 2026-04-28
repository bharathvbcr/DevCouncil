from pydantic import BaseModel, Field
from typing import Literal, List

class PlannedFile(BaseModel):
    path: str
    reason: str
    allowed_change: Literal["create", "modify", "delete", "read_only"]

class Task(BaseModel):
    id: str
    title: str
    description: str
    requirement_ids: List[str] = Field(default_factory=list)
    acceptance_criterion_ids: List[str] = Field(default_factory=list)
    planned_files: List[PlannedFile] = Field(default_factory=list)
    expected_tests: List[str] = Field(default_factory=list)
    allowed_commands: List[str] = Field(default_factory=list)
    forbidden_changes: List[str] = Field(default_factory=list)
    status: Literal[
        "planned",
        "ready",
        "running",
        "blocked",
        "verified",
        "done"
    ] = "planned"
