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


def build_correction_manifest(
    project_root: Path,
    task: Task,
    blocking_gaps: list[Gap],
    *,
    repair_service=None,
) -> CorrectionManifest:
    config = load_config(project_root)
    failed: list[str] = []
    db = get_db(project_root)
    if db:
        with db.get_session() as session:
            for ev in EvidenceRepository(session).get_all():
                if hasattr(ev, "command") and getattr(ev, "exit_code", 0) != 0:
                    failed.append(f"{ev.command} (exit {ev.exit_code})")

    root_cause = blocking_gaps[0].description if blocking_gaps else "Unknown failure"
    manifest = CorrectionManifest(
        task_id=task.id,
        root_cause=root_cause,
        failed_evidence=failed,
        allowed_repair_files=[pf.path for pf in task.planned_files],
        forbidden_changes=list(task.forbidden_changes),
        commands_to_rerun=task.expected_tests or task.allowed_commands,
        retry_budget=config.execution.max_repair_attempts,
        executor_recommendation=config.execution.default_executor,
        created_at=datetime.now(timezone.utc).isoformat(),
    )

    if repair_service is not None:
        try:
            import asyncio

            plan = asyncio.run(repair_service.generate_repair_plan(blocking_gaps, task.description))
            if plan.suggested_tasks:
                manifest.root_cause = plan.suggested_tasks[0].description or manifest.root_cause
        except Exception:
            pass
    return manifest


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

    manifest = build_correction_manifest(project_root, task, gaps, repair_service=repair_service)
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
