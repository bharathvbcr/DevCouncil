from sqlmodel import SQLModel, Field
from typing import Optional


class SchemaVersionModel(SQLModel, table=True):
    __tablename__ = "schema_version"
    id: str = Field(primary_key=True, default="singleton")
    version: int

class RequirementModel(SQLModel, table=True):
    __tablename__ = "requirements"
    id: str = Field(primary_key=True)
    title: str
    description: str
    priority: str
    source: str
    # Store complex lists as JSON strings for simplicity in SQLite
    acceptance_criteria_json: str = Field(default="[]")

class AssumptionModel(SQLModel, table=True):
    __tablename__ = "assumptions"
    id: str = Field(primary_key=True)
    statement: str
    confidence: str
    impact: str
    reversible: bool
    requires_user_confirmation: bool
    linked_requirement_ids_json: str = Field(default="[]")
    status: str = "open"

class TaskModel(SQLModel, table=True):
    __tablename__ = "tasks"
    id: str = Field(primary_key=True)
    title: str
    description: str
    requirement_ids_json: str = Field(default="[]")
    acceptance_criterion_ids_json: str = Field(default="[]")
    planned_files_json: str = Field(default="[]")
    expected_tests_json: str = Field(default="[]")
    allowed_commands_json: str = Field(default="[]")
    forbidden_changes_json: str = Field(default="[]")
    status: str = "planned"

class EvidenceModel(SQLModel, table=True):
    __tablename__ = "evidence"
    id: Optional[int] = Field(default=None, primary_key=True)
    type: str  # "command", "diff", "test"
    task_id: Optional[str] = None
    requirement_id: Optional[str] = None
    acceptance_criterion_id: Optional[str] = None
    data_json: str

class GapModel(SQLModel, table=True):
    __tablename__ = "gaps"
    id: str = Field(primary_key=True)
    severity: str
    gap_type: str
    requirement_id: Optional[str] = None
    task_id: Optional[str] = None
    description: str
    evidence_json: str = Field(default="[]")
    recommended_fix: str
    blocking: bool

class CritiqueFindingModel(SQLModel, table=True):
    __tablename__ = "critique_findings"
    id: str = Field(primary_key=True)
    source_agent: str
    target_plan_id: str
    severity: str
    finding_type: str
    claim: str
    linked_requirement_id: Optional[str] = None
    suggested_requirement: Optional[str] = None
    suggested_task: Optional[str] = None
    falsifiable_check: str
    status: str = "open"

class ProjectStateModel(SQLModel, table=True):
    __tablename__ = "project_state"
    id: str = Field(primary_key=True, default="singleton")
    current_phase: str
    history_json: str = Field(default="[]")
