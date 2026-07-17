"""Verify-time stale map refresh tests."""

from __future__ import annotations

import json
import subprocess

from devcouncil.domain.requirement import AcceptanceCriterion, Requirement
from devcouncil.domain.task import Task
from devcouncil.indexing.repo_mapper import RepoMapper
from devcouncil.verification.verifier import Verifier


def _git(root, *args):
    subprocess.run(["git", *args], cwd=root, check=True, capture_output=True, text=True)


def test_verify_refreshes_stale_map_before_gates(tmp_path, monkeypatch):
    (tmp_path / "pkg").mkdir()
    (tmp_path / "pkg" / "a.py").write_text("x = 1\n", encoding="utf-8")
    _git(tmp_path, "init")
    _git(tmp_path, "-c", "user.email=t@t", "-c", "user.name=t", "add", "-A")
    _git(tmp_path, "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", "init")

    map_path = tmp_path / ".devcouncil" / "repo_map.json"
    map_path.parent.mkdir(parents=True, exist_ok=True)
    stale = RepoMapper(tmp_path).map_repo(liveness=False).model_dump()
    stale["generated_head"] = "deadbeef"
    stale["indexed_hash"] = "oldhash"
    map_path.write_text(json.dumps(stale), encoding="utf-8")

    refreshed: list[bool] = []

    def _fake_refresh(project_root, *, on_checkout=True, on_verify=True):
        refreshed.append(on_verify)
        from devcouncil.cli.commands.map import generate_map_artifacts

        generate_map_artifacts(project_root, map_path, quiet=True)
        return True

    monkeypatch.setattr(
        "devcouncil.indexing.map_refresh.refresh_stale_map_if_needed",
        _fake_refresh,
    )

    verifier = Verifier(tmp_path)
    verifier.get_changed_files = lambda: []
    verifier.get_diff = lambda: ""

    import asyncio

    task = Task(id="TASK-1", title="t", description="d")
    req = Requirement(
        id="REQ-1",
        title="r",
        description="d",
        priority="high",
        source="user",
        acceptance_criteria=[
            AcceptanceCriterion(id="AC-1", description="t", verification_method="manual"),
        ],
    )
    asyncio.run(verifier.verify_task(task, [req]))

    assert refreshed == [True]
    fresh = json.loads(map_path.read_text(encoding="utf-8"))
    assert not RepoMapper(tmp_path).map_is_stale(fresh)
