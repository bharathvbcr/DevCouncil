"""Unified Stop / SubagentStop gate: claim checks + active-task verify."""

from __future__ import annotations

import logging
import os
import time
import traceback
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from devcouncil.execution.stop_gate_history import append_event, build_event, last_event, session_tally
from devcouncil.execution.stop_gate_state import get_block_count, increment_block_count
from devcouncil.execution.stop_gate_verify_cache import load_verify_cache, record_verify_cache
from devcouncil.live.tasks import active_task_id
from devcouncil.telemetry.traces import TraceLogger
from devcouncil.verification.claims.checks import (
    ClaimCheckBudget,
    execute_checks,
    resolve_commands_from_config,
)
from devcouncil.verification.claims.mapper import map_claims
from devcouncil.verification.claims.models import CheckResult, Status
from devcouncil.verification.claims.transcript import (
    ends_on_open_question,
    last_assistant_sentence,
    last_assistant_text,
)
from devcouncil.verification.claims.verdict import decide_claims, summary_line

logger = logging.getLogger(__name__)


@dataclass
class StopGateResult:
    """Outcome emitted to the coding-CLI Stop hook."""

    decision: str  # pass | assist | block
    reason: str = ""
    system_message: str | None = None
    claim_results: list[CheckResult] = field(default_factory=list)
    task_id: str | None = None
    blocking_gaps: int = 0
    next_actions: list[str] = field(default_factory=list)
    mode: str = "off"
    fail_open: bool = False


def _resolve_mode(configured: str) -> str:
    env = (os.environ.get("DEVCOUNCIL_STOP_GATE") or "").strip().lower()
    if env in {"off", "assist", "block"}:
        return env
    mode = (configured or "off").strip().lower()
    return mode if mode in {"off", "assist", "block"} else "off"


def _load_stop_gate_config(project_root: Path) -> tuple[Any, Any]:
    from devcouncil.app.config import load_config

    cfg = load_config(project_root)
    return cfg, cfg.execution.stop_gate


def _run_claim_pass(
    project_root: Path,
    claim_text: str,
    *,
    commands_cfg: object,
    per_command_timeout: int,
    total_timeout: int,
) -> list[CheckResult]:
    assertions = map_claims(claim_text)
    if not assertions:
        return []
    resolved = resolve_commands_from_config(commands_cfg)
    budget = ClaimCheckBudget(
        per_command_timeout=per_command_timeout,
        total_timeout=total_timeout,
    )
    return execute_checks(assertions, cwd=project_root, commands=resolved, budget=budget)


def _run_task_verify(
    project_root: Path,
    *,
    ttl_minutes: int,
) -> tuple[str | None, int, list[str], bool]:
    """Return (task_id, blocking_count, next_action_strings, used_cache)."""
    task_id = active_task_id(project_root)
    if not task_id:
        return None, 0, [], False

    cached = load_verify_cache(project_root, task_id=task_id, ttl_minutes=ttl_minutes)
    if cached is not None:
        actions = []
        for item in cached.get("next_actions") or []:
            if isinstance(item, dict) and item.get("action"):
                actions.append(str(item["action"]))
            elif isinstance(item, str):
                actions.append(item)
        return task_id, int(cached.get("blocking_gaps") or 0), actions[:10], True

    try:
        import asyncio

        from devcouncil.domain.evidence import CommandResult, DiffCoverageEvidence, DiffEvidence, TestEvidence
        from devcouncil.storage.db import get_db
        from devcouncil.storage.repositories import (
            EvidenceRepository,
            GapRepository,
            RequirementRepository,
            TaskRepository,
        )
        from devcouncil.verification.next_actions import split_next_actions
        from devcouncil.verification.verifier import Verifier

        db = get_db(project_root)
        if not db:
            return task_id, 0, [], False
        with db.get_session() as session:
            task = TaskRepository(session).get_by_id(task_id)
            if not task:
                return task_id, 0, [], False
            reqs = RequirementRepository(session).get_all()
            gaps, evidence = asyncio.run(Verifier(project_root).verify_task(task, reqs))
            gap_repo = GapRepository(session)
            ev_repo = EvidenceRepository(session)
            gap_repo.delete_for_task(task.id)
            ev_repo.delete_for_task(task.id)
            for gap in gaps:
                gap_repo.save(gap)
            for ev in evidence:
                if isinstance(ev, CommandResult):
                    ev_repo.save_command_result(task.id, ev)
                elif isinstance(ev, DiffCoverageEvidence):
                    ev_repo.save_diff_coverage_evidence(ev)
                elif isinstance(ev, DiffEvidence):
                    ev_repo.save_diff_evidence(ev)
                elif isinstance(ev, TestEvidence):
                    ev_repo.save_test_evidence(ev, task.id)
            blocking = [g for g in gaps if g.blocking]
            task.status = "blocked" if blocking else "verified"
            TaskRepository(session).save(task)
        blocking_actions, _ = split_next_actions(gaps)
        action_strs = [a.action for a in blocking_actions[:10]]
        record_verify_cache(
            project_root,
            task_id=task_id,
            status=task.status,
            blocking_gaps=len(blocking),
            next_actions=[a.model_dump() for a in blocking_actions[:20]],
            passed=len(blocking) == 0,
        )
        return task_id, len(blocking), action_strs, False
    except Exception as exc:  # noqa: BLE001 — fail open inside claim of verify
        logger.debug("stop_gate task verify failed: %s", exc)
        return task_id, 0, [], False


def _merge_corrective(
    claim_reason: str,
    task_id: str | None,
    blocking_gaps: int,
    next_actions: list[str],
) -> str:
    parts: list[str] = []
    if claim_reason:
        parts.append(claim_reason)
    if task_id and blocking_gaps:
        parts.append(
            f"ACTIVE TASK VERIFICATION FAILED for {task_id}: {blocking_gaps} blocking gap(s)."
        )
        if next_actions:
            bullets = "\n".join(f"- {a}" for a in next_actions[:5])
            parts.append("Next actions:\n" + bullets)
        parts.append("Fix the gaps (or run `dev repair`), then re-verify before stopping.")
    return "\n\n".join(parts)


def _system_message(
    *,
    claim_results: list[CheckResult],
    blocking_gaps: int,
    decision: str,
    notify_on_pass: bool,
) -> str | None:
    claim_part = summary_line(claim_results) if claim_results else None
    task_part = None
    if blocking_gaps:
        task_part = f"task blocked ({blocking_gaps} gap(s))"
    elif decision == "pass" and notify_on_pass:
        task_part = "task ✓" if claim_results else "stop-gate ✓"

    if claim_part and task_part:
        # summary_line already has prefix; append task note
        return f"{claim_part} | {task_part}"
    if claim_part:
        return claim_part
    if task_part:
        return f"🛡 devcouncil: {task_part}"
    if decision == "assist" and blocking_gaps == 0 and claim_results:
        return claim_part
    return None


def evaluate_stop(project_root: Path, payload: dict[str, Any] | None = None) -> StopGateResult:
    """Single entry for Stop / SubagentStop. Fail-open on internal errors."""
    payload = payload if isinstance(payload, dict) else {}
    root = project_root.expanduser().resolve()
    session_id = str(payload.get("session_id") or "unknown")

    try:
        cfg, sg = _load_stop_gate_config(root)
        mode = _resolve_mode(getattr(sg, "mode", "off"))
        if mode == "off":
            return StopGateResult(decision="pass", mode=mode)

        stop_hook_active = bool(payload.get("stop_hook_active"))
        blocks_so_far = get_block_count(root, session_id)
        if stop_hook_active and blocks_so_far == 0:
            TraceLogger(root).log_event(
                "stop_gate_fail_open",
                {"reason": "stop_hook_active_without_prior_block", "session_id": session_id},
                summary="stop_gate fail-open (another hook blocked)",
            )
            return StopGateResult(decision="pass", mode=mode, fail_open=True)

        claim_text = ""
        transcript_path = payload.get("transcript_path")
        if transcript_path and getattr(sg, "check_claims", True):
            claim_text = last_assistant_text(Path(str(transcript_path))) or ""
        # Allow direct claim injection for tests / non-Claude clients
        if not claim_text and isinstance(payload.get("claim_text"), str):
            claim_text = payload["claim_text"]

        claim_results: list[CheckResult] = []
        if getattr(sg, "check_claims", True) and claim_text:
            claim_results = _run_claim_pass(
                root,
                claim_text,
                commands_cfg=cfg.commands,
                per_command_timeout=int(getattr(sg, "per_command_timeout", 90)),
                total_timeout=int(getattr(sg, "total_timeout", 120)),
            )

        task_id: str | None = None
        blocking_gaps = 0
        next_actions: list[str] = []
        if getattr(sg, "verify_active_task", True):
            task_id, blocking_gaps, next_actions, _ = _run_task_verify(
                root,
                ttl_minutes=int(getattr(sg, "verify_cache_minutes", 5)),
            )

        max_blocks = int(getattr(sg, "max_blocks", 2))
        claim_verdict = decide_claims(claim_results, blocks_so_far, max_blocks)
        claim_failures = any(r.status is Status.FAIL for r in claim_results)
        task_failures = blocking_gaps > 0
        has_failure = claim_failures or task_failures
        at_cap = blocks_so_far >= max_blocks

        reason = _merge_corrective(
            claim_verdict.reason if claim_verdict.block or (claim_failures and not at_cap) else "",
            task_id,
            blocking_gaps,
            next_actions,
        )
        if claim_failures and not reason:
            # At cap, decide_claims clears reason — still surface a short assist note.
            reason = summary_line(claim_results)

        notify_on_pass = bool(getattr(sg, "notify_on_pass", False))

        if not has_failure:
            decision = "pass"
            sys_msg = _system_message(
                claim_results=claim_results,
                blocking_gaps=0,
                decision=decision,
                notify_on_pass=notify_on_pass,
            )
        elif mode == "block" and not at_cap:
            decision = "block"
            if not reason:
                reason = _merge_corrective("", task_id, blocking_gaps, next_actions) or summary_line(
                    claim_results
                )
            sys_msg = _system_message(
                claim_results=claim_results,
                blocking_gaps=blocking_gaps,
                decision=decision,
                notify_on_pass=True,
            )
            increment_block_count(root, session_id)
        else:
            decision = "assist"
            sys_msg = _system_message(
                claim_results=claim_results,
                blocking_gaps=blocking_gaps,
                decision=decision,
                notify_on_pass=True,
            )
            if reason and sys_msg:
                sys_msg = f"{sys_msg}\n{reason}" if len(reason) < 800 else sys_msg
            elif reason:
                sys_msg = reason[:1200]

        event = build_event(
            session_id=session_id,
            decision=decision,
            claim=claim_text,
            results=claim_results,
            task_id=task_id,
            blocking_gaps=blocking_gaps,
            mode=mode,
        )
        append_event(root, event)
        TraceLogger(root).log_event(
            "stop_gate",
            {
                "session_id": session_id,
                "decision": decision,
                "mode": mode,
                "task_id": task_id,
                "blocking_gaps": blocking_gaps,
                "claim_fail": sum(1 for r in claim_results if r.status is Status.FAIL),
                "claim_pass": sum(1 for r in claim_results if r.status is Status.PASS),
            },
            task_id=task_id,
            summary=f"stop_gate {decision} (mode={mode})",
        )

        return StopGateResult(
            decision=decision,
            reason=reason if decision == "block" else (reason if decision == "assist" else ""),
            system_message=sys_msg,
            claim_results=claim_results,
            task_id=task_id,
            blocking_gaps=blocking_gaps,
            next_actions=next_actions,
            mode=mode,
        )
    except Exception:  # noqa: BLE001 — prime directive: fail open
        try:
            TraceLogger(root).log_event(
                "stop_gate_error",
                {"session_id": session_id, "traceback": traceback.format_exc()[-2000:]},
                summary="stop_gate fail-open on internal error",
            )
        except Exception:
            logger.debug("stop_gate fail-open logging failed", exc_info=True)
        return StopGateResult(decision="pass", fail_open=True, mode="off")


def session_briefing(project_root: Path, payload: dict[str, Any] | None = None) -> str | None:
    """Richer SessionStart additionalContext (Hindsight-lite)."""
    payload = payload if isinstance(payload, dict) else {}
    parts: list[str] = []

    # Base status comes from hook._status_line; we append continuity extras.
    last = last_event(project_root)
    if last:
        decision = last.get("decision", "?")
        claim = (last.get("claim") or "")[:120]
        gaps = last.get("blocking_gaps", 0)
        parts.append(
            f"Where you left off: last stop-gate was `{decision}`"
            + (f" with {gaps} blocking gap(s)" if gaps else "")
            + (f"; claim: {claim!r}" if claim else "")
            + "."
        )

    transcript_path = payload.get("transcript_path")
    if transcript_path:
        tpath = Path(str(transcript_path))
        sentence = last_assistant_sentence(tpath)
        if sentence:
            parts.append(f"Last assistant sentence: {sentence[:200]}")
        if ends_on_open_question(tpath):
            parts.append("Session appears to have ended on an open question.")

    try:
        task_id = active_task_id(project_root)
        if task_id:
            from devcouncil.storage.db import get_db
            from devcouncil.storage.repositories import GapRepository
            from devcouncil.verification.next_actions import split_next_actions

            db = get_db(project_root)
            if db:
                with db.get_session() as session:
                    gaps = [g for g in GapRepository(session).get_for_task(task_id) if g.blocking]
                if gaps:
                    actions, _ = split_next_actions(gaps)
                    top = [a.action for a in actions[:2]]
                    if top:
                        parts.append("Blocking gaps on active task: " + "; ".join(top))
    except Exception:
        logger.debug("session_briefing task gaps failed", exc_info=True)

    if not parts:
        return None
    return "DevCouncil continuity — " + " ".join(parts)


COMPACT_SNAPSHOT_REL = Path(".devcouncil") / "state" / "compact_snapshot.json"
LAST_COMPACT_BRIEF_REL = Path(".devcouncil") / "state" / "last_compact_brief.json"


def compact_snapshot_path(project_root: Path) -> Path:
    return project_root / COMPACT_SNAPSHOT_REL


def last_compact_brief_path(project_root: Path) -> Path:
    return project_root / LAST_COMPACT_BRIEF_REL


def _project_phase(project_root: Path) -> str | None:
    try:
        from devcouncil.app.project_status import compute_phase
        from devcouncil.storage.db import get_db
        from devcouncil.storage.repositories import ArtifactGraphRepository, StateRepository

        db = get_db(project_root)
        if not db:
            return None
        with db.get_session() as session:
            graph = ArtifactGraphRepository(session).load_graph()
            state = StateRepository(session).get_state()
            return compute_phase(graph, state.current_phase if state else None)
    except Exception:
        logger.debug("_project_phase failed", exc_info=True)
        return None


def _task_blocking_summary(project_root: Path, task_id: str | None) -> tuple[int, list[str]]:
    if not task_id:
        return 0, []
    try:
        from devcouncil.storage.db import get_db
        from devcouncil.storage.repositories import GapRepository
        from devcouncil.verification.next_actions import split_next_actions

        db = get_db(project_root)
        if not db:
            return 0, []
        with db.get_session() as session:
            gaps = [g for g in GapRepository(session).get_for_task(task_id) if g.blocking]
        if not gaps:
            return 0, []
        actions, _ = split_next_actions(gaps)
        return len(gaps), [a.action for a in actions[:2]]
    except Exception:
        logger.debug("_task_blocking_summary failed", exc_info=True)
        return 0, []


def build_compact_snapshot(project_root: Path, payload: dict[str, Any] | None = None) -> dict[str, Any]:
    """Gather DevCouncil state for PreCompact disk snapshot."""
    payload = payload if isinstance(payload, dict) else {}
    snapshot: dict[str, Any] = {
        "ts": time.time(),
        "session_id": payload.get("session_id"),
    }
    task_id = active_task_id(project_root)
    if task_id:
        snapshot["task_id"] = task_id
    phase = _project_phase(project_root)
    if phase:
        snapshot["phase"] = phase
    blocking, next_actions = _task_blocking_summary(project_root, task_id)
    snapshot["blocking_gaps"] = blocking
    if next_actions:
        snapshot["next_actions"] = next_actions

    last = last_event(project_root)
    if last:
        snapshot["last_stop_gate"] = {
            "decision": last.get("decision"),
            "claim": (last.get("claim") or "")[:120],
            "blocking_gaps": last.get("blocking_gaps", 0),
        }

    transcript_path = payload.get("transcript_path")
    if transcript_path:
        sentence = last_assistant_sentence(Path(str(transcript_path)))
        if sentence:
            snapshot["last_assistant_sentence"] = sentence[:200]

    return snapshot


def write_compact_snapshot(project_root: Path, payload: dict[str, Any] | None = None) -> None:
    from devcouncil.utils.json_persist import write_json

    path = compact_snapshot_path(project_root)
    path.parent.mkdir(parents=True, exist_ok=True)
    write_json(path, build_compact_snapshot(project_root, payload))


def read_compact_snapshot(project_root: Path) -> dict[str, Any] | None:
    try:
        from devcouncil.utils.json_persist import read_json

        data = read_json(compact_snapshot_path(project_root))
        return data if isinstance(data, dict) else None
    except (FileNotFoundError, OSError, ValueError):
        return None


def record_compact_brief(project_root: Path, session_id: str | None = None) -> None:
    from devcouncil.utils.json_persist import write_json

    path = last_compact_brief_path(project_root)
    path.parent.mkdir(parents=True, exist_ok=True)
    write_json(path, {"ts": time.time(), "session_id": session_id})


def recent_compact_brief(project_root: Path, within_seconds: int) -> bool:
    if within_seconds <= 0:
        return False
    now = time.time()
    try:
        from devcouncil.utils.json_persist import read_json

        data = read_json(last_compact_brief_path(project_root))
        if isinstance(data, dict):
            ts = data.get("ts")
            if isinstance(ts, (int, float)) and (now - ts) <= within_seconds:
                return True
    except (FileNotFoundError, OSError, ValueError):
        pass
    snap = read_compact_snapshot(project_root)
    if snap:
        ts = snap.get("ts")
        if isinstance(ts, (int, float)) and (now - ts) <= within_seconds:
            return True
    return False


def compact_briefing(project_root: Path, payload: dict[str, Any] | None = None) -> str | None:
    """Slim SessionStart additionalContext after context compaction (~1–2k chars)."""
    payload = payload if isinstance(payload, dict) else {}
    snapshot = read_compact_snapshot(project_root)
    parts: list[str] = []

    task_id: str | None = None
    blocking = 0
    next_actions: list[str] = []
    phase: str | None = None

    if snapshot:
        raw_task = snapshot.get("task_id")
        task_id = str(raw_task) if raw_task else active_task_id(project_root)
        blocking = int(snapshot.get("blocking_gaps") or 0)
        next_actions = [str(a) for a in (snapshot.get("next_actions") or [])[:2]]
        raw_phase = snapshot.get("phase")
        phase = str(raw_phase) if raw_phase else None
        last_sg = snapshot.get("last_stop_gate")
        if isinstance(last_sg, dict):
            decision = last_sg.get("decision", "?")
            sg_gaps = int(last_sg.get("blocking_gaps") or blocking or 0)
            claim = (last_sg.get("claim") or "")[:120]
            parts.append(
                f"Last stop-gate: `{decision}`"
                + (f" ({sg_gaps} blocking gap(s))" if sg_gaps else "")
                + (f"; claim: {claim!r}" if claim else "")
                + "."
            )
    else:
        task_id = active_task_id(project_root)
        phase = _project_phase(project_root)
        blocking, next_actions = _task_blocking_summary(project_root, task_id)
        last = last_event(project_root)
        if last:
            decision = last.get("decision", "?")
            claim = (last.get("claim") or "")[:120]
            gaps = int(last.get("blocking_gaps") or blocking or 0)
            parts.append(
                f"Last stop-gate: `{decision}`"
                + (f" ({gaps} blocking gap(s))" if gaps else "")
                + (f"; claim: {claim!r}" if claim else "")
                + "."
            )

    if not task_id:
        task_id = active_task_id(project_root)
    if task_id:
        parts.append(f"Active task: {task_id}.")
    if phase:
        parts.append(f"Phase: {phase}.")
    if blocking:
        parts.append(f"{blocking} blocking gap(s) on active task.")
    if next_actions:
        parts.append("Next: " + "; ".join(next_actions) + ".")
    elif task_id and blocking:
        _, fallback_actions = _task_blocking_summary(project_root, task_id)
        if fallback_actions:
            parts.append("Next: " + "; ".join(fallback_actions) + ".")

    parts.append(
        "Use devcouncil_get_gaps / get_next_actions MCP tools; reconnect MCP if tools are stale."
    )

    if not parts:
        return None
    text = "DevCouncil compact continuity — " + " ".join(parts)
    if len(text) > 2000:
        text = text[:1997] + "..."
    return text


def statusline_tally(project_root: Path, session_id: str | None) -> str | None:
    """Compact shield tally ``🛡 2✓ 1✗`` for the current session."""
    if not session_id:
        return None
    ok, bad = session_tally(project_root, session_id)
    if ok == 0 and bad == 0:
        return None
    return f"🛡 {ok}✓ {bad}✗"
