"""Task audit-trail payload shared by CLI and MCP."""

from __future__ import annotations

from pathlib import Path

from devcouncil.cli.commands.init import initialize_project
from devcouncil.domain.evidence import DiffCoverageEvidence
from devcouncil.storage.db import get_db
from devcouncil.storage.native import (
    CorrectionManifestRepository,
    FileChangeRepository,
    VerificationRunRepository,
)
from devcouncil.storage.repositories import EvidenceRepository


def task_provenance_payload(project_root: Path, task_id: str) -> dict:
    """Build the provenance audit trail for one task."""
    initialize_project(project_root, quiet=True)
    db = get_db(project_root)
    if not db:
        return {"ok": False, "error": "DevCouncil state is unavailable in this directory.", "task_id": task_id}

    with db.get_session() as session:
        file_changes = [r.model_dump() for r in FileChangeRepository(session).list_for_task(task_id)]
        verification_runs = [r.model_dump() for r in VerificationRunRepository(session).list_for_task(task_id)]
        coverage = [
            ev.model_dump()
            for ev in EvidenceRepository(session).get_all()
            if isinstance(ev, DiffCoverageEvidence) and ev.task_id == task_id
        ]
        correction_manifest = CorrectionManifestRepository(session).latest_for_task(task_id)

    return {
        "ok": True,
        "task_id": task_id,
        "file_changes": file_changes,
        "verification_runs": verification_runs,
        "diff_coverage": coverage,
        "latest_correction_manifest": correction_manifest.model_dump() if correction_manifest else None,
    }
