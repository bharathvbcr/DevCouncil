"""MCP prompt templates and live status snapshots."""

from __future__ import annotations

from pathlib import Path
from typing import NamedTuple

from mcp.types import GetPromptResult, Prompt, PromptArgument, PromptMessage, TextContent

from devcouncil.app.project_status import compute_phase
from devcouncil.integrations.mcp.util import normalize_arguments
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import ArtifactGraphRepository, StateRepository


class PromptSpec(NamedTuple):
    name: str
    description: str
    arguments: list[PromptArgument]


PROMPT_SPECS: list[PromptSpec] = [
    PromptSpec(
        name="devcouncil_implement_next_task",
        description="Pick up the next unblocked DevCouncil task and implement it through the policy-gated MCP loop.",
        arguments=[
            PromptArgument(name="client_id", description="Optional stable client id used for the task lease.", required=False),
        ],
    ),
    PromptSpec(
        name="devcouncil_repair_task",
        description="Repair the blocking verification gaps for a task (defaults to the active running task).",
        arguments=[
            PromptArgument(name="task_id", description="Task id, e.g. TASK-001. Defaults to the active task.", required=False),
        ],
    ),
    PromptSpec(
        name="devcouncil_verify_task",
        description="Run DevCouncil verification for a task and report blocking gaps.",
        arguments=[
            PromptArgument(name="task_id", description="Task id, e.g. TASK-001. Defaults to the active task.", required=False),
        ],
    ),
    PromptSpec(
        name="devcouncil_review_live",
        description="Review pending live-review critique cards and resolve the blocking ones.",
        arguments=[
            PromptArgument(name="task_id", description="Optional task scope for the live-review cards.", required=False),
        ],
    ),
    PromptSpec(
        name="devcouncil_project_status",
        description="Summarize the current DevCouncil project phase, tasks, and blocking gaps.",
        arguments=[],
    ),
    PromptSpec(
        name="devcouncil_apply_knowledge",
        description="Select the ingested project knowledge (OKF + design) that applies to a goal and inject it.",
        arguments=[
            PromptArgument(name="goal", description="The task or goal to find applicable project knowledge for.", required=True),
        ],
    ),
]


def status_snapshot(root: Path) -> str:
    """A short live status block for prompt bodies, or an init hint when uninitialized."""
    db = get_db(root)
    if not db:
        return (
            "DevCouncil is not initialized here yet — run `dev status` or `dev map` "
            "(both auto-bootstrap a minimal project) or `dev init` for full setup."
        )
    try:
        with db.get_session() as session:
            graph = ArtifactGraphRepository(session).load_graph()
            summary = graph.coverage_summary()
            state = StateRepository(session).get_state()
            phase = compute_phase(graph, state.current_phase if state else None)
        return (
            f"Phase: {phase} | "
            f"tasks: {summary['total_tasks']} | "
            f"gaps: {summary['total_gaps']} ({summary['blocking_gaps']} blocking)"
        )
    except Exception:
        return "DevCouncil status unavailable."


def render_prompt_text(name: str, arguments: dict, root: Path) -> str:
    snapshot = status_snapshot(root)
    if name == "devcouncil_implement_next_task":
        client_id = arguments.get("client_id") or "claude-code"
        return (
            "You are implementing the next DevCouncil task under policy enforcement.\n\n"
            f"Project status: {snapshot}\n\n"
            "Do exactly this:\n"
            "1. Call `devcouncil_next_task` to get the highest-priority unblocked task.\n"
            f"2. Call `devcouncil_checkout_task` with that task_id and client_id='{client_id}' to acquire a lease.\n"
            "3. Read the task scope with `devcouncil_get_task` and `devcouncil_get_prompt`; inspect files with `devcouncil_read_file` and `devcouncil_get_diff`.\n"
            "4. Make changes ONLY through `devcouncil_write_file` / `devcouncil_apply_patch` (the policy gate rejects out-of-scope or protected paths) and run tests with `devcouncil_run_command`.\n"
            "5. Call `devcouncil_verify_task`; if it reports blocking gaps, fix them and re-verify.\n"
            "6. When verified, call `devcouncil_release_task` with the lease token.\n\n"
            "Never edit files outside the task scope. If a write is rejected, call `devcouncil_update_task_scope` only when the change is legitimately in-scope."
        )
    if name == "devcouncil_repair_task":
        task_id = arguments.get("task_id") or "(the active task)"
        return (
            f"Repair the blocking verification gaps for {task_id}.\n\n"
            f"Project status: {snapshot}\n\n"
            "1. Call `devcouncil_get_gaps` (blocking_only=true) and `devcouncil_get_next_actions` for the task.\n"
            "2. Inspect the relevant files and evidence with `devcouncil_read_file` and `devcouncil_get_evidence`.\n"
            "3. Apply minimal fixes via `devcouncil_apply_patch` / `devcouncil_write_file`.\n"
            "4. Re-run `devcouncil_verify_task` until no blocking gaps remain, then `devcouncil_release_task`."
        )
    if name == "devcouncil_verify_task":
        task_id = arguments.get("task_id") or "(the active task)"
        return (
            f"Run DevCouncil verification for {task_id} and report the result.\n\n"
            f"Project status: {snapshot}\n\n"
            "Call `devcouncil_verify_task` (you must hold the task lease via `devcouncil_checkout_task`). "
            "Summarize the blocking gaps and proposed next actions; do not mark work complete while blocking gaps remain."
        )
    if name == "devcouncil_review_live":
        scope = arguments.get("task_id")
        scope_line = f" scoped to {scope}" if scope else ""
        return (
            f"Review the pending live-review critique cards{scope_line}.\n\n"
            "1. Call `devcouncil_live_review` for the blocker count and `devcouncil_live_cards` (status='open') for the cards.\n"
            "2. For each blocking card, call `devcouncil_live_repair_prompt` (or `devcouncil_live_repair_all`) to get a ready-to-apply repair.\n"
            "3. Apply the fixes through the policy-gated write tools and re-verify."
        )
    if name == "devcouncil_project_status":
        return (
            "Summarize the DevCouncil project state for the user.\n\n"
            f"Live snapshot: {snapshot}\n\n"
            "Call `devcouncil_status` and `devcouncil_report` for the full coverage report, then give a concise "
            "phase / tasks / blocking-gaps summary and recommend the next action."
        )
    if name == "devcouncil_apply_knowledge":
        goal = arguments.get("goal") or ""
        return (
            f"Find and apply the project knowledge that applies to this goal: {goal!r}.\n\n"
            "Call `devcouncil_select_knowledge` with the goal, then treat the returned preamble as authoritative "
            "project context (design system + OKF docs) for any code you write toward this goal."
        )
    return f"Unknown DevCouncil prompt: {name}"


def list_prompts() -> list[Prompt]:
    return [
        Prompt(name=spec.name, description=spec.description, arguments=list(spec.arguments))
        for spec in PROMPT_SPECS
    ]


def get_prompt(name: str, arguments: dict | None, root: Path) -> GetPromptResult:
    spec = next((spec for spec in PROMPT_SPECS if spec.name == name), None)
    if spec is None:
        raise ValueError(f"Unknown prompt: {name}")
    args = normalize_arguments(arguments)
    text = render_prompt_text(name, args, root)
    return GetPromptResult(
        description=spec.description,
        messages=[PromptMessage(role="user", content=TextContent(type="text", text=text))],
    )
