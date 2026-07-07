"""Cross-run rigor analytics — tune thresholds from evidence, not guesses.

Aggregates persisted verification gaps and correction manifests to surface
recurring stub/effort failures and rough false-positive rates for advisory gaps.
"""

from __future__ import annotations

import json
from collections import Counter
from dataclasses import dataclass, field
from pathlib import Path
from typing import Dict, List

from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import GapRepository, TaskRepository
from devcouncil.utils.json_persist import read_json


@dataclass
class RigorAnalyticsReport:
    """Summary of rigor-gap patterns across all persisted verification runs."""

    total_gaps: int = 0
    by_gap_type: Dict[str, int] = field(default_factory=dict)
    stub_by_task: Dict[str, int] = field(default_factory=dict)
    effort_by_task: Dict[str, int] = field(default_factory=dict)
    stub_declared_count: int = 0
    tasks_with_repair_attempts: int = 0
    avg_repair_attempts: float = 0.0
    recurring_stub_tasks: List[str] = field(default_factory=list)
    notes: List[str] = field(default_factory=list)

    def to_markdown(self) -> str:
        lines = [
            "# Rigor analytics",
            "",
            f"- Total gaps recorded: **{self.total_gaps}**",
            f"- Stub declarations (allow-stub audit): **{self.stub_declared_count}**",
            f"- Tasks with repair manifests: **{self.tasks_with_repair_attempts}** "
            f"(avg attempts: {self.avg_repair_attempts:.1f})",
            "",
            "## Gap counts by type",
        ]
        for gap_type, count in sorted(self.by_gap_type.items(), key=lambda x: -x[1]):
            lines.append(f"- `{gap_type}`: {count}")
        if self.recurring_stub_tasks:
            lines.extend(["", "## Tasks recurring on stub gaps", ""])
            for tid in self.recurring_stub_tasks:
                lines.append(f"- {tid} ({self.stub_by_task.get(tid, 0)} stub gap(s))")
        if self.notes:
            lines.extend(["", "## Notes", ""])
            lines.extend(f"- {n}" for n in self.notes)
        return "\n".join(lines) + "\n"


def _manifest_attempts(project_root: Path) -> List[int]:
    runs_dir = project_root / ".devcouncil" / "runs"
    if not runs_dir.exists():
        return []
    attempts: List[int] = []
    for run_dir in runs_dir.iterdir():
        path = run_dir / "correction-manifest.json"
        if not path.exists():
            continue
        try:
            data = read_json(path)
            attempts.append(int(data.get("prior_failed_attempts", 0)) + 1)
        except Exception:
            continue
    return attempts


def build_rigor_report(project_root: Path) -> RigorAnalyticsReport:
    """Aggregate rigor-related gaps and repair history. Never raises."""
    report = RigorAnalyticsReport()
    db = get_db(project_root)
    if db is None:
        report.notes.append("No DevCouncil state database — nothing to analyze.")
        return report

    with db.get_session() as session:
        gaps = GapRepository(session).get_all()
        tasks = {t.id: t for t in TaskRepository(session).get_all()}

    report.total_gaps = len(gaps)
    report.by_gap_type = dict(Counter(g.gap_type for g in gaps))

    stub_counts: Counter[str] = Counter()
    effort_counts: Counter[str] = Counter()
    for gap in gaps:
        if gap.gap_type == "stub_detected" and gap.task_id:
            stub_counts[gap.task_id] += 1
        elif gap.gap_type == "suspicious_effort" and gap.task_id:
            effort_counts[gap.task_id] += 1
        elif gap.gap_type == "stub_declared":
            report.stub_declared_count += 1

    report.stub_by_task = dict(stub_counts)
    report.effort_by_task = dict(effort_counts)
    report.recurring_stub_tasks = sorted(
        tid for tid, n in stub_counts.items() if n >= 2
    )

    manifest_attempts = _manifest_attempts(project_root)
    if manifest_attempts:
        report.tasks_with_repair_attempts = len(manifest_attempts)
        report.avg_repair_attempts = sum(manifest_attempts) / len(manifest_attempts)

    effort_total = report.by_gap_type.get("suspicious_effort", 0)
    verified_tasks = sum(1 for t in tasks.values() if t.status in ("verified", "done"))
    if effort_total and verified_tasks:
        # Rough advisory false-positive proxy: effort gaps on tasks that still verified.
        verified_with_effort = sum(
            1 for tid in effort_counts if tid in tasks and tasks[tid].status in ("verified", "done")
        )
        if verified_with_effort:
            rate = verified_with_effort / max(1, len(effort_counts))
            report.notes.append(
                f"suspicious_effort advisory gaps on later-verified tasks: "
                f"{verified_with_effort}/{len(effort_counts)} ({rate:.0%}) — "
                "tune min_added_lines_per_planned_file if this is high."
            )

    if report.recurring_stub_tasks:
        report.notes.append(
            f"{len(report.recurring_stub_tasks)} task(s) hit stub_detected more than once — "
            "review repair prompts or raise stub_detection to 'always' for those areas."
        )

    return report
