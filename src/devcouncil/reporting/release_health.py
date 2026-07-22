"""Release-health reporting: separate historical gaps from RC regressions.

A release candidate must not be marked green solely because historical project
gaps already existed. This module compares the current gap set against a
baseline snapshot and classifies each blocker as historical debt vs a new
regression introduced by the RC.
"""

from __future__ import annotations

import json
from dataclasses import asdict, dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Iterable, Literal, Mapping, Sequence

from devcouncil.domain.gap import Gap
from devcouncil.verification.gap_ids import gap_identity

ReleaseHealthVerdict = Literal["passed", "historical_debt", "regressed"]

DEFAULT_BASELINE_RELPATH = Path(".devcouncil") / "release_health_baseline.json"
SCHEMA_VERSION = 1


def gap_fingerprint(gap: Gap | Mapping[str, Any]) -> str:
    """Stable key for comparing gaps across baseline and current reports."""
    if isinstance(gap, Gap):
        gap_id = (gap.id or "").strip()
        if gap_id:
            return gap_id
        return gap_identity(gap)

    gap_id = str(gap.get("id") or "").strip()
    if gap_id:
        return gap_id
    return "|".join(
        (
            str(gap.get("gap_type") or ""),
            str(gap.get("file") or ""),
            str(gap.get("line") or ""),
            str(gap.get("acceptance_criterion_id") or ""),
            str(gap.get("description") or "").strip(),
        )
    )


def _gap_as_dict(gap: Gap | Mapping[str, Any]) -> dict[str, Any]:
    if isinstance(gap, Gap):
        return gap.model_dump()
    return dict(gap)


def _is_blocking(gap: Gap | Mapping[str, Any]) -> bool:
    if isinstance(gap, Gap):
        return bool(gap.blocking)
    return bool(gap.get("blocking"))


def _compact_gap(gap: Gap | Mapping[str, Any]) -> dict[str, Any]:
    data = _gap_as_dict(gap)
    return {
        "id": data.get("id"),
        "fingerprint": gap_fingerprint(gap),
        "severity": data.get("severity"),
        "gap_type": data.get("gap_type"),
        "task_id": data.get("task_id"),
        "requirement_id": data.get("requirement_id"),
        "description": data.get("description"),
        "blocking": bool(data.get("blocking")),
        "file": data.get("file"),
        "acceptance_criterion_id": data.get("acceptance_criterion_id"),
    }


@dataclass
class ReleaseHealthReport:
    """Machine-readable release-health classification."""

    schema_version: int = SCHEMA_VERSION
    verdict: ReleaseHealthVerdict = "passed"
    release_ready: bool = True
    baseline_path: str | None = None
    baseline_present: bool = False
    generated_at: str = ""
    counts: dict[str, int] = field(default_factory=dict)
    historical_blocking: list[dict[str, Any]] = field(default_factory=list)
    regressions: list[dict[str, Any]] = field(default_factory=list)
    resolved_since_baseline: list[dict[str, Any]] = field(default_factory=list)
    advisory_current: list[dict[str, Any]] = field(default_factory=list)
    notes: list[str] = field(default_factory=list)

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)

    def to_json(self, *, indent: int = 2) -> str:
        return json.dumps(self.to_dict(), indent=indent)

    def to_markdown(self) -> str:
        lines = [
            "# Release health",
            "",
            f"**Verdict:** `{self.verdict}`",
            f"**Release ready:** `{self.release_ready}`",
            "",
            "## Counts",
            "",
            f"- Blocking (current): {self.counts.get('blocking_current', 0)}",
            f"- Historical blocking: {self.counts.get('historical_blocking', 0)}",
            f"- RC regressions: {self.counts.get('regressions', 0)}",
            f"- Resolved since baseline: {self.counts.get('resolved_since_baseline', 0)}",
            f"- Advisory (current): {self.counts.get('advisory_current', 0)}",
            "",
        ]
        if self.notes:
            lines.append("## Notes")
            lines.append("")
            for note in self.notes:
                lines.append(f"- {note}")
            lines.append("")

        def _section(title: str, gaps: Sequence[Mapping[str, Any]]) -> None:
            lines.append(f"## {title}")
            lines.append("")
            if not gaps:
                lines.append("_None._")
                lines.append("")
                return
            for gap in gaps:
                gid = gap.get("id") or gap.get("fingerprint") or "unknown"
                desc = gap.get("description") or ""
                lines.append(f"- `{gid}` — {desc}")
            lines.append("")

        _section("RC regressions (new blockers)", self.regressions)
        _section("Historical blocking (pre-existing)", self.historical_blocking)
        _section("Resolved since baseline", self.resolved_since_baseline)
        return "\n".join(lines).rstrip() + "\n"


def build_baseline_snapshot(
    gaps: Iterable[Gap | Mapping[str, Any]],
    *,
    label: str = "",
) -> dict[str, Any]:
    """Serialize a baseline snapshot for later release-health comparison."""
    gap_list = list(gaps)
    blocking = [_compact_gap(g) for g in gap_list if _is_blocking(g)]
    advisory = [_compact_gap(g) for g in gap_list if not _is_blocking(g)]
    return {
        "schema_version": SCHEMA_VERSION,
        "kind": "release_health_baseline",
        "label": label,
        "captured_at": datetime.now(timezone.utc).isoformat(),
        "blocking": blocking,
        "advisory": advisory,
        "fingerprints": {
            "blocking": sorted({g["fingerprint"] for g in blocking}),
            "advisory": sorted({g["fingerprint"] for g in advisory}),
        },
    }


def write_baseline_snapshot(
    path: Path,
    gaps: Iterable[Gap | Mapping[str, Any]],
    *,
    label: str = "",
) -> dict[str, Any]:
    snapshot = build_baseline_snapshot(gaps, label=label)
    path = path.expanduser()
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(snapshot, indent=2) + "\n", encoding="utf-8")
    return snapshot


def load_baseline_snapshot(path: Path) -> dict[str, Any] | None:
    if not path.is_file():
        return None
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None
    return data if isinstance(data, dict) else None


def _fingerprints_from_baseline(baseline: Mapping[str, Any] | None) -> set[str]:
    if not baseline:
        return set()
    fps = baseline.get("fingerprints")
    if isinstance(fps, Mapping):
        blocking = fps.get("blocking")
        if isinstance(blocking, list):
            return {str(x) for x in blocking}
    blocking_gaps = baseline.get("blocking")
    if isinstance(blocking_gaps, list):
        out: set[str] = set()
        for item in blocking_gaps:
            if isinstance(item, Mapping):
                fp = item.get("fingerprint") or item.get("id")
                if fp:
                    out.add(str(fp))
        return out
    return set()


def _baseline_gap_index(baseline: Mapping[str, Any] | None) -> dict[str, dict[str, Any]]:
    if not baseline:
        return {}
    index: dict[str, dict[str, Any]] = {}
    for key in ("blocking", "advisory"):
        items = baseline.get(key)
        if not isinstance(items, list):
            continue
        for item in items:
            if isinstance(item, Mapping):
                fp = str(item.get("fingerprint") or item.get("id") or "")
                if fp:
                    index[fp] = dict(item)
    return index


def build_release_health_report(
    current_gaps: Iterable[Gap | Mapping[str, Any]],
    *,
    baseline: Mapping[str, Any] | None = None,
    baseline_path: str | Path | None = None,
) -> ReleaseHealthReport:
    """Classify current gaps against an optional historical baseline.

    Without a baseline, every current blocker is treated as an RC regression so
    a missing baseline cannot invent a green release from silent historical debt.
    """
    gaps = list(current_gaps)
    blocking = [g for g in gaps if _is_blocking(g)]
    advisory = [g for g in gaps if not _is_blocking(g)]

    baseline_fps = _fingerprints_from_baseline(baseline)
    baseline_index = _baseline_gap_index(baseline)
    baseline_present = baseline is not None

    historical: list[dict[str, Any]] = []
    regressions: list[dict[str, Any]] = []
    for gap in blocking:
        compact = _compact_gap(gap)
        fp = compact["fingerprint"]
        if baseline_present and fp in baseline_fps:
            historical.append(compact)
        else:
            regressions.append(compact)

    current_fps = {gap_fingerprint(g) for g in blocking}
    resolved: list[dict[str, Any]] = []
    if baseline_present:
        for fp in sorted(baseline_fps - current_fps):
            resolved.append(baseline_index.get(fp) or {"fingerprint": fp, "id": fp})

    notes: list[str] = []
    if not baseline_present:
        notes.append(
            "No baseline snapshot loaded; every current blocking gap is classified "
            "as an RC regression (fail-closed). Capture a baseline with "
            "`dev report release-health --write-baseline`."
        )
        verdict: ReleaseHealthVerdict = "regressed" if regressions else "passed"
    elif regressions:
        notes.append(
            "New blocking gaps appeared relative to the baseline; treat these as "
            "release-candidate regressions, not historical dogfood debt."
        )
        verdict = "regressed"
    elif historical:
        notes.append(
            "Only historical blocking gaps remain. This is not a green release — "
            "stale project debt must not be advertised as RC success."
        )
        verdict = "historical_debt"
    else:
        notes.append("No blocking gaps relative to baseline.")
        verdict = "passed"

    # Green / release_ready only when there are zero blocking gaps.
    release_ready = verdict == "passed"

    return ReleaseHealthReport(
        schema_version=SCHEMA_VERSION,
        verdict=verdict,
        release_ready=release_ready,
        baseline_path=str(baseline_path) if baseline_path else None,
        baseline_present=baseline_present,
        generated_at=datetime.now(timezone.utc).isoformat(),
        counts={
            "blocking_current": len(blocking),
            "historical_blocking": len(historical),
            "regressions": len(regressions),
            "resolved_since_baseline": len(resolved),
            "advisory_current": len(advisory),
        },
        historical_blocking=historical,
        regressions=regressions,
        resolved_since_baseline=resolved,
        advisory_current=[_compact_gap(g) for g in advisory],
        notes=notes,
    )


def resolve_baseline_path(project_root: Path, explicit: Path | None = None) -> Path:
    if explicit is not None:
        return explicit.expanduser()
    return project_root.expanduser().resolve() / DEFAULT_BASELINE_RELPATH


def compact_release_health_summary(report: ReleaseHealthReport) -> dict[str, Any]:
    """Compact projection for status / CI summaries."""
    return {
        "verdict": report.verdict,
        "release_ready": report.release_ready,
        "baseline_present": report.baseline_present,
        "counts": dict(report.counts),
        "regression_ids": [g.get("id") or g.get("fingerprint") for g in report.regressions],
        "historical_blocking_ids": [
            g.get("id") or g.get("fingerprint") for g in report.historical_blocking
        ],
    }
