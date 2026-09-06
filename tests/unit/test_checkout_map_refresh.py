"""Checkout refreshes a stale repo map before the liveness baseline snapshot."""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

from devcouncil.cli.commands.init import initialize_project
from devcouncil.domain.task import Task
from devcouncil.execution.lease_ops import _refresh_stale_map_if_needed, checkout_task_payload
from devcouncil.indexing.repo_mapper import RepoMapper
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import TaskRepository
from devcouncil.utils.json_persist import write_json


def _git(root, *args):
    subprocess.run(["git", *args], cwd=root, check=True, capture_output=True, text=True)


def _seed_project(tmp_path: Path) -> None:
    # Include a declared entry root so liveness baselines can be marked complete
    # (empty-root scans are intentionally incomplete / non-ratcheting).
    (tmp_path / "pkg").mkdir()
    (tmp_path / "pkg" / "__init__.py").write_text("", encoding="utf-8")
    (tmp_path / "pkg" / "cli.py").write_text("def main():\n    pass\n", encoding="utf-8")
    (tmp_path / "pkg" / "a.py").write_text("x = 1\n", encoding="utf-8")
    (tmp_path / "pyproject.toml").write_text(
        '[project]\nname="x"\nversion="0"\n[project.scripts]\ncli="pkg.cli:main"\n',
        encoding="utf-8",
    )
    _git(tmp_path, "init")
    _git(tmp_path, "-c", "user.email=t@t", "-c", "user.name=t", "add", "-A")
    _git(tmp_path, "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", "init")
    initialize_project(tmp_path, quiet=True)


def test_refresh_stale_map_rewrites_fingerprint(tmp_path):
    _seed_project(tmp_path)
    map_path = tmp_path / ".devcouncil" / "repo_map.json"
    stale = json.loads(map_path.read_text(encoding="utf-8"))
    stale["generated_head"] = "deadbeef"
    stale["indexed_hash"] = "oldhash"
    map_path.write_text(json.dumps(stale), encoding="utf-8")
    assert RepoMapper(tmp_path).map_is_stale(stale) is True

    assert _refresh_stale_map_if_needed(tmp_path) is True
    fresh = json.loads(map_path.read_text(encoding="utf-8"))
    assert RepoMapper(tmp_path).map_is_stale(fresh) is False
    assert fresh["generated_head"] != "deadbeef"


def test_a_kernel_that_cannot_run_defers_the_refresh_instead_of_failing(tmp_path, monkeypatch, caplog):
    """A kernel that cannot run must not fail a checkout or a verify.

    `refresh_stale_map_if_needed` says exactly that above its
    `DevMapEngineError` branch, and both callers — checkout before the liveness
    baseline, verify before the gates — depend on it. Nothing asserted it.

    The assertion is on the *log*, not just the return value, and that is the
    point. An outer `except Exception` wraps the whole body, so deleting the
    `DevMapEngineError` branch entirely still returns False — a first version of
    this test asserted only the return and passed against that deletion. What
    actually differs is the classification: an unbuilt kernel is an expected
    condition, reported as deferred, while an unclassified failure is logged
    with a traceback. Collapsing the two turns a routine "kernel not built yet"
    into noise that reads like a defect.
    """
    _seed_project(tmp_path)
    map_path = tmp_path / ".devcouncil" / "repo_map.json"
    stale = json.loads(map_path.read_text(encoding="utf-8"))
    stale["generated_head"] = "deadbeef"
    map_path.write_text(json.dumps(stale), encoding="utf-8")

    from devcouncil.devmap_engine import DevMapEngineError

    def _kernel_unavailable(*args, **kwargs):
        raise DevMapEngineError("devmap binary not built")

    monkeypatch.setattr(
        "devcouncil.indexing.map_artifacts.refresh_map_artifacts", _kernel_unavailable
    )
    with caplog.at_level("WARNING", logger="devcouncil.indexing.map_refresh"):
        assert _refresh_stale_map_if_needed(tmp_path) is False

    messages = [r.getMessage() for r in caplog.records]
    assert any("deferred" in m for m in messages), messages
    assert not any("map refresh failed" in m for m in messages), messages
    assert not any(r.exc_info for r in caplog.records), "an unbuilt kernel is not a traceback"
    # Still stale, and still readable: the caller may proceed on the old map.
    assert json.loads(map_path.read_text(encoding="utf-8"))["generated_head"] == "deadbeef"


def test_an_unexpected_refresh_error_is_contained_not_propagated(tmp_path, monkeypatch):
    """The same containment for a failure the engine does not classify.

    The broad `except Exception` exists because a checkout must survive a
    refresh that breaks in a way nobody predicted — a corrupt artifact, a
    permissions error mid-write. It is the branch most likely to be "tidied"
    into a narrower catch by someone who cannot see what it protects.
    """
    _seed_project(tmp_path)
    map_path = tmp_path / ".devcouncil" / "repo_map.json"
    stale = json.loads(map_path.read_text(encoding="utf-8"))
    stale["generated_head"] = "deadbeef"
    map_path.write_text(json.dumps(stale), encoding="utf-8")

    def _explodes(*args, **kwargs):
        raise RuntimeError("something nobody predicted")

    monkeypatch.setattr(
        "devcouncil.indexing.map_artifacts.refresh_map_artifacts", _explodes
    )
    assert _refresh_stale_map_if_needed(tmp_path) is False


def test_refresh_skipped_when_map_fresh(tmp_path):
    _seed_project(tmp_path)
    # Init may leave the map briefly stale (agent guides written after fingerprint).
    _refresh_stale_map_if_needed(tmp_path)
    assert _refresh_stale_map_if_needed(tmp_path) is False


def test_refresh_disabled_by_config(tmp_path):
    _seed_project(tmp_path)
    cfg_path = tmp_path / ".devcouncil" / "config.yaml"
    text = cfg_path.read_text(encoding="utf-8")
    text = text.replace(
        "refresh_stale_map_on_checkout: true",
        "refresh_stale_map_on_checkout: false",
        1,
    )
    cfg_path.write_text(text, encoding="utf-8")

    write_json(
        tmp_path / ".devcouncil" / "repo_map.json",
        {
            "generated_head": "deadbeef",
            "indexed_hash": "oldhash",
            "languages": [],
            "frameworks": [],
            "package_managers": [],
            "test_commands": [],
            "important_files": [],
            "candidate_files": [],
            "files": [],
            "subsystems": [],
        },
    )
    assert _refresh_stale_map_if_needed(tmp_path) is False


def test_checkout_refreshes_before_baseline(tmp_path):
    _seed_project(tmp_path)
    db = get_db(tmp_path)
    assert db is not None
    with db.get_session() as session:
        TaskRepository(session).save(
            Task(id="TASK-001", title="T", description="D", allowed_commands=["pytest"])
        )

    map_path = tmp_path / ".devcouncil" / "repo_map.json"
    stale = json.loads(map_path.read_text(encoding="utf-8"))
    stale["generated_head"] = "deadbeef"
    stale["indexed_hash"] = "oldhash"
    map_path.write_text(json.dumps(stale), encoding="utf-8")

    result = checkout_task_payload(
        tmp_path, task_id="TASK-001", client_id="test-client"
    )
    assert result["ok"] is True

    fresh = json.loads(map_path.read_text(encoding="utf-8"))
    assert RepoMapper(tmp_path).map_is_stale(fresh) is False

    baseline = tmp_path / ".devcouncil" / "liveness_baseline" / "TASK-001.json"
    assert baseline.is_file()
    payload = json.loads(baseline.read_text(encoding="utf-8"))
    assert payload.get("complete") is True
