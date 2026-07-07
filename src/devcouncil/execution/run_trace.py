"""Shepherd-style reversible run traces: one view over a run's events + checkpoints.

DevCouncil already records the three ingredients of a reversible agent run:

* **manifests** — ``.devcouncil/runs/<run-id>/agent-run.json`` (executor, status, task);
* **trace events** — ``.devcouncil/logs/traces.jsonl`` (:mod:`devcouncil.telemetry.traces`);
* **git checkpoints** — before/after/attempt refs + patches
  (:class:`devcouncil.execution.checkpoints.CheckpointService`).

What was missing — and what Shepherd (shepherd-agents/shepherd) argues for — is treating
the run itself as an object another agent can operate on: inspect the full timeline,
see exactly what the run changed, and revert it, behind one programming model. This
module joins the three stores into a :class:`RunTimeline` and provides the meta-agent
operations on top: :func:`revert_run` (reverse the run's workspace effects via its git
checkpoints) and :func:`supervise_run` (an LLM meta-agent that reviews the timeline +
diff and issues a keep/revert/repair verdict, degrading to deterministic heuristics
when no model is configured).
"""

from __future__ import annotations

import json
import logging
from pathlib import Path
from typing import TYPE_CHECKING, Literal, Optional

from pydantic import BaseModel, Field

from devcouncil.execution.checkpoints import CheckpointResult, CheckpointService
from devcouncil.telemetry.traces import TraceEvent, TraceLogger, read_trace_events
from devcouncil.utils.json_persist import read_json

if TYPE_CHECKING:
    from devcouncil.llm.router import ModelRouter

logger = logging.getLogger(__name__)

# Event types that indicate the run itself went wrong (not merely that work remains).
_FAILURE_EVENT_TYPES = frozenset({
    "llm_structured_parse_repair_failed",
    "llm_provider_request_failed_fallback",
    "task_failed",
    "verification_failed",
})

_SUPERVISOR_SYSTEM = (
    "You are a supervisor meta-agent reviewing one recorded run of a coding agent. "
    "You are given the run manifest, its trace-event timeline, and the workspace diff "
    "the run produced. Decide whether its changes should be kept, reverted, or "
    "repaired (kept but with follow-up fixes). Judge only from the evidence given; "
    "an exit code of 0 with a plausible diff is normally 'keep'. Recommend 'revert' "
    "only when the evidence shows the run failed, was cut off, or made changes that "
    "contradict its task."
)


class RunCheckpoint(BaseModel):
    """One git checkpoint ref recorded for the run's task."""

    stage: str  # before | after | attempt-N
    ref: str
    sha: str = ""
    patch_path: str = ""


class RunTimeline(BaseModel):
    """A run as an inspectable object: manifest + events + checkpoints + diff."""

    run_id: str = ""
    task_id: str = ""
    manifest: dict = Field(default_factory=dict)
    events: list[TraceEvent] = Field(default_factory=list)
    checkpoints: list[RunCheckpoint] = Field(default_factory=list)
    diff_stat: str = ""
    reversible: bool = False

    @property
    def status(self) -> str:
        return str(self.manifest.get("status") or "")

    @property
    def returncode(self) -> Optional[int]:
        rc = self.manifest.get("returncode")
        return rc if isinstance(rc, int) else None


class SupervisorVerdict(BaseModel):
    """The supervisor meta-agent's decision about a run."""

    verdict: Literal["keep", "revert", "repair"] = "keep"
    confidence: float = Field(0.5, ge=0.0, le=1.0)
    rationale: str = ""
    findings: list[str] = Field(default_factory=list)
    source: str = "heuristic"  # heuristic | model


# --- Resolution -------------------------------------------------------------------


def _runs_dir(project_root: Path) -> Path:
    return project_root / ".devcouncil" / "runs"


def _load_manifest(project_root: Path, run_id: str) -> dict:
    path = _runs_dir(project_root) / run_id / "agent-run.json"
    try:
        data = read_json(path)
        return data if isinstance(data, dict) else {}
    except (OSError, json.JSONDecodeError):
        return {}


def resolve_run(project_root: Path, ref: str) -> tuple[str, str]:
    """Resolve ``ref`` (a run id or a task id) to ``(run_id, task_id)``.

    A run id is matched by manifest directory; a task id is matched against the
    newest run manifest recorded for that task (empty run_id when the task only has
    checkpoints and no recorded run). Either element may be empty, but not both.
    """
    ref = ref.strip()
    if not ref:
        raise ValueError("Empty run/task reference.")

    manifest = _load_manifest(project_root, ref)
    if manifest:
        return ref, str(manifest.get("task_id") or "")

    # Treat as a task id: newest manifest that claims it.
    runs_dir = _runs_dir(project_root)
    candidates: list[tuple[float, str]] = []
    if runs_dir.is_dir():
        for manifest_path in runs_dir.glob("*/agent-run.json"):
            data = _load_manifest(project_root, manifest_path.parent.name)
            if str(data.get("task_id") or "") == ref:
                try:
                    mtime = manifest_path.stat().st_mtime
                except OSError:
                    mtime = 0.0
                candidates.append((mtime, manifest_path.parent.name))
    if candidates:
        candidates.sort(reverse=True)
        return candidates[0][1], ref

    # No manifest at all — accept a bare task id if it has checkpoints, so runs made
    # before manifests existed (or by native executors) are still addressable.
    service = CheckpointService(project_root)
    if service._ref_exists(CheckpointService.REF_BEFORE.format(task_id=ref)) or (
        service.checkpoint_dir / f"{ref}-after.patch"
    ).exists():
        return "", ref
    raise ValueError(f"No run manifest or checkpoints found for {ref!r}.")


# --- Timeline assembly --------------------------------------------------------------


def _git_lines(project_root: Path, *args: str) -> str:
    from devcouncil.utils.proc import git_output

    return git_output(list(args), cwd=project_root, default="").strip()


def _collect_checkpoints(project_root: Path, task_id: str) -> list[RunCheckpoint]:
    if not task_id:
        return []
    service = CheckpointService(project_root)
    checkpoints: list[RunCheckpoint] = []

    def add(stage: str, ref: str, patch_name: str = "") -> None:
        sha = _git_lines(project_root, "rev-parse", "--verify", ref)
        patch = service.checkpoint_dir / patch_name if patch_name else None
        if sha or (patch is not None and patch.exists()):
            checkpoints.append(RunCheckpoint(
                stage=stage,
                ref=ref,
                sha=sha,
                patch_path=str(patch) if patch is not None and patch.exists() else "",
            ))

    add("before", CheckpointService.REF_BEFORE.format(task_id=task_id), f"{task_id}-before.patch")
    add("after", CheckpointService.REF_AFTER.format(task_id=task_id), f"{task_id}-after.patch")

    # Attempt refs live under refs/devcouncil/tasks/<task>/attempts/<n>.
    prefix = f"refs/devcouncil/tasks/{task_id}/attempts/"
    listing = _git_lines(project_root, "for-each-ref", "--format=%(refname) %(objectname)", prefix)
    for line in listing.splitlines():
        parts = line.split()
        if len(parts) == 2 and parts[0].startswith(prefix):
            checkpoints.append(RunCheckpoint(
                stage=f"attempt-{parts[0][len(prefix):]}", ref=parts[0], sha=parts[1]
            ))
    return checkpoints


def diff_run(project_root: Path, task_id: str, *, stat_only: bool = False) -> str:
    """The workspace changes a run produced: ``git diff before..after`` when both refs
    exist, falling back to the recorded after-patch."""
    if not task_id:
        return ""
    before = CheckpointService.REF_BEFORE.format(task_id=task_id)
    after = CheckpointService.REF_AFTER.format(task_id=task_id)
    if _git_lines(project_root, "rev-parse", "--verify", before) and _git_lines(
        project_root, "rev-parse", "--verify", after
    ):
        args = ["diff", "--stat", before, after] if stat_only else ["diff", before, after]
        return _git_lines(project_root, *args)
    patch = project_root / ".devcouncil" / "checkpoints" / f"{task_id}-after.patch"
    try:
        content = patch.read_text(encoding="utf-8")
    except OSError:
        return ""
    if not stat_only:
        return content
    # Cheap stat for a raw patch: count per-file hunks.
    files = [line.split(" b/", 1)[-1] for line in content.splitlines() if line.startswith("diff --git ")]
    return "\n".join(files)


def load_timeline(project_root: Path, ref: str, *, event_limit: int = 200) -> RunTimeline:
    """Assemble the full reversible-trace view for a run (or bare task) reference."""
    run_id, task_id = resolve_run(project_root, ref)
    manifest = _load_manifest(project_root, run_id) if run_id else {}
    if not task_id:
        task_id = str(manifest.get("task_id") or "")

    events = [
        e for e in read_trace_events(project_root)
        if (run_id and e.run_id == run_id) or (task_id and e.task_id == task_id)
    ]
    if event_limit > 0:
        events = events[-event_limit:]

    checkpoints = _collect_checkpoints(project_root, task_id)
    reversible = any(c.stage in ("before", "after") for c in checkpoints)
    return RunTimeline(
        run_id=run_id,
        task_id=task_id,
        manifest=manifest,
        events=events,
        checkpoints=checkpoints,
        diff_stat=diff_run(project_root, task_id, stat_only=True),
        reversible=reversible,
    )


# --- Meta-agent operations -----------------------------------------------------------


def revert_run(project_root: Path, ref: str) -> CheckpointResult:
    """Reverse a run's workspace effects using its task's git checkpoints.

    The revert itself is recorded as a trace event, so the trace stays a faithful,
    append-only history of everything that happened — including supervision.
    """
    run_id, task_id = resolve_run(project_root, ref)
    if not task_id:
        raise ValueError(f"Run {ref!r} has no task id; nothing to revert.")
    result = CheckpointService(project_root).rollback(task_id)
    reverted = result.git_ref_created or bool(result.patch_path and "Rolled back" in result.message)
    TraceLogger(project_root).log_event(
        "run_reverted" if reverted else "run_revert_failed",
        {"reference": ref, "message": result.message},
        run_id=run_id or None,
        task_id=task_id,
        summary=f"Supervisor revert of {task_id}: {result.message}",
    )
    return result


def heuristic_verdict(timeline: RunTimeline) -> SupervisorVerdict:
    """Deterministic verdict from the recorded evidence (no model required)."""
    findings: list[str] = []
    rc = timeline.returncode
    if rc not in (None, 0):
        findings.append(f"Executor exited with returncode {rc}.")
    if timeline.status == "running":
        findings.append("Run is still marked 'running' (possibly orphaned).")
    failures = [e for e in timeline.events if e.type in _FAILURE_EVENT_TYPES]
    for event in failures[:5]:
        findings.append(f"Failure event: {event.type} — {event.summary or '(no summary)'}")
    if not timeline.diff_stat:
        findings.append("No workspace diff recorded for this run.")

    if rc not in (None, 0) and timeline.diff_stat:
        verdict: Literal["keep", "revert", "repair"] = "revert"
        rationale = "The executor failed after modifying the workspace; its changes are unverified."
    elif failures:
        verdict = "repair"
        rationale = "The run completed but recorded failure events; keep the changes and fix forward."
    else:
        verdict = "keep"
        rationale = "No failure evidence recorded for this run."
    return SupervisorVerdict(
        verdict=verdict,
        confidence=0.4 if findings else 0.6,
        rationale=rationale,
        findings=findings,
        source="heuristic",
    )


async def supervise_run(
    project_root: Path,
    timeline: RunTimeline,
    router: "Optional[ModelRouter]" = None,
    *,
    diff_chars: int = 12_000,
) -> SupervisorVerdict:
    """Review a run and produce a keep/revert/repair verdict.

    With a router, the ``run_supervisor`` role reviews the manifest, a bounded event
    timeline, and the run's diff; the deterministic heuristic verdict is both the
    degradation fallback and part of the prompt (so the model starts from the
    recorded evidence rather than re-deriving it).
    """
    fallback = heuristic_verdict(timeline)
    if router is None:
        return fallback

    events_payload = [
        {"type": e.type, "summary": e.summary, "timestamp": e.timestamp}
        for e in timeline.events[-60:]
    ]
    diff = diff_run(project_root, timeline.task_id)[:diff_chars]
    payload = {
        "manifest": timeline.manifest,
        "heuristic_findings": fallback.findings,
        "events": events_payload,
        "diff_stat": timeline.diff_stat,
        "diff_excerpt": diff,
        "reversible": timeline.reversible,
    }
    messages = [
        {"role": "system", "content": _SUPERVISOR_SYSTEM},
        {"role": "user", "content": f"Run evidence (JSON):\n{json.dumps(payload, indent=2, default=str)}"},
    ]
    try:
        verdict = await router.complete_structured(
            "run_supervisor", messages, SupervisorVerdict, fallback=fallback,
            run_id=timeline.run_id or None,
        )
    except Exception as exc:
        # A router without a run_supervisor role (or any other model failure) must
        # degrade to the heuristic verdict, never break supervision.
        logger.warning("Model supervision unavailable; using heuristic verdict: %s", exc)
        verdict = fallback
    if verdict is not fallback:
        verdict.source = "model"
    TraceLogger(project_root).log_event(
        "run_supervised",
        {"verdict": verdict.verdict, "confidence": verdict.confidence, "source": verdict.source},
        run_id=timeline.run_id or None,
        task_id=timeline.task_id or None,
        summary=f"Supervisor verdict for {timeline.task_id or timeline.run_id}: {verdict.verdict}",
    )
    return verdict
