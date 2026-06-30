"""Native agent handoff manifest builder."""

from __future__ import annotations

import uuid
from datetime import datetime, timezone
from pathlib import Path

from pydantic import BaseModel, Field

from devcouncil.execution.checkpoints import CheckpointService
from devcouncil.storage.db import get_db
from devcouncil.storage.native import AgentHandoffRepository, SemanticDiffRepository, TaskLeaseRepository
from devcouncil.storage.repositories import EvidenceRepository, GapRepository, RequirementRepository, TaskRepository
from devcouncil.verification.verifier import Verifier


class HandoffManifest(BaseModel):
    task: dict
    requirements: list[dict] = Field(default_factory=list)
    planned_files: list[dict] = Field(default_factory=list)
    changed_files: list[str] = Field(default_factory=list)
    semantic_diff: dict | None = None
    checkpoint_refs: dict[str, str] = Field(default_factory=dict)
    command_evidence: list[dict] = Field(default_factory=list)
    open_gaps: list[dict] = Field(default_factory=list)
    from_agent: str
    to_agent: str
    instruction: str = ""
    created_at: str


class HandoffService:
    def __init__(self, project_root: Path):
        self.project_root = project_root.resolve()

    def create(
        self,
        task_id: str,
        from_agent: str,
        to_agent: str,
        *,
        instruction: str = "",
    ) -> tuple[HandoffManifest, Path, str]:
        db = get_db(self.project_root)
        if not db:
            raise RuntimeError("DevCouncil not initialized.")
        run_id = str(uuid.uuid4())
        run_dir = self.project_root / ".devcouncil" / "runs" / run_id
        run_dir.mkdir(parents=True, exist_ok=True)
        manifest_path = run_dir / "handoff.json"

        with db.get_session() as session:
            task = TaskRepository(session).get_by_id(task_id)
            if not task:
                raise ValueError(f"Task {task_id} not found")
            reqs = RequirementRepository(session).get_all()
            gaps = GapRepository(session).get_blocking_for_task(task_id)
            evidence = EvidenceRepository(session).get_command_results_for_task(task_id)
            semantic = SemanticDiffRepository(session).latest_for_task(task_id)

        changed = Verifier(self.project_root).get_task_changed_files(task_id)
        refs = {
            "before": CheckpointService.REF_BEFORE.format(task_id=task_id),
            "after": CheckpointService.REF_AFTER.format(task_id=task_id),
        }
        manifest = HandoffManifest(
            task=task.model_dump(),
            requirements=[r.model_dump() for r in reqs if r.id in task.requirement_ids],
            planned_files=[pf.model_dump() for pf in task.planned_files],
            changed_files=changed,
            semantic_diff={"classifications": semantic.classifications, "summary": semantic.summary} if semantic else None,
            checkpoint_refs=refs,
            command_evidence=[ev.model_dump() for ev in evidence],
            open_gaps=[g.model_dump() for g in gaps],
            from_agent=from_agent,
            to_agent=to_agent,
            instruction=instruction,
            created_at=datetime.now(timezone.utc).isoformat(),
        )
        manifest_path.write_text(manifest.model_dump_json(indent=2), encoding="utf-8")

        status = "manifest_only"
        with db.get_session() as session:
            active = TaskLeaseRepository(session).active_for_task(task_id)
            if active and active.agent == from_agent:
                TaskLeaseRepository(session).release(task_id, active.lease_token)
                TaskLeaseRepository(session).acquire(
                    task_id,
                    owner=f"handoff:{to_agent}",
                    agent=to_agent,
                )
                status = "lease_transferred"
            AgentHandoffRepository(session).save(
                task_id,
                from_agent,
                to_agent,
                run_id,
                str(manifest_path),
                status,
            )
        return manifest, manifest_path, run_id
