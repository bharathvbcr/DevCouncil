"""Test support for the kernel-era map: a stamped map without a kernel, a stub
kernel, and the live import-edge view.

`RepoMapper.map_repo` — the Python engine's map builder — was retired on
2026-09-02 with `build_code_graph`; the Rust kernel builds the map. Tests that
used `map_repo` as a *fixture* (to get a fresh, stamped `RepoMap` on disk, or to
read import edges) use these instead, so they keep testing the behaviour they
were written for against the owners that still exist.
"""

from __future__ import annotations

import json
from pathlib import Path
from types import SimpleNamespace

from devcouncil.indexing.repo_mapper import RepoMap, RepoMapper


def stamped_repo_map(root: Path) -> RepoMap:
    """A minimal, kernel-shaped map whose freshness stamps match *root* now.

    Files come from the same inventory `map_is_stale` reads, and the stamps
    from the same function the seam hands to the kernel, so the map reads
    fresh to every consumer exactly as a kernel-written one would.
    """
    from devcouncil.devmap_engine import compute_freshness

    root = Path(root)
    files = RepoMapper(root).get_git_files()
    payload = {
        "languages": sorted({"python" for f in files if f.endswith(".py")} | {"typescript" for f in files if f.endswith(".ts")}),
        "frameworks": [],
        "package_managers": [],
        "test_commands": [],
        "important_files": [],
        "candidate_files": [],
        "files": [
            {"area": ".", "kind": "code" if f.endswith((".py", ".ts", ".js", ".go", ".rs")) else "doc",
             "language": "python" if f.endswith(".py") else "", "path": f}
            for f in sorted(files)
        ],
        "subsystems": [],
        "dependency_risks": [],
        "map_engine": "devmap-rust",
        **compute_freshness(root),
    }
    return RepoMap.model_validate(payload)


def write_stamped_map(root: Path) -> Path:
    """Write `stamped_repo_map(root)` to `.devcouncil/repo_map.json` and return the path."""
    root = Path(root)
    out = root / ".devcouncil" / "repo_map.json"
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(stamped_repo_map(root).model_dump_json(), encoding="utf-8")
    return out


def stub_kernel(monkeypatch, *, record: list | None = None) -> None:  # noqa: ANN001
    """Replace `build_map` with a writer of a stamped, kernel-shaped map + graph.

    The map lists the repository's files so goal ranking has something to
    rank; the guides are held still so a build is one call, not two.
    """
    from devcouncil import devmap_engine
    from devcouncil.cli.commands import map as map_cmd
    from devcouncil.indexing import map_artifacts

    def _fake_build_map(root: Path, *, output=None, graph_output=None, timeout=900.0, full=False):  # noqa: ANN001
        if record is not None:
            record.append({"root": root, "full": full})
        out = output if output is not None else root / ".devcouncil" / "repo_map.json"
        out = out if out.is_absolute() else root / out
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(stamped_repo_map(root).model_dump_json(), encoding="utf-8")
        graph = root / ".devcouncil" / "graph" / "code_graph.json"
        graph.parent.mkdir(parents=True, exist_ok=True)
        graph.write_text(json.dumps({"meta": {"map_engine": "devmap-rust"}, "nodes": [], "edges": []}), encoding="utf-8")
        return out

    monkeypatch.setattr(map_cmd, "build_map", _fake_build_map, raising=False)
    monkeypatch.setattr(devmap_engine, "build_map", _fake_build_map)
    monkeypatch.setattr(map_artifacts, "write_agent_guides", lambda *_a, **_k: False)


def dependents_view(root: Path) -> SimpleNamespace:
    """The live import-edge owner (`RepoMapper.dependents_for`, used by verify's
    wiring check) presented with the `.dependents` attribute the old map had."""
    mapper = RepoMapper(Path(root))
    return SimpleNamespace(dependents=mapper.dependents_for(mapper.get_git_files()))
