from pydantic import BaseModel, Field
from typing import Literal, List, Optional

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
    expected_tests: List[str] = Field(
        default_factory=list,
        description=(
            "Runnable shell commands (not prose) that prove this task's acceptance "
            "criteria when they exit 0; they are executed verbatim by the verifier. "
            "They must run right after THIS task with no missing tools or test files. "
            "Prefer self-contained inline assertions, e.g. "
            "python -c \"import calc; assert calc.subtract(10,3)==7\". Use pytest only "
            "on a test file this or an earlier task creates, running the whole file "
            "(python -m pytest tests/test_x.py -q), never a ::name node. Do not use "
            "linters/type-checkers (flake8, mypy, ruff, eslint, tsc, npm) unless the "
            "repo is already configured for them."
        ),
    )
    agent_appended_expected_tests: List[str] = Field(
        default_factory=list,
        description=(
            "Subset of expected_tests appended at runtime by a leased agent via "
            "devcouncil_update_task_scope. These may run during verification but "
            "cannot coarse-prove acceptance criteria."
        ),
    )
    allowed_commands: List[str] = Field(
        default_factory=list,
        description=(
            "Shell commands the executor and verifier are permitted to run for this "
            "task (build/test/lint commands the implementation may invoke). Literal "
            "commands only, e.g. 'python -m pytest -q', 'uv run pytest'. Leave empty "
            "only when the task needs no commands beyond those in expected_tests."
        ),
    )
    agent_appended_allowed_commands: List[str] = Field(
        default_factory=list,
        description=(
            "Subset of allowed_commands appended at runtime by a leased agent via "
            "devcouncil_update_task_scope. These may run during execution/verification "
            "but cannot coarse-prove acceptance criteria (self-certification guard, "
            "mirroring agent_appended_expected_tests)."
        ),
    )
    forbidden_changes: List[str] = Field(default_factory=list)
    difficulty: Optional[Literal["easy", "normal", "hard"]] = Field(
        default=None,
        description=(
            "Manual difficulty override. When set (by a planner or a human) it wins over "
            "the deterministic estimator in devcouncil.verification.difficulty; hard tasks "
            "get stricter verification (stub/effort gates block, coverage enforced) per "
            "the verification.rigor config."
        ),
    )
    priority: Optional[Literal["high", "medium", "low"]] = Field(
        default=None,
        description="Optional human/planner priority hint (high/medium/low). Default None.",
    )
    depends_on: List[str] = Field(
        default_factory=list,
        description=(
            "IDs of tasks that must complete before this one (e.g. a task that creates a "
            "module this task imports/tests). Used to order execution and to skip a task "
            "whose prerequisites are unmet rather than letting it fail spuriously."
        ),
    )
    status: Literal[
        "planned",
        "ready",
        "running",
        "blocked",
        "verified",
        "done",
        "cancelled",
    ] = "planned"
