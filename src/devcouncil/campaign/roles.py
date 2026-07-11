"""The Director command hierarchy — ranks, duties, and enforceable boundaries.

Each :class:`Role` couples a rank to the *actions it is permitted to take*. The
original encodes "who may do what / who may never do what" as prose in role
markdown; here that contract is also machine-checkable via
:func:`assert_allowed`, so the orchestrator can hard-enforce the chain of
command (only the Coordinator writes the dashboard, only the Reviewer performs QC, an
Worker never reviews its own work, and so on) rather than trusting a prompt.

The human-readable persona/duty text still lives in ``prompts/<rank>.md`` and is
surfaced by :func:`load_role_instructions` for injection into an agent's
context.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum
from pathlib import Path
from typing import Dict, FrozenSet, List

_PROMPTS_DIR = Path(__file__).parent / "prompts"


class Rank(str, Enum):
    """A position in the command hierarchy."""

    DIRECTOR = "director"
    COORDINATOR = "coordinator"
    WORKER = "worker"
    REVIEWER = "reviewer"

    def __str__(self) -> str:  # nicer CLI/log output
        return self.value


class Action(str, Enum):
    """A discrete capability that a rank may or may not exercise."""

    RELAY_ORDER = "relay_order"          # accept the operator's order, hand to Coordinator
    READ_DASHBOARD = "read_dashboard"
    APPROVE_SKILL = "approve_skill"
    DECOMPOSE = "decompose"              # split an order into subtasks
    ASSIGN = "assign"                    # dispatch a task to a worker
    WRITE_DASHBOARD = "write_dashboard"  # sole right of the Coordinator
    ROUTE_QC = "route_qc"                # send finished work to the Reviewer
    ROLLUP = "rollup"                    # aggregate reports, decide done/blocked
    NOTIFY = "notify"                    # push to the operator (ntfy / dashboard)
    EXECUTE_TASK = "execute_task"        # actually do the work
    SELF_REVIEW = "self_review"          # a worker's own sanity check
    WRITE_REPORT = "write_report"        # a worker reporting completion
    QC_REVIEW = "qc_review"              # quality control of another's work
    DEEP_ANALYSIS = "deep_analysis"      # architecture / root-cause (Bloom L4-6)
    AGGREGATE_REPORTS = "aggregate_reports"
    CONTACT_HUMAN = "contact_human"      # only the Director/Coordinator may reach the operator


class ForbiddenActionError(RuntimeError):
    """Raised when a rank attempts an action outside its remit."""

    def __init__(self, rank: "Rank", action: "Action"):
        self.rank = rank
        self.action = action
        super().__init__(
            f"{rank} is forbidden from '{action.value}' — it violates the role hierarchy"
        )


@dataclass(frozen=True)
class Role:
    """A rank plus its persona and the actions it is permitted to take."""

    rank: Rank
    title: str
    summary: str
    allowed: FrozenSet[Action]
    reports_to: Rank | None
    # Human-facing bullet duties, mirrored in prompts/<rank>.md for quick display.
    duties: List[str] = field(default_factory=list)

    def may(self, action: Action) -> bool:
        return action in self.allowed

    @property
    def forbidden(self) -> FrozenSet[Action]:
        return frozenset(Action) - self.allowed

    def instructions(self) -> str:
        return load_role_instructions(self.rank)


ROLES: Dict[Rank, Role] = {
    Rank.DIRECTOR: Role(
        rank=Rank.DIRECTOR,
        title="Director ",
        summary="Relays the operator's order to the Coordinator and steps back. Never works.",
        allowed=frozenset(
            {
                Action.RELAY_ORDER,
                Action.READ_DASHBOARD,
                Action.APPROVE_SKILL,
                Action.NOTIFY,
                Action.CONTACT_HUMAN,
            }
        ),
        reports_to=None,
        duties=[
            "Receive the operator's order and record it as a campaign command.",
            "Hand the command to the Coordinator, then yield so the operator may issue more.",
            "Read the dashboard; never write it, never execute a task, never bypass the Coordinator.",
        ],
    ),
    Rank.COORDINATOR: Role(
        rank=Rank.COORDINATOR,
        title="Coordinator ",
        summary="Decomposes the order, dispatches Worker in parallel, owns the dashboard.",
        allowed=frozenset(
            {
                Action.DECOMPOSE,
                Action.ASSIGN,
                Action.WRITE_DASHBOARD,
                Action.ROUTE_QC,
                Action.ROLLUP,
                Action.NOTIFY,
                Action.READ_DASHBOARD,
                Action.CONTACT_HUMAN,
            }
        ),
        reports_to=Rank.DIRECTOR,
        duties=[
            "Decompose the command into subtasks and classify each by Bloom level.",
            "Dispatch execution tasks to Worker in parallel; route analysis/QC to the Reviewer.",
            "Own the dashboard and roll finished, verified work up to the Director.",
            "Never do the work yourself — 'one Worker can do it all' is Coordinator laziness.",
        ],
    ),
    Rank.WORKER: Role(
        rank=Rank.WORKER,
        title="Worker ",
        summary="Runs exactly one task through a coding executor, then reports to the Reviewer.",
        allowed=frozenset(
            {
                Action.EXECUTE_TASK,
                Action.SELF_REVIEW,
                Action.WRITE_REPORT,
            }
        ),
        reports_to=Rank.REVIEWER,
        duties=[
            "Execute the single task assigned to you and nothing beyond its scope.",
            "Self-review against the parent command, then write a completion report.",
            "Notify the Reviewer for QC. Never QC your own work, never touch another's task, never poll.",
        ],
    ),
    Rank.REVIEWER: Role(
        rank=Rank.REVIEWER,
        title="Reviewer ",
        summary="A thinker, not a doer. Verifies finished work and handles Bloom L4-L6.",
        allowed=frozenset(
            {
                Action.QC_REVIEW,
                Action.DEEP_ANALYSIS,
                Action.AGGREGATE_REPORTS,
                Action.WRITE_REPORT,
            }
        ),
        reports_to=Rank.COORDINATOR,
        duties=[
            "Quality-control Worker output through the DevCouncil Verifier.",
            "Own architecture, root-cause and strategy work (Bloom Analyze/Evaluate/Create).",
            "Aggregate verdicts and report up to the Coordinator. Never manage Worker, never implement.",
        ],
    ),
}


def get_role(rank: Rank | str) -> Role:
    """Look up a :class:`Role` by rank (enum or string)."""
    if isinstance(rank, str):
        rank = Rank(rank)
    return ROLES[rank]


def assert_allowed(rank: Rank | str, action: Action) -> None:
    """Raise :class:`ForbiddenActionError` if ``rank`` may not perform ``action``."""
    role = get_role(rank)
    if not role.may(action):
        raise ForbiddenActionError(role.rank, action)


def load_role_instructions(rank: Rank | str) -> str:
    """Return the persona/duty markdown for ``rank`` (``prompts/<rank>.md``).

    Falls back to a synthesised summary when the file is missing so the system is
    resilient to a trimmed install.
    """
    if isinstance(rank, str):
        rank = Rank(rank)
    path = _PROMPTS_DIR / f"{rank.value}.md"
    if path.exists():
        return path.read_text(encoding="utf-8")
    role = ROLES[rank]
    lines = [f"# {role.title}", "", role.summary, ""]
    lines += [f"- {duty}" for duty in role.duties]
    return "\n".join(lines)
