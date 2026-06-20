"""Correction manifest generation for repair loops."""

from __future__ import annotations

import json
import uuid
from datetime import datetime, timezone
from pathlib import Path

from pydantic import BaseModel, Field

from devcouncil.app.config import load_config
from devcouncil.domain.gap import Gap
from devcouncil.domain.task import Task
from devcouncil.storage.db import get_db
from devcouncil.storage.native import CorrectionManifestRepository
from devcouncil.storage.repositories import EvidenceRepository, GapRepository, TaskRepository
from devcouncil.utils.redaction import redact_text

# Bounds for the prior-attempt context folded into the manifest. These reach the
# next executor's prompt verbatim, so they must stay small enough not to crowd out
# the task spec / blow the context window while still carrying the signal the agent
# needs (what it changed last time, and why verification rejected it).
_MAX_PRIOR_DIFF_CHARS = 8000
_MAX_FAILING_OUTPUT_CHARS = 4000
# Per failed command, how much of the captured stdout/stderr tail to keep. Test
# runners put the actual assertion/traceback at the end, so we keep the tail.
_MAX_PER_COMMAND_OUTPUT_CHARS = 1500


class CorrectionManifest(BaseModel):
    task_id: str
    root_cause: str
    failed_evidence: list[str] = Field(default_factory=list)
    allowed_repair_files: list[str] = Field(default_factory=list)
    forbidden_changes: list[str] = Field(default_factory=list)
    commands_to_rerun: list[str] = Field(default_factory=list)
    prior_failed_attempts: int = 0
    retry_budget: int = 3
    executor_recommendation: str = "manual"
    created_at: str
    # Blocking gaps ordered most-actionable-first (severity, then gap-type priority),
    # so the repair loop is steered at the real defect (a failing test) rather than an
    # arbitrary first gap (e.g. an orphan_diff). The first entry is the root_cause.
    ordered_blocking_gaps: list[str] = Field(default_factory=list)
    # Prior-attempt context (optional, backward-compatible). Without these the repair
    # executor only sees the root_cause text and re-derives the same wrong approach
    # blind. ``prior_diff`` is what the previous attempt actually changed; it lets the
    # agent see (and stop re-applying) its rejected edit. ``failing_output`` is the
    # captured failing test / verification output that explains *why* it was rejected.
    # Both are redacted and size-bounded before being written.
    prior_diff: str = ""
    failing_output: str = ""


# Severity ordering: most severe first.
_SEVERITY_RANK = {"critical": 0, "high": 1, "medium": 2, "low": 3}

# Gap-type priority within a severity band. Lower sorts first. Executable-evidence
# failures (a failing test / unproven acceptance criterion) are the real defect signal
# and must outrank scope (orphan/dependency) and advisory (review/secret) gaps so the
# repair loop targets the failing test, not an orphan_diff.
_GAP_TYPE_PRIORITY = {
    "test_failed": 0,
    "acceptance_criteria_unproven": 1,
    "diff_not_exercised": 1,
    "task_not_implemented": 2,
    "migration_gap": 2,
    "orphan_diff": 3,
    "planned_file_not_changed": 3,
    "dependency_risk": 3,
    "architecture_drift": 4,
    "assumption_violated": 4,
    "security_risk": 5,
}


def _ordered_blocking_gaps(blocking_gaps: list[Gap]) -> list[Gap]:
    """Stable-sort blocking gaps by (severity, gap-type priority).

    ``test_failed`` / ``acceptance_*`` gaps come before orphan/dependency before
    review/secret, so the picked root_cause is the failing behavior rather than an
    incidental scope finding. Unknown severities/types sort last (defensive)."""
    return sorted(
        blocking_gaps,
        key=lambda g: (
            _SEVERITY_RANK.get(g.severity, 9),
            _GAP_TYPE_PRIORITY.get(g.gap_type, 9),
        ),
    )


def _latest_agent_run(project_root: Path, task_id: str) -> dict | None:
    runs_dir = project_root / ".devcouncil" / "runs"
    if not runs_dir.exists():
        return None
    candidates = sorted(runs_dir.glob("*/agent-run.json"), reverse=True)
    for path in candidates:
        try:
            payload = json.loads(path.read_text(encoding="utf-8"))
        except Exception:
            continue
        if payload.get("task_id") == task_id:
            return payload
    return None


def _truncate_tail(text: str, limit: int) -> str:
    """Keep the last ``limit`` chars of ``text`` (the actionable tail of test output),
    prefixing a marker when truncated. Empty/whitespace input returns ""."""
    text = (text or "").strip()
    if len(text) <= limit:
        return text
    return "[devcouncil: output truncated, showing last "f"{limit} chars]\n" + text[-limit:]


def _truncate_head(text: str, limit: int) -> str:
    """Keep the first ``limit`` chars of ``text`` (diffs read top-down), with a marker
    when truncated. Empty/whitespace input returns ""."""
    text = (text or "").strip()
    if len(text) <= limit:
        return text
    return text[:limit] + "\n[devcouncil: diff truncated, "f"{len(text) - limit} chars omitted]"


def _read_text_tail(path: Path, limit: int) -> str:
    """Best-effort read of a captured stdout/stderr file, keeping its tail. Never raises."""
    try:
        if not path.is_file():
            return ""
        return _truncate_tail(path.read_text(encoding="utf-8", errors="replace"), limit)
    except Exception:
        return ""


def _collect_prior_diff(project_root: Path, task_id: str) -> str:
    """The prior attempt's working-tree diff, from the task's ``after`` checkpoint patch.

    The checkpoint service writes ``<task_id>-after.patch`` after each executor run, so
    this is exactly what the previous attempt changed. Redacted and head-bounded so the
    repair executor can see (and avoid re-applying) its rejected edit without the diff
    swamping the prompt. Returns "" when no patch exists (e.g. first attempt)."""
    patch_path = project_root / ".devcouncil" / "checkpoints" / f"{task_id}-after.patch"
    try:
        if not patch_path.is_file():
            return ""
        raw = patch_path.read_text(encoding="utf-8", errors="replace")
    except Exception:
        return ""
    return _truncate_head(redact_text(raw), _MAX_PRIOR_DIFF_CHARS)


def _collect_failing_output(project_root: Path, failed_results) -> str:
    """The captured stdout/stderr of the failing verification commands.

    Folds each failed command's summary plus the tail of its captured stdout/stderr so
    the repair executor sees *why* it was rejected (the actual assertion / traceback),
    not just that a command exited non-zero. Redacted and size-bounded. Returns ""
    when there is nothing useful to show."""
    blocks: list[str] = []
    for result in failed_results:
        parts = [f"$ {result.command} (exit {result.exit_code})"]
        if result.summary and result.summary.strip():
            parts.append(result.summary.strip())
        for label, rel in (("stdout", result.stdout_path), ("stderr", result.stderr_path)):
            if not rel:
                continue
            path = Path(rel)
            if not path.is_absolute():
                path = project_root / rel
            tail = _read_text_tail(path, _MAX_PER_COMMAND_OUTPUT_CHARS)
            if tail:
                parts.append(f"--- {label} ---\n{tail}")
        blocks.append("\n".join(parts))
    if not blocks:
        return ""
    return _truncate_tail(redact_text("\n\n".join(blocks)), _MAX_FAILING_OUTPUT_CHARS)


def build_correction_manifest(
    project_root: Path,
    task: Task,
    blocking_gaps: list[Gap],
    *,
    repair_service=None,
    prior_attempts: int = 0,
) -> CorrectionManifest:
    config = load_config(project_root)
    failed: list[str] = []
    failed_results: list = []
    db = get_db(project_root)
    if db:
        with db.get_session() as session:
            # Scope failed evidence to THIS task. Scanning every evidence row made a
            # repair for one task chase unrelated failures from another, so the loop
            # never converged on the real defect.
            for result in EvidenceRepository(session).get_command_results_for_task(task.id):
                if result.exit_code != 0:
                    failed.append(f"{result.command} (exit {result.exit_code})")
                    failed_results.append(result)

    # Steer the repair at the most actionable failure (a failing test / unproven AC),
    # not an arbitrary first gap such as an orphan_diff.
    ordered_gaps = _ordered_blocking_gaps(blocking_gaps)
    root_cause = ordered_gaps[0].description if ordered_gaps else "Unknown failure"
    manifest = CorrectionManifest(
        task_id=task.id,
        root_cause=root_cause,
        ordered_blocking_gaps=[g.description for g in ordered_gaps],
        failed_evidence=failed,
        allowed_repair_files=[pf.path for pf in task.planned_files],
        forbidden_changes=list(task.forbidden_changes),
        commands_to_rerun=task.expected_tests or task.allowed_commands,
        # The number of repair attempts already made on this task — real, not a
        # hardcoded 0. The agent sees how much of its budget is spent so it knows
        # when to change approach rather than retry the same fix.
        prior_failed_attempts=prior_attempts,
        retry_budget=config.execution.max_repair_attempts,
        executor_recommendation=config.execution.default_executor,
        created_at=datetime.now(timezone.utc).isoformat(),
        # Prior-attempt context so the next executor repairs against what actually
        # happened (its rejected diff + the failing output) instead of re-deriving
        # the same wrong approach blind. Both are redacted and size-bounded.
        prior_diff=_collect_prior_diff(project_root, task.id),
        failing_output=_collect_failing_output(project_root, failed_results),
    )

    if repair_service is not None:
        try:
            import asyncio

            plan = asyncio.run(repair_service.generate_repair_plan(blocking_gaps, task.description))
            if plan.suggested_tasks:
                suggested = plan.suggested_tasks[0]
                manifest.root_cause = suggested.description or manifest.root_cause
                # Use the repair plan's concrete scope instead of throwing it away:
                # union its targeted files/tests with the task's so the re-implement
                # step focuses on what actually needs fixing without losing task scope.
                manifest.allowed_repair_files = _union(
                    manifest.allowed_repair_files, [pf.path for pf in suggested.planned_files]
                )
                manifest.commands_to_rerun = _union(manifest.commands_to_rerun, suggested.expected_tests)
        except Exception:
            pass
    return manifest


def _union(base: list[str], extra: list[str]) -> list[str]:
    """Append items from ``extra`` not already in ``base`` (order-preserving dedupe)."""
    merged = list(base)
    for item in extra:
        if item and item not in merged:
            merged.append(item)
    return merged


def write_correction_manifest(project_root: Path, task_id: str, *, repair_service=None) -> Path | None:
    db = get_db(project_root)
    if not db:
        return None
    with db.get_session() as session:
        task = TaskRepository(session).get_by_id(task_id)
        if not task:
            return None
        gaps = [g for g in GapRepository(session).get_all() if g.task_id == task_id and g.blocking]
        if not gaps:
            return None
        prior_record = CorrectionManifestRepository(session).latest_for_task(task_id)
        prior_attempts = (prior_record.attempt + 1) if prior_record else 1

    manifest = build_correction_manifest(
        project_root, task, gaps, repair_service=repair_service, prior_attempts=prior_attempts
    )
    run_id = str(uuid.uuid4())
    run_dir = project_root / ".devcouncil" / "runs" / run_id
    run_dir.mkdir(parents=True, exist_ok=True)
    path = run_dir / "correction-manifest.json"
    path.write_text(manifest.model_dump_json(indent=2), encoding="utf-8")

    with db.get_session() as session:
        CorrectionManifestRepository(session).save(
            task_id,
            str(path),
            "open",
            run_id=run_id,
            retry_budget=manifest.retry_budget,
            attempt=manifest.prior_failed_attempts,
        )
    return path


def load_latest_correction_manifest(project_root: Path, task_id: str) -> CorrectionManifest | None:
    db = get_db(project_root)
    if not db:
        return None
    with db.get_session() as session:
        record = CorrectionManifestRepository(session).latest_for_task(task_id)
        if not record:
            return None
        path = Path(record.manifest_path)
        if not path.exists():
            return None
        return CorrectionManifest.model_validate(json.loads(path.read_text(encoding="utf-8")))
