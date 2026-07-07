"""The Shogun command hierarchy — ranks, duties, and enforceable boundaries.

Each :class:`Role` couples a rank to the *actions it is permitted to take*. The
original encodes "who may do what / who may never do what" as prose in role
markdown; here that contract is also machine-checkable via
:func:`assert_allowed`, so the orchestrator can hard-enforce the chain of
command (only the Karo writes the dashboard, only the Gunshi performs QC, an
Ashigaru never reviews its own work, and so on) rather than trusting a prompt.

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
    """A position in the feudal chain of command."""

    SHOGUN = "shogun"
    KARO = "karo"
    ASHIGARU = "ashigaru"
    GUNSHI = "gunshi"

    def __str__(self) -> str:  # nicer CLI/log output
        return self.value


class Action(str, Enum):
    """A discrete capability that a rank may or may not exercise."""

    RELAY_ORDER = "relay_order"          # accept the Lord's order, hand to Karo
    READ_DASHBOARD = "read_dashboard"
    APPROVE_SKILL = "approve_skill"
    DECOMPOSE = "decompose"              # split an order into subtasks
    ASSIGN = "assign"                    # dispatch a task to a worker
    WRITE_DASHBOARD = "write_dashboard"  # sole right of the Karo
    ROUTE_QC = "route_qc"                # send finished work to the Gunshi
    ROLLUP = "rollup"                    # aggregate reports, decide done/blocked
    NOTIFY = "notify"                    # push to the Lord (ntfy / dashboard)
    EXECUTE_TASK = "execute_task"        # actually do the work
    SELF_REVIEW = "self_review"          # a worker's own sanity check
    WRITE_REPORT = "write_report"        # a worker reporting completion
    QC_REVIEW = "qc_review"              # quality control of another's work
    DEEP_ANALYSIS = "deep_analysis"      # architecture / root-cause (Bloom L4-6)
    AGGREGATE_REPORTS = "aggregate_reports"
    CONTACT_HUMAN = "contact_human"      # only the Shogun/Karo may reach the Lord


class ForbiddenActionError(RuntimeError):
    """Raised when a rank attempts an action outside its remit."""

    def __init__(self, rank: "Rank", action: "Action"):
        self.rank = rank
        self.action = action
        super().__init__(
            f"{rank} is forbidden from '{action.value}' — it violates the chain of command"
        )


@dataclass(frozen=True)
class Role:
    """A rank plus its persona and the actions it is permitted to take."""

    rank: Rank
    title_ja: str
    title_en: str
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
    Rank.SHOGUN: Role(
        rank=Rank.SHOGUN,
        title_ja="将軍",
        title_en="Shogun · Supreme Commander",
        summary="Relays the Lord's order to the Karo and steps back. Never works.",
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
            "Receive the Lord's order and record it as a campaign command.",
            "Hand the command to the Karo, then yield so the Lord may issue more.",
            "Read the dashboard; never write it, never execute a task, never bypass the Karo.",
        ],
    ),
    Rank.KARO: Role(
        rank=Rank.KARO,
        title_ja="家老",
        title_en="Karo · Chief Retainer (traffic control)",
        summary="Decomposes the order, dispatches Ashigaru in parallel, owns the dashboard.",
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
        reports_to=Rank.SHOGUN,
        duties=[
            "Decompose the command into subtasks and classify each by Bloom level.",
            "Dispatch execution tasks to Ashigaru in parallel; route analysis/QC to the Gunshi.",
            "Own the dashboard and roll finished, verified work up to the Shogun.",
            "Never do the work yourself — 'one Ashigaru can do it all' is Karo laziness.",
        ],
    ),
    Rank.ASHIGARU: Role(
        rank=Rank.ASHIGARU,
        title_ja="足軽",
        title_en="Ashigaru · Foot-soldier (worker)",
        summary="Runs exactly one task through a coding executor, then reports to the Gunshi.",
        allowed=frozenset(
            {
                Action.EXECUTE_TASK,
                Action.SELF_REVIEW,
                Action.WRITE_REPORT,
            }
        ),
        reports_to=Rank.GUNSHI,
        duties=[
            "Execute the single task assigned to you and nothing beyond its scope.",
            "Self-review against the parent command, then write a completion report.",
            "Notify the Gunshi for QC. Never QC your own work, never touch another's task, never poll.",
        ],
    ),
    Rank.GUNSHI: Role(
        rank=Rank.GUNSHI,
        title_ja="軍師",
        title_en="Gunshi · Strategist (quality control)",
        summary="A thinker, not a doer. Verifies finished work and handles Bloom L4-L6.",
        allowed=frozenset(
            {
                Action.QC_REVIEW,
                Action.DEEP_ANALYSIS,
                Action.AGGREGATE_REPORTS,
                Action.WRITE_REPORT,
            }
        ),
        reports_to=Rank.KARO,
        duties=[
            "Quality-control Ashigaru output through the DevCouncil Verifier.",
            "Own architecture, root-cause and strategy work (Bloom Analyze/Evaluate/Create).",
            "Aggregate verdicts and report up to the Karo. Never manage Ashigaru, never implement.",
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
    lines = [f"# {role.title_en}", "", role.summary, ""]
    lines += [f"- {duty}" for duty in role.duties]
    return "\n".join(lines)
