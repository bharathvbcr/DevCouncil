"""The kernel build seam: one generation per changed tree, and its honesty fields.

``codeintel/build_control.py`` -- the Python graph build's status file, stall
and CPU-heartbeat watchdog, and cross-process writer lease -- was deleted with
the Python store it guarded, and the tests of it went with it: `BuildStatus`,
`read_build_status`, `_write_status`, `graph_build_session` and the
`_lease_held` race pinned the concurrency of a writer that no longer exists.
The kernel takes its own advisory `flock` on `devmap.sqlite`, released by the
OS when the holder dies.
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest
from typer.testing import CliRunner

from devcouncil.indexing.map_artifacts import generate_map_artifacts


@pytest.fixture
def anyio_backend():
    return "asyncio"


def _have_kernel() -> bool:
    from devcouncil.devmap_engine import DevMapEngineError, find_engine_binary

    try:
        find_engine_binary()
    except DevMapEngineError:
        return False
    return True


requires_kernel = pytest.mark.skipif(
    not _have_kernel(), reason="devmap kernel not built (cargo build --release -p devmap-cli)"
)


@requires_kernel
def test_full_map_commits_one_generation_and_records_progress(tmp_path: Path) -> None:
    """`generate_map_artifacts` builds the kernel store and writes its artifact.

    This asserted a Python `index.sqlite` generation, then a `load_code_graph`
    read of the cache that filled it. Both are deleted; the graph is read where
    the kernel writes it.
    """
    from devcouncil.devmap_client import DevMapClient
    from devcouncil.indexing.graph.build import read_code_graph

    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / "app.py").write_text("def main():\n    return 1\n", encoding="utf-8")

    generate_map_artifacts(tmp_path, tmp_path / ".devcouncil" / "repo_map.json", quiet=True)

    status = DevMapClient(tmp_path).status()
    # Two generations, not one: the kernel's build, then the build that
    # indexes the AGENTS.md / CLAUDE.md the first one caused to be written.
    # A repository that already carries its guides commits exactly one.
    assert status.generation_id == 2
    assert status.node_count > 0
    graph = read_code_graph(tmp_path)
    assert graph is not None
    assert graph.meta.get("map_engine") == "devmap-rust"


@requires_kernel
def test_incremental_map_after_full_map_commits_one_generation(tmp_path: Path) -> None:
    from devcouncil.devmap_client import DevMapClient

    (tmp_path / ".devcouncil").mkdir()
    app = tmp_path / "app.py"
    app.write_text("def main():\n    return 1\n", encoding="utf-8")

    generate_map_artifacts(tmp_path, tmp_path / ".devcouncil" / "repo_map.json", quiet=True)
    gen_after_full = DevMapClient(tmp_path).status().generation_id
    assert gen_after_full == 2  # kernel build + the guides it wrote (see above)

    app.write_text("def main():\n    return 2\n", encoding="utf-8")
    generate_map_artifacts(tmp_path, tmp_path / ".devcouncil" / "repo_map.json", quiet=True)
    assert DevMapClient(tmp_path).status().generation_id == gen_after_full + 1

    # An unchanged tree is a no-op in the kernel: no new generation.
    generate_map_artifacts(tmp_path, tmp_path / ".devcouncil" / "repo_map.json", quiet=True)
    assert DevMapClient(tmp_path).status().generation_id == gen_after_full + 1


def test_map_cli_surfaces_build_status_fields(tmp_path: Path) -> None:
    from devcouncil.cli.commands.init import initialize_project
    from devcouncil.cli.main import app

    initialize_project(tmp_path, quiet=True, with_map=False, with_skills=False)
    (tmp_path / "a.py").write_text("def a():\n    return 1\n", encoding="utf-8")

    runner = CliRunner()
    result = runner.invoke(app, ["map", "status", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "generation" in result.output.lower() or "healthy" in result.output.lower()


@pytest.mark.anyio
async def test_graph_ingest_busy_returns_structured_error(tmp_path: Path, monkeypatch) -> None:
    """A kernel that cannot build (not built, older than the store, another
    writer holding its lock) is a structured error, never an MCP exception."""
    from devcouncil.devmap_engine import DevMapEngineError
    from devcouncil.integrations.mcp.handlers import map as mapmod

    monkeypatch.setattr(
        "devcouncil.indexing.map_artifacts.refresh_map_artifacts",
        lambda *a, **k: (_ for _ in ()).throw(
            DevMapEngineError("another devmap writer (pid 4242) holds the store")
        ),
    )
    result = await mapmod.handle_graph_ingest(tmp_path, {"paths": ["a.py"]})
    payload = json.loads(result[0].text)
    assert payload["ok"] is False
    assert payload["code"] == "engine_unavailable"
    assert "pid 4242" in payload["error"]
    assert payload["paths"] == ["a.py"]


def _incomplete_refresh(**over):
    from devcouncil.indexing.map_artifacts import GraphRefreshResult

    base = dict(
        repo_map=None,
        graph=None,
        generation=7,
        mode="prior_generation",
        degraded=False,
        reason="GraphBuildTimeout: no progress for 90.0s",
        compatibility_export_degraded=False,
        build_incomplete=True,
    )
    base.update(over)
    return GraphRefreshResult(**base)


def test_mcp_graph_ingest_reports_a_not_fresh_kernel_store(tmp_path: Path, monkeypatch) -> None:
    """An agent must be told when the kernel could not index part of the tree.

    The kernel fails closed instead of serving a prior generation, so the
    "older than HEAD" case this test used to pin no longer exists. What can
    still happen is a committed generation with paths the kernel could not
    process — absent from the graph — and that must be in the payload.
    """
    import asyncio
    import json as _json

    from devcouncil.devmap_client import DevMapStatus
    from devcouncil.integrations.mcp.handlers import map as map_handler

    refresh = _incomplete_refresh(
        build_incomplete=False,
        reason="2 path(s) exceeded the retry threshold",
        kernel_status=DevMapStatus(
            generation_id=7,
            pending_count=2,
            node_count=10,
            edge_count=5,
            is_fresh=False,
            degraded_reason="2 path(s) exceeded the retry threshold",
            quarantined_count=2,
        ),
    )
    monkeypatch.setattr(
        "devcouncil.indexing.map_artifacts.refresh_map_artifacts",
        lambda *a, **k: refresh,
    )
    result = asyncio.run(map_handler.handle_graph_ingest(tmp_path, {}))
    payload = _json.loads(result[0].text)

    assert payload["generation"] == 7
    assert payload["kernel"]["is_fresh"] is False
    assert payload["kernel"]["quarantined_count"] == 2
    assert "not fresh" in payload["detail"]
    assert "repair --pending" in payload["detail"]


@requires_kernel
def test_pdg_layer_leaves_the_kernels_artifact_untouched(tmp_path: Path) -> None:
    """`dev map --pdg` must not turn the graph into a foreign artifact.

    It used to merge the layer into a `CodeGraph` and call `write_code_graph`,
    which rewrites `code_graph.json`. The risk then was that the rewrite dropped
    `meta.map_engine` or restamped freshness from today's tree, so `dev map
    doctor` would report a foreign writer (CRITICAL) immediately after a
    successful build, and this test pinned the stamps that survived it.

    The rewrite is gone: the layer lands in `.devcouncil/graph/pdg.json` and the
    kernel is the only writer of `code_graph.json`. The invariant is now the
    stronger one -- not "the rewrite preserves the stamps" but "there is no
    rewrite" -- and it is asserted on the bytes, through `_build_pdg_layer`, the
    function `dev map --pdg` actually calls. The old test drove
    `load_code_graph` + `merge_pdg_into_graph` + `write_code_graph` by hand, so
    it kept passing while its docstring described a path production had left.
    """
    import subprocess

    from devcouncil import devmap_health
    from devcouncil.cli.commands.map import _build_pdg_layer

    subprocess.run(["git", "init", "-q"], cwd=tmp_path, check=True)
    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / "app.py").write_text(
        "import os\n\n\ndef read_input():\n    return os.environ.get('X', '')\n\n\n"
        "def main():\n    return eval(read_input())\n",
        encoding="utf-8",
    )
    map_path = tmp_path / ".devcouncil" / "repo_map.json"
    generate_map_artifacts(tmp_path, map_path, quiet=True)

    graph_path = tmp_path / ".devcouncil" / "graph" / "code_graph.json"
    before_bytes = graph_path.read_bytes()
    before = json.loads(before_bytes.decode("utf-8"))
    assert before["meta"]["map_engine"] == "devmap-rust"

    _build_pdg_layer(tmp_path)

    assert graph_path.read_bytes() == before_bytes, (
        "the PDG layer rewrote code_graph.json; the kernel is its only writer"
    )
    sidecar = tmp_path / ".devcouncil" / "graph" / "pdg.json"
    assert sidecar.is_file(), "the PDG layer must write its own artifact"
    layer = json.loads(sidecar.read_text(encoding="utf-8"))
    assert layer["files"], "an empty layer would mean the analysis did not run"

    doctor = devmap_health.run_doctor(tmp_path)
    failed = [check["name"] for check in doctor["checks"] if check["ok"] is False]
    assert doctor["ok"], f"doctor failed after --pdg: {failed}"
