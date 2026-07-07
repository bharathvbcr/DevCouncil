from sqlmodel import SQLModel, Field
from sqlalchemy import Index, text
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
    agent_appended_expected_tests_json: str = Field(default="[]")
    allowed_commands_json: str = Field(default="[]")
    agent_appended_allowed_commands_json: str = Field(default="[]")
    forbidden_changes_json: str = Field(default="[]")
    # Manual/planner difficulty override ("easy"/"normal"/"hard"); None = estimator.
    difficulty: Optional[str] = Field(default=None)
    priority: Optional[str] = Field(default=None)
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
    # Machine-routable repair hints. Persisted (v4) so the typed next-actions
    # contract survives a reload — a reconnecting/crashed agent can read outstanding
    # gaps without the heuristic reconstruction the loader used to fall back to.
    file: Optional[str] = None
    line: Optional[int] = None
    suggested_command: Optional[str] = None
    acceptance_criterion_id: Optional[str] = None
    # Verification method the criterion expects (unit_test/manual/llm_review/...). Persisted
    # so the repair loop can tell an executor-remediable "incomplete" (an automatable check
    # that did not run) from one a human must close (manual/llm_review) after a reload.
    expected_verification_method: Optional[str] = None

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


class TaskLeaseModel(SQLModel, table=True):
    __tablename__ = "task_leases"
    # Enforce the single-writer guarantee at the DB level: at most one ACTIVE lease per
    # task. A partial unique index turns a concurrent double-checkout race into an
    # IntegrityError (surfaced as lease_conflict) instead of two live leases.
    __table_args__ = (
        Index(
            "ux_task_leases_active",
            "task_id",
            unique=True,
            sqlite_where=text("status = 'active'"),
        ),
    )
    id: str = Field(primary_key=True)
    task_id: str = Field(index=True)
    owner: str
    agent: Optional[str] = None
    client_id: Optional[str] = None
    run_id: Optional[str] = None
    branch: Optional[str] = None
    lease_token: str
    status: str = Field(default="active", index=True)
    created_at: str
    expires_at: Optional[str] = None
    released_at: Optional[str] = None


class ShellSessionModel(SQLModel, table=True):
    __tablename__ = "shell_sessions"
    id: str = Field(primary_key=True)
    task_id: str = Field(index=True)
    lease_id: Optional[str] = None
    shell: str
    cwd: str
    status: str
    started_at: str
    ended_at: Optional[str] = None


class ShellCommandEventModel(SQLModel, table=True):
    __tablename__ = "shell_command_events"
    id: Optional[int] = Field(default=None, primary_key=True)
    task_id: str = Field(index=True)
    session_id: Optional[str] = None
    command: str
    status: str
    exit_code: Optional[int] = None
    reason: str = ""
    stdout_path: str = ""
    stderr_path: str = ""
    created_at: str


class FileChangeEventModel(SQLModel, table=True):
    __tablename__ = "file_change_events"
    id: Optional[int] = Field(default=None, primary_key=True)
    task_id: Optional[str] = Field(default=None, index=True)
    lease_id: Optional[str] = None
    session_id: Optional[str] = None
    path: str
    operation: str
    allowed: bool
    reason: str = ""
    created_at: str


class SemanticDiffModel(SQLModel, table=True):
    __tablename__ = "semantic_diffs"
    id: Optional[int] = Field(default=None, primary_key=True)
    task_id: str = Field(index=True)
    before_snapshot_path: str
    after_snapshot_path: str
    classifications_json: str = "[]"
    summary: str = ""
    created_at: str


class AgentHandoffModel(SQLModel, table=True):
    __tablename__ = "agent_handoffs"
    id: str = Field(primary_key=True)
    task_id: str = Field(index=True)
    from_agent: str
    to_agent: str
    run_id: str
    manifest_path: str
    status: str
    created_at: str


class CorrectionManifestModel(SQLModel, table=True):
    __tablename__ = "correction_manifests"
    id: str = Field(primary_key=True)
    task_id: str = Field(index=True)
    run_id: Optional[str] = None
    manifest_path: str
    retry_budget: int = 3
    attempt: int = 0
    status: str
    created_at: str


class VerificationRunModel(SQLModel, table=True):
    __tablename__ = "verification_runs"
    id: str = Field(primary_key=True)
    task_id: str = Field(index=True)
    sandbox: str
    environment_json: str = "{}"
    commands_json: str = "[]"
    status: str
    started_at: str
    finished_at: Optional[str] = None
