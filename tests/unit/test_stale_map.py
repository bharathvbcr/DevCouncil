"""Stale-map verification gate tests."""

from __future__ import annotations

import subprocess

from devcouncil.domain.task import Task
from devcouncil.indexing.repo_mapper import RepoMapper
from devcouncil.verification.checks.stale_map import detect_stale_map_gaps
from devcouncil.verification.difficulty import resolve_rigor_policy


def _git(root, *args):
    subprocess.run(["git", *args], cwd=root, check=True, capture_output=True, text=True)


def _commit(root, msg="init"):
    _git(root, "init")
    _git(root, "-c", "user.email=t@t", "-c", "user.name=t", "add", "-A")
    _git(root, "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", msg)


def _task(*, difficulty: str = "hard") -> Task:
    return Task(
        id="TASK-1",
        title="t",
        description="d",
        difficulty=difficulty,  # type: ignore[arg-type]
    )


def _gap_id(task_id: str, kind: str) -> str:
    return f"{task_id}-{kind}-1"


def test_missing_map_flagged_blocking(tmp_path):
    gaps = detect_stale_map_gaps(
        task=_task(difficulty="hard"),
        project_root=tmp_path,
        next_gap_id=_gap_id,
        stale_map_blocking=True,
    )
    assert len(gaps) == 1
    assert gaps[0].gap_type == "stale_map"
    assert gaps[0].blocking is True
    assert "missing" in gaps[0].description.lower()


def test_fresh_map_produces_no_gap(tmp_path):
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "a.py").write_text("x = 1\n", encoding="utf-8")
    _commit(tmp_path)
    map_path = tmp_path / ".devcouncil" / "repo_map.json"
    map_path.parent.mkdir(parents=True)
    repo_map = RepoMapper(tmp_path).map_repo(liveness=False)
    map_path.write_text(repo_map.model_dump_json(), encoding="utf-8")

    gaps = detect_stale_map_gaps(
        task=_task(),
        project_root=tmp_path,
        next_gap_id=_gap_id,
        stale_map_blocking=True,
    )
    assert gaps == []


def test_generated_archive_does_not_change_map_or_freshness(tmp_path):
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "debug_runtime.py").write_text("x = 1\n", encoding="utf-8")
    _commit(tmp_path)
    mapper = RepoMapper(tmp_path)
    repo_map = mapper.map_repo(liveness=False)

    assert any(item.path == "src/debug_runtime.py" for item in repo_map.files)
    archive = tmp_path / "devcouncil-0.3.1.tgz"
    archive.write_text("generated archive", encoding="utf-8")
    assert "devcouncil-0.3.1.tgz" not in mapper.get_git_files()
    assert not mapper.map_is_stale(repo_map.model_dump())


def test_stale_map_flagged_blocking_on_hard(tmp_path):
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "a.py").write_text("x = 1\n", encoding="utf-8")
    _commit(tmp_path)
    map_path = tmp_path / ".devcouncil" / "repo_map.json"
    map_path.parent.mkdir(parents=True)
    repo_map = RepoMapper(tmp_path).map_repo(liveness=False)
    map_path.write_text(repo_map.model_dump_json(), encoding="utf-8")

    (tmp_path / "src" / "b.py").write_text("y = 2\n", encoding="utf-8")
    _git(tmp_path, "-c", "user.email=t@t", "-c", "user.name=t", "add", "-A")
    _git(tmp_path, "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", "add b")

    gaps = detect_stale_map_gaps(
        task=_task(difficulty="hard"),
        project_root=tmp_path,
        next_gap_id=_gap_id,
        stale_map_blocking=True,
    )
    assert len(gaps) == 1
    assert gaps[0].gap_type == "stale_map"
    assert gaps[0].blocking is True
    assert gaps[0].suggested_command == "dev map"


def test_stale_map_advisory_when_not_blocking(tmp_path):
    (tmp_path / "a.py").write_text("x = 1\n", encoding="utf-8")
    _commit(tmp_path)
    stale = {
        "generated_head": "deadbeef",
        "indexed_hash": "oldhash",
        "subsystems": [],
    }
    gaps = detect_stale_map_gaps(
        task=_task(difficulty="easy"),
        project_root=tmp_path,
        next_gap_id=_gap_id,
        stale_map_blocking=False,
        repo_map=stale,
    )
    assert len(gaps) == 1
    assert gaps[0].blocking is False
    assert gaps[0].severity == "medium"


def test_legacy_map_without_fingerprints_not_flagged(tmp_path):
    gaps = detect_stale_map_gaps(
        task=_task(),
        project_root=tmp_path,
        next_gap_id=_gap_id,
        stale_map_blocking=True,
        repo_map={"subsystems": []},
    )
    assert gaps == []


def test_disabled_gate_returns_empty(tmp_path):
    gaps = detect_stale_map_gaps(
        task=_task(),
        project_root=tmp_path,
        next_gap_id=_gap_id,
        stale_map_enabled=False,
        stale_map_blocking=True,
        repo_map={"generated_head": "x", "indexed_hash": "y"},
    )
    assert gaps == []


def test_rigor_policy_stale_map_defaults():
    hard = resolve_rigor_policy(_task(difficulty="hard"), None, config=None)
    assert hard.stale_map_enabled is True
    assert hard.stale_map_blocking is True
    assert "stale_map_blocking" in hard.applied

    easy = resolve_rigor_policy(_task(difficulty="easy"), None, config=None)
    assert easy.stale_map_enabled is True
    assert easy.stale_map_blocking is False


def test_map_is_stale_fail_closed_on_git_files_error(tmp_path, monkeypatch):
    (tmp_path / "a.py").write_text("x = 1\n", encoding="utf-8")
    _commit(tmp_path)
    mapper = RepoMapper(tmp_path)
    dumped = mapper.map_repo(liveness=False).model_dump()
    assert not mapper.map_is_stale(dumped)

    def boom(self):  # noqa: ANN001
        raise RuntimeError("git ls-files failed")

    monkeypatch.setattr(RepoMapper, "get_git_files", boom)
    assert mapper.map_is_stale(dumped) is True


def test_map_is_stale_fail_closed_on_content_fingerprint_error(tmp_path, monkeypatch):
    (tmp_path / "a.py").write_text("x = 1\n", encoding="utf-8")
    _commit(tmp_path)
    mapper = RepoMapper(tmp_path)
    dumped = mapper.map_repo(liveness=False).model_dump()
    assert dumped.get("content_fingerprint")
    assert not mapper.map_is_stale(dumped)

    def boom(self, files):  # noqa: ANN001
        raise RuntimeError("fingerprint failed")

    monkeypatch.setattr(RepoMapper, "_content_fingerprint", boom)
    assert mapper.map_is_stale(dumped) is True


def test_legacy_missing_content_fingerprint_still_not_stale_by_content(tmp_path):
    """Keep intentional legacy rule: missing content_fingerprint → not content-stale."""
    (tmp_path / "a.py").write_text("x = 1\n", encoding="utf-8")
    _commit(tmp_path)
    mapper = RepoMapper(tmp_path)
    dumped = mapper.map_repo(liveness=False).model_dump()
    dumped.pop("content_fingerprint", None)
    (tmp_path / "a.py").write_text("x = 2\n", encoding="utf-8")
    assert not mapper.map_is_stale(dumped)
