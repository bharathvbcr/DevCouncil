"""Incremental refresh, cache v5, warm/cold parity, and content fingerprint staleness."""

from __future__ import annotations

import json
import subprocess
import time

from devcouncil.indexing.graph.build import (
    build_code_graph,
    content_fingerprint,
    refresh_map_for_paths,
)
from devcouncil.indexing.graph.cache import PARSE_CACHE_VERSION
from devcouncil.indexing.repo_mapper import RepoMapper


def _git(root, *args):
    subprocess.run(["git", *args], cwd=root, check=True, capture_output=True, text=True)


def _commit(root):
    _git(root, "init")
    _git(root, "-c", "user.email=t@t", "-c", "user.name=t", "add", "-A")
    _git(root, "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", "init")


def _write(tmp_path, files):
    for rel, content in files.items():
        p = tmp_path / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(content, encoding="utf-8")


def test_parse_cache_v5_has_symbols_and_import_details(tmp_path):
    _write(tmp_path, {
        "pkg/__init__.py": "",
        "pkg/a.py": "from pkg.util import helper\ndef foo():\n    return helper()\n",
        "pkg/util.py": "def helper():\n    return 1\n",
    })
    _commit(tmp_path)
    RepoMapper(tmp_path).map_repo(liveness=False)
    data = json.loads(
        (tmp_path / ".devcouncil" / "cache" / "repo_map_parse.json").read_text(encoding="utf-8")
    )
    assert data["version"] == PARSE_CACHE_VERSION == RepoMapper._PARSE_CACHE_VERSION == 5
    entry = data["files"]["pkg/a.py"]
    assert entry["sha256"]
    assert isinstance(entry.get("symbols"), list)
    assert any(s.get("name") == "foo" for s in entry["symbols"])
    assert isinstance(entry.get("import_details"), list)
    assert any(
        "helper" in (d.get("names") or []) for d in entry["import_details"] if isinstance(d, dict)
    )
    # True end_line persisted (multi-line function)
    foo = next(s for s in entry["symbols"] if s.get("name") == "foo")
    assert foo.get("end_line", 0) >= foo.get("line", 0)


def test_warm_cold_named_import_edge_parity(tmp_path):
    """Cold build of ``from pkg.util import helper`` has 1 named-import edge; warm must too.

    Reproduces the confirmed cache-v3 bug where warm rebuilds dropped import_details
    and produced 0 named-import symbol edges.
    """
    _write(tmp_path, {
        "pkg/__init__.py": "",
        "pkg/util.py": "def helper():\n    return 1\n",
        "pkg/main.py": "from pkg.util import helper\n\ndef run():\n    return helper()\n",
    })
    _commit(tmp_path)

    cold = build_code_graph(tmp_path, liveness=False)
    cold_named = [
        e for e in cold.edges
        if e.kind == "imports" and e.reason == "named import"
    ]
    assert len(cold_named) == 1
    assert cold_named[0].target.endswith("::helper")

    # Cache must retain import_details after cold build
    data = json.loads(
        (tmp_path / ".devcouncil" / "cache" / "repo_map_parse.json").read_text(encoding="utf-8")
    )
    assert data["version"] == 5
    assert data["files"]["pkg/main.py"].get("import_details")

    warm = build_code_graph(tmp_path, liveness=False)
    warm_named = [
        e for e in warm.edges
        if e.kind == "imports" and e.reason == "named import"
    ]
    assert len(warm_named) == 1, (
        f"warm named-import edges={len(warm_named)} (expected 1; cold had {len(cold_named)})"
    )
    assert warm_named[0].target == cold_named[0].target


def test_map_repo_single_pass_token_scan(tmp_path, monkeypatch):
    """map_repo with liveness must run the token scan exactly once via graph build."""
    _write(tmp_path, {
        "pkg/__init__.py": "",
        "pkg/a.py": "def orphan():\n    return 1\n",
    })
    _commit(tmp_path)

    calls: list[int] = []
    import devcouncil.indexing.graph.build as build_mod

    real = build_mod._token_scan_dead

    def tracking(*args, **kwargs):
        calls.append(1)
        return real(*args, **kwargs)

    monkeypatch.setattr(build_mod, "_token_scan_dead", tracking)
    RepoMapper(tmp_path).map_repo(liveness=True)
    assert len(calls) == 1


def test_content_fingerprint_marks_edit_stale(tmp_path):
    _write(tmp_path, {
        "pkg/__init__.py": "",
        "pkg/a.py": "def foo():\n    return 1\n",
    })
    _commit(tmp_path)
    mapper = RepoMapper(tmp_path)
    repo_map = mapper.map_repo(liveness=False)
    dumped = repo_map.model_dump()
    assert not mapper.map_is_stale(dumped)
    time.sleep(0.05)
    (tmp_path / "pkg" / "a.py").write_text("def foo():\n    return 2\n", encoding="utf-8")
    assert mapper.map_is_stale(dumped)


def test_legacy_map_without_content_fingerprint_not_stale(tmp_path):
    _write(tmp_path, {"pkg/a.py": "x=1\n", "pkg/__init__.py": ""})
    _commit(tmp_path)
    mapper = RepoMapper(tmp_path)
    repo_map = mapper.map_repo(liveness=False)
    dumped = repo_map.model_dump()
    dumped.pop("content_fingerprint", None)
    (tmp_path / "pkg" / "a.py").write_text("x=2\n", encoding="utf-8")
    # Without content_fingerprint field, content edits alone must not alarm
    # (HEAD + indexed_hash still match if no commit).
    assert not mapper.map_is_stale(dumped)


def test_refresh_map_for_paths(tmp_path):
    _write(tmp_path, {
        "pkg/__init__.py": "",
        "pkg/a.py": "def foo():\n    return 1\n",
        "pkg/b.py": "def bar():\n    return 1\n",
    })
    _commit(tmp_path)
    RepoMapper(tmp_path).map_repo(liveness=False)
    (tmp_path / "pkg" / "a.py").write_text(
        "def foo():\n    return 1\ndef baz():\n    return 2\n", encoding="utf-8"
    )
    graph = refresh_map_for_paths(tmp_path, ["pkg/a.py"], liveness=False)
    assert any(n.name == "baz" for n in graph.nodes)
    assert content_fingerprint(tmp_path, RepoMapper(tmp_path).get_git_files())


def test_incremental_reextracts_sha_mismatch_outside_changed_paths(tmp_path):
    """Edit B but refresh only A — sha check must still pick up B's new symbol."""
    _write(tmp_path, {
        "pkg/__init__.py": "",
        "pkg/a.py": "def foo():\n    return 1\n",
        "pkg/b.py": "def bar():\n    return 1\n",
    })
    _commit(tmp_path)
    RepoMapper(tmp_path).map_repo(liveness=False)
    (tmp_path / "pkg" / "a.py").write_text(
        "def foo():\n    return 1\ndef baz():\n    return 2\n", encoding="utf-8"
    )
    (tmp_path / "pkg" / "b.py").write_text(
        "def bar():\n    return 1\ndef qux():\n    return 3\n", encoding="utf-8"
    )
    graph = refresh_map_for_paths(tmp_path, ["pkg/a.py"], liveness=False)
    names = {n.name for n in graph.nodes}
    assert "baz" in names
    assert "qux" in names, "missed-path edit must re-extract via sha256 mismatch"


def test_graph_build_failure_omits_token_only_dead(tmp_path, monkeypatch):
    """On assemble failure, map must not flood dead_symbol_candidates via token-only."""
    _write(tmp_path, {
        "pkg/__init__.py": "",
        "pkg/orphan.py": (
            "def unused_alpha():\n    return 1\n"
            "def unused_beta():\n    return 2\n"
            "def unused_gamma():\n    return 3\n"
        ),
    })
    _commit(tmp_path)

    def boom(*_a, **_k):
        raise RuntimeError("forced graph failure")

    monkeypatch.setattr(
        "devcouncil.indexing.graph.build.build_code_graph",
        boom,
    )
    repo_map = RepoMapper(tmp_path).map_repo(liveness=True)
    assert repo_map.dead_symbol_candidates == []
    # Fingerprint is still stamped so freshness tracking continues.
    assert repo_map.content_fingerprint
    assert repo_map.generated_head
