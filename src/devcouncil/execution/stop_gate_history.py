"""Persist stop-gate events under ``.devcouncil/logs/stop_gate.jsonl``."""

from __future__ import annotations

import json
import time
from pathlib import Path
from typing import Any

from devcouncil.verification.claims.models import CheckResult

CLAIM_CAP = 500
DETAIL_CAP = 2000
HISTORY_REL = Path(".devcouncil") / "logs" / "stop_gate.jsonl"


def history_path(project_root: Path) -> Path:
    return project_root / HISTORY_REL


def build_event(
    *,
    session_id: str,
    decision: str,
    claim: str = "",
    results: list[CheckResult] | None = None,
    task_id: str | None = None,
    blocking_gaps: int = 0,
    mode: str = "",
    extra: dict[str, Any] | None = None,
) -> dict[str, Any]:
    event: dict[str, Any] = {
        "ts": time.time(),
        "session_id": session_id,
        "claim": (claim or "")[:CLAIM_CAP],
        "decision": decision,
        "task_id": task_id,
        "blocking_gaps": blocking_gaps,
        "mode": mode,
        "results": [
            {
                "kind": r.assertion.kind.value,
                "target": r.assertion.target,
                "status": r.status.value,
                "source": r.assertion.source_text[:CLAIM_CAP],
                "detail": r.detail[:DETAIL_CAP],
            }
            for r in (results or [])
        ],
    }
    if extra:
        event.update(extra)
    return event


def append_event(project_root: Path, event: dict[str, Any]) -> None:
    try:
        path = history_path(project_root)
        path.parent.mkdir(parents=True, exist_ok=True)
        with open(path, "a", encoding="utf-8") as fh:
            fh.write(json.dumps(event) + "\n")
    except (OSError, TypeError, ValueError):
        pass


def read_events(project_root: Path, limit: int = 500) -> list[dict[str, Any]]:
    """Events newest-first; corrupt lines skipped."""
    try:
        lines = history_path(project_root).read_text(encoding="utf-8-sig", errors="replace").splitlines()
    except OSError:
        return []
    events: list[dict[str, Any]] = []
    for line in reversed(lines):
        if len(events) >= limit:
            break
        line = line.strip()
        if not line:
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(event, dict):
            events.append(event)
    return events


def session_tally(project_root: Path, session_id: str) -> tuple[int, int]:
    """Return (pass_or_allow_count, fail_or_block_count) for a session."""
    if not session_id:
        return 0, 0
    ok = 0
    bad = 0
    for event in read_events(project_root, limit=2000):
        if event.get("session_id") != session_id:
            continue
        decision = str(event.get("decision") or "")
        if decision in {"block", "assist"}:
            # assist with failures still counts as a miss for the shield tally
            results = event.get("results") or []
            has_fail = any(isinstance(r, dict) and r.get("status") == "fail" for r in results)
            if decision == "block" or has_fail or event.get("blocking_gaps", 0):
                bad += 1
            else:
                ok += 1
        elif decision in {"pass", "allow"}:
            ok += 1
    return ok, bad


def last_event(project_root: Path) -> dict[str, Any] | None:
    events = read_events(project_root, limit=1)
    return events[0] if events else None
