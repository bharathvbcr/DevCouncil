"""Shepherd-style run traces: resolution, timeline assembly, revert, and supervision."""

import asyncio
import json
import subprocess
from pathlib import Path

import pytest

from devcouncil.execution.checkpoints import CheckpointService
from devcouncil.execution.run_trace import (
    RunTimeline,
    diff_run,
    heuristic_verdict,
    load_timeline,
    resolve_run,
    revert_run,
    supervise_run,
    SupervisorVerdict,
)
from devcouncil.telemetry.traces import TraceLogger


def _init_git_repo(path: Path) -> None:
    subprocess.check_call(["git", "init"], cwd=path)
    subprocess.check_call(["git", "config", "user.email", "test@example.com"], cwd=path)
    subprocess.check_call(["git", "config", "user.name", "Test"], cwd=path)
    (path / "README.md").write_text("hello\n", encoding="utf-8")
    subprocess.check_call(["git", "add", "README.md"], cwd=path)
    subprocess.check_call(["git", "commit", "-m", "init"], cwd=path)


def _write_manifest(root: Path, run_id: str, task_id: str, **extra) -> None:
    run_dir = root / ".devcouncil" / "runs" / run_id
    run_dir.mkdir(parents=True, exist_ok=True)
    manifest = {"run_id": run_id, "task_id": task_id, "status": "finished", "returncode": 0}
    manifest.update(extra)
    (run_dir / "agent-run.json").write_text(json.dumps(manifest), encoding="utf-8")


def _recorded_run(root: Path, run_id: str = "RUN-1", task_id: str = "TASK-1", **extra) -> None:
    """A full recorded run: manifest + before/after checkpoints around a file edit."""
    _init_git_repo(root)
    _write_manifest(root, run_id, task_id, **extra)
    service = CheckpointService(root)
    service.create_before(task_id)
    (root / "README.md").write_text("changed by agent\n", encoding="utf-8")
    service.create_after(task_id)
    TraceLogger(root).log_event(
        "tool_patch_applied", {"paths": ["README.md"]}, run_id=run_id, task_id=task_id,
        summary="Patch applied",
    )


def test_resolve_by_run_id_and_task_id(tmp_path: Path):
    _recorded_run(tmp_path)
    assert resolve_run(tmp_path, "RUN-1") == ("RUN-1", "TASK-1")
    assert resolve_run(tmp_path, "TASK-1") == ("RUN-1", "TASK-1")


def test_resolve_bare_task_with_only_checkpoints(tmp_path: Path):
    _init_git_repo(tmp_path)
    CheckpointService(tmp_path).create_before("TASK-9")
    assert resolve_run(tmp_path, "TASK-9") == ("", "TASK-9")


def test_resolve_unknown_reference_raises(tmp_path: Path):
    with pytest.raises(ValueError):
        resolve_run(tmp_path, "NOPE-1")


def test_timeline_joins_manifest_events_and_checkpoints(tmp_path: Path):
    _recorded_run(tmp_path)
    tl = load_timeline(tmp_path, "RUN-1")
    assert tl.run_id == "RUN-1" and tl.task_id == "TASK-1"
    assert tl.reversible is True
    stages = {c.stage for c in tl.checkpoints}
    assert {"before", "after"} <= stages
    assert any(e.type == "tool_patch_applied" for e in tl.events)
    assert "README.md" in tl.diff_stat
    assert "README.md" in diff_run(tmp_path, "TASK-1")


def test_revert_restores_pre_run_state_and_logs_event(tmp_path: Path):
    _recorded_run(tmp_path)
    result = revert_run(tmp_path, "RUN-1")
    assert "Rolled back" in result.message
    assert "changed by agent" not in (tmp_path / "README.md").read_text(encoding="utf-8")
    # Supervision actions are themselves part of the trace.
    tl = load_timeline(tmp_path, "RUN-1")
    assert any(e.type == "run_reverted" for e in tl.events)


def test_heuristic_verdicts():
    clean = RunTimeline(manifest={"status": "finished", "returncode": 0}, diff_stat="README | 1 +")
    assert heuristic_verdict(clean).verdict == "keep"

    failed = RunTimeline(manifest={"status": "failed", "returncode": 2}, diff_stat="README | 1 +")
    assert heuristic_verdict(failed).verdict == "revert"


def test_supervise_without_router_uses_heuristics(tmp_path: Path):
    _recorded_run(tmp_path, returncode=2, status="failed")
    tl = load_timeline(tmp_path, "RUN-1")
    verdict = asyncio.run(supervise_run(tmp_path, tl, None))
    assert verdict.source == "heuristic"
    assert verdict.verdict == "revert"


def test_supervise_with_router_uses_model_verdict_and_traces_it(tmp_path: Path):
    class FakeRouter:
        async def complete_structured(self, role, messages, schema, fallback=None, **kwargs):
            assert role == "run_supervisor"
            return SupervisorVerdict(verdict="keep", confidence=0.9, rationale="Diff matches task.")

    _recorded_run(tmp_path)
    tl = load_timeline(tmp_path, "RUN-1")
    verdict = asyncio.run(supervise_run(tmp_path, tl, FakeRouter()))
    assert verdict.source == "model"
    assert verdict.verdict == "keep"
    events = load_timeline(tmp_path, "RUN-1").events
    assert any(e.type == "run_supervised" for e in events)
