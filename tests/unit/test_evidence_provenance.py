"""Rank 12 — diff-coverage evidence survives reload, and the provenance trail is readable."""

import json

import pytest

from devcouncil.domain.evidence import DiffCoverageEvidence
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.integrations.mcp.server import call_tool
from devcouncil.storage.db import Database
from devcouncil.storage.native import FileChangeRepository, VerificationRunRepository
from devcouncil.storage.repositories import ArtifactGraphRepository, EvidenceRepository, TaskRepository


@pytest.fixture
def anyio_backend():
    return "asyncio"


def _db(tmp_path):
    dev = tmp_path / ".devcouncil"
    dev.mkdir()
    db = Database(dev / "state.sqlite")
    db.create_db_and_tables()
    return db


def test_diff_coverage_evidence_survives_graph_reload(tmp_path):
    db = _db(tmp_path)
    with db.get_session() as session:
        TaskRepository(session).save(Task(id="TASK-001", title="T", description="D"))
        EvidenceRepository(session).save_diff_coverage_evidence(DiffCoverageEvidence(
            task_id="TASK-001", tool="coverage.py", measured=True,
            changed_lines=10, covered_lines=4, coverage_ratio=0.4,
            summary="4/10 changed lines exercised",
        ))
    # Reloading the graph must re-attach the coverage proof (previously dropped).
    with db.get_session() as session:
        graph = ArtifactGraphRepository(session).load_graph()
    assert len(graph.diff_coverage_evidence) == 1
    assert graph.diff_coverage_evidence[0].covered_lines == 4
    summary = graph.coverage_summary()
    assert summary["diff_coverage_runs"] == 1
    assert summary["unexercised_diff_findings"] == 1  # 4/10 < full


@pytest.mark.anyio
async def test_get_task_provenance_returns_audit_trail(tmp_path, monkeypatch):
    db = _db(tmp_path)
    with db.get_session() as session:
        TaskRepository(session).save(Task(
            id="TASK-001", title="T", description="D",
            planned_files=[PlannedFile(path="src/a.py", reason="x", allowed_change="modify")],
        ))
        FileChangeRepository(session).record("src/a.py", "write", True, task_id="TASK-001", reason="policy allowed")
        FileChangeRepository(session).record("src/evil.py", "write", False, task_id="TASK-001", reason="out of scope")
        VerificationRunRepository(session).save("TASK-001", "local", {}, [{"cmd": "pytest"}], "verified")
        EvidenceRepository(session).save_diff_coverage_evidence(DiffCoverageEvidence(
            task_id="TASK-001", tool="coverage.py", measured=True, changed_lines=2, covered_lines=2,
            coverage_ratio=1.0, summary="all exercised",
        ))
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    payload = json.loads((await call_tool("devcouncil_get_task_provenance", {"task_id": "TASK-001"}))[0].text)

    assert payload["ok"] is True
    paths = {(fc["path"], fc["allowed"]) for fc in payload["file_changes"]}
    assert ("src/a.py", True) in paths
    assert ("src/evil.py", False) in paths  # rejected writes are recorded too
    assert len(payload["verification_runs"]) == 1
    assert payload["verification_runs"][0]["status"] == "verified"
    assert len(payload["diff_coverage"]) == 1
    assert payload["diff_coverage"][0]["covered_lines"] == 2
