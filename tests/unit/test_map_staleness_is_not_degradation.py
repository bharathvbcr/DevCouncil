"""Staleness is age, not coverage — and its reason names what changed.

Found on this repository after the 2026-09-06 merge: every fingerprint in
`repo_map.json` matched the tree and `git status` was clean, yet
`RepoMapper.map_is_stale` answered True — because the map's `graph_degraded`
flag was set (a vendored 30 MB `parser.c` refused at the source ceiling, two
files with no linked grammar), and the old rule treated a degraded map as
stale "so --if-stale keeps retrying until healthy". On a permanently degraded
repository that made eleven consumers call the map stale forever, the hook's
Continuity line demand a rebuild on every prompt, and `map_freshness` explain
the verdict as "tracked files changed since the map was written".

`RepoMapper.staleness` now owns both the verdict and its reason.
"""

from __future__ import annotations

import subprocess
from pathlib import Path

from devcouncil.devmap_health import map_freshness
from devcouncil.indexing.repo_mapper import RepoMapper

from tests.unit.support_maps import stamped_repo_map


def _git(root: Path, *args: str) -> None:
    subprocess.run(
        ["git", "-c", "user.email=t@t", "-c", "user.name=t", *args],
        cwd=root,
        check=True,
        capture_output=True,
        text=True,
    )


def _repo(tmp_path: Path) -> Path:
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "a.py").write_text("def a():\n    return 1\n", encoding="utf-8")
    _git(tmp_path, "init", "-q")
    _git(tmp_path, "add", "-A")
    _git(tmp_path, "commit", "-q", "-m", "init")
    return tmp_path


def _write(root: Path, payload: dict) -> None:
    import json

    out = root / ".devcouncil" / "repo_map.json"
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(payload), encoding="utf-8")


def test_a_degraded_map_of_the_current_tree_is_current(tmp_path: Path) -> None:
    root = _repo(tmp_path)
    payload = stamped_repo_map(root).model_dump()
    payload["graph_degraded"] = True
    payload["graph_degraded_reason"] = "1 refused by discovery and never read at all"
    mapper = RepoMapper(root)
    assert mapper.staleness(payload) is None, "every fingerprint matches; coverage is not age"
    assert mapper.map_is_stale(payload) is False
    _write(root, payload)
    freshness = map_freshness(root)
    assert freshness["fresh"] is True, freshness
    assert freshness["reason"] == ""


def test_the_reason_names_what_actually_changed(tmp_path: Path) -> None:
    root = _repo(tmp_path)
    mapper = RepoMapper(root)
    payload = stamped_repo_map(root).model_dump()
    assert mapper.staleness(payload) is None

    # Contents: the file set and HEAD are unchanged.
    (root / "src" / "a.py").write_text("def a():\n    return 2\n", encoding="utf-8")
    reason = mapper.staleness(payload)
    assert reason == "tracked file contents changed since the map was written", reason
    _write(root, payload)
    assert map_freshness(root)["reason"] == reason

    # The file set: a new tracked file, before any commit.
    (root / "src" / "b.py").write_text("def b():\n    return 3\n", encoding="utf-8")
    _git(root, "add", "src/b.py")
    reason = mapper.staleness(payload)
    assert reason == "tracked file set changed since the map was written", reason

    # HEAD moves: named first, because it is the cheapest and most specific fact.
    _git(root, "add", "-A")
    _git(root, "commit", "-q", "-m", "edit")
    reason = mapper.staleness(payload)
    assert reason is not None and reason.startswith("map was built from "), reason
    assert " but HEAD is " in reason
    assert map_freshness(root)["reason"] == reason


def test_an_unstamped_map_is_not_an_alarm(tmp_path: Path) -> None:
    root = _repo(tmp_path)
    mapper = RepoMapper(root)
    assert mapper.staleness({"files": []}) is None
    assert mapper.map_is_stale({"files": []}) is False


def test_a_tree_that_cannot_be_listed_is_stale_and_says_it_could_not_be_verified(
    tmp_path: Path, monkeypatch
) -> None:
    root = _repo(tmp_path)
    mapper = RepoMapper(root)
    payload = stamped_repo_map(root).model_dump()

    def _boom(*_args, **_kwargs):
        raise RuntimeError("git is wedged")

    monkeypatch.setattr(mapper, "get_git_files", _boom)
    reason = mapper.staleness(payload)
    assert reason == "tracked files could not be listed, so freshness could not be verified"
    assert mapper.map_is_stale(payload) is True, "fail closed"
