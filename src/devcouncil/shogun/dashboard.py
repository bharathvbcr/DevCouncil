"""The battle dashboard — the Karo's single source of truth for the Lord.

Only the Karo writes ``.devcouncil/shogun/dashboard.md``. It renders the roster,
the order in play, work in progress, achievements (verified tasks) and anything
blocked, so the Lord (or the Shogun answering for the Lord) can read the state of
the campaign at a glance.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Dict, List, Optional


@dataclass
class RosterEntry:
    agent: str
    rank: str
    status: str = "idle"       # idle | working | reviewing
    current_task: str = "-"


@dataclass
class DashboardState:
    """Everything the dashboard renders — mutated by the campaign as it runs."""

    goal: str = ""
    roster: List[RosterEntry] = field(default_factory=list)
    in_progress: List[str] = field(default_factory=list)   # "T-001 · ashigaru1 · Apply"
    achievements: List[str] = field(default_factory=list)   # verified task lines
    blocked: List[str] = field(default_factory=list)        # "T-002 — <gap>"
    skipped: List[str] = field(default_factory=list)        # unmet dependency
    routing: Dict[str, int] = field(default_factory=dict)

    def roster_for(self, agent: str) -> Optional[RosterEntry]:
        for entry in self.roster:
            if entry.agent == agent:
                return entry
        return None


class DashboardWriter:
    """Renders :class:`DashboardState` to markdown and writes it atomically-ish."""

    def __init__(self, root: Path | str = Path(".")):
        self.root = Path(root)
        self.path = self.root / ".devcouncil" / "shogun" / "dashboard.md"

    def render(self, state: DashboardState) -> str:
        now = datetime.now(timezone.utc).strftime("%Y-%m-%d %H:%M:%SZ")
        lines: List[str] = []
        lines.append("# 陣中日誌 · Shogun Campaign Dashboard")
        lines.append("")
        lines.append(f"_Updated {now} — written by the Karo._")
        lines.append("")
        lines.append("## Order")
        lines.append("")
        lines.append(f"> {state.goal or '(none)'}")
        lines.append("")
        if state.routing:
            routed = ", ".join(f"{k}: {v}" for k, v in state.routing.items())
            lines.append(f"Routing — {routed}")
            lines.append("")

        lines.append("## Roster")
        lines.append("")
        lines.append("| Agent | Rank | Status | Current |")
        lines.append("| --- | --- | --- | --- |")
        for e in state.roster:
            lines.append(f"| {e.agent} | {e.rank} | {e.status} | {e.current_task} |")
        lines.append("")

        lines.append(f"## In Progress ({len(state.in_progress)})")
        lines.append("")
        lines.extend(f"- ⚔️ {row}" for row in state.in_progress)
        if not state.in_progress:
            lines.append("- (quiet)")
        lines.append("")

        lines.append(f"## 戦果 · Achievements ({len(state.achievements)})")
        lines.append("")
        lines.extend(f"- ✅ {row}" for row in state.achievements)
        if not state.achievements:
            lines.append("- (none yet)")
        lines.append("")

        if state.blocked:
            lines.append(f"## Blocked ({len(state.blocked)})")
            lines.append("")
            lines.extend(f"- ⛔ {row}" for row in state.blocked)
            lines.append("")

        if state.skipped:
            lines.append(f"## Skipped — unmet dependencies ({len(state.skipped)})")
            lines.append("")
            lines.extend(f"- ⏭️ {row}" for row in state.skipped)
            lines.append("")

        return "\n".join(lines).rstrip() + "\n"

    def write(self, state: DashboardState) -> Path:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self.path.write_text(self.render(state), encoding="utf-8")
        return self.path
