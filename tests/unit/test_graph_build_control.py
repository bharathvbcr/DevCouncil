from __future__ import annotations

import os
from pathlib import Path
import json
import threading
import time

import pytest
from typer.testing import CliRunner

from devcouncil.codeintel.build_control import (
    BuildStatus,
    _write_status,
    read_build_status,
)
from devcouncil.codeintel.service import get_codeintel_service
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
    """`generate_map_artifacts` builds the *kernel* store and the Python cache reads it.

    This used to assert a Python `index.sqlite` generation. The kernel is the
    engine now; the Python store is a read cache that `load_code_graph` fills
    from the kernel's `code_graph.json` on first use.
    """
    from devcouncil.devmap_client import DevMapClient
    from devcouncil.indexing.graph.build import load_code_graph

    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / "app.py").write_text("def main():\n    return 1\n", encoding="utf-8")

    generate_map_artifacts(tmp_path, tmp_path / ".devcouncil" / "repo_map.json", quiet=True)

    status = DevMapClient(tmp_path).status()
    # Two generations, not one: the kernel's build, then the build that
    # indexes the AGENTS.md / CLAUDE.md the first one caused to be written.
    # A repository that already carries its guides commits exactly one.
    assert status.generation_id == 2
    assert status.node_count > 0
    graph = load_code_graph(tmp_path)
    assert graph is not None
    assert graph.meta.get("map_engine") == "devmap-rust"
    assert get_codeintel_service(tmp_path).store.current_generation() is not None


def test_read_build_status_marks_dead_worker_stale(tmp_path: Path) -> None:
    status = BuildStatus(
        build_id="dead-worker",
        state="building",
        mode="full",
        pid=2_000_000_000,
        phase="extract",
        completed=1,
        total=3,
        last_progress_at=time.time(),
        stall_timeout_seconds=90.0,
        total_timeout_seconds=900.0,
    )
    _write_status(tmp_path, status)

    loaded = read_build_status(tmp_path)
    assert loaded.state == "stale"
    assert "no longer running" in loaded.degraded_reason


def test_read_build_status_marks_stalled_without_recent_progress(tmp_path: Path) -> None:
    import os

    status = BuildStatus(
        build_id="stalled",
        state="building",
        mode="full",
        pid=os.getpid(),
        phase="resolve",
        completed=2,
        total=5,
        last_progress_at=time.time() - 120.0,
        stall_timeout_seconds=30.0,
        total_timeout_seconds=900.0,
    )
    _write_status(tmp_path, status)

    loaded = read_build_status(tmp_path)
    assert loaded.state == "stalled"
    assert "no graph progress or worker CPU" in loaded.degraded_reason


def test_read_build_status_returns_idle_on_missing_or_invalid_file(tmp_path: Path) -> None:
    assert read_build_status(tmp_path).state == "idle"
    path = tmp_path / ".devcouncil" / "codeintel" / "build_status.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("{not-json", encoding="utf-8")
    assert read_build_status(tmp_path).state == "idle"


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


def test_lease_held_flag_allows_write_without_owning_writer_lease(tmp_path: Path) -> None:
    """_lease_held=True still skips acquisition — callers must hold or use graph_build_session."""
    from concurrent.futures import ThreadPoolExecutor
    import time

    from devcouncil.codeintel.sync.lease import WriterLease
    from devcouncil.indexing.graph.build import write_code_graph
    from devcouncil.indexing.graph.schema import CodeGraph, GraphEdge, GraphNode, NodeKind

    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "app.py").write_text("def main():\n    return 1\n", encoding="utf-8")
    (tmp_path / ".devcouncil").mkdir()
    path = "src/app.py"

    def _g(name: str) -> CodeGraph:
        return CodeGraph(
            nodes=[
                GraphNode(id=path, kind=NodeKind.FILE, path=path, name="app.py", language="python"),
                GraphNode(
                    id=f"{path}::{name}",
                    kind=NodeKind.FUNCTION,
                    path=path,
                    name=name,
                    line=1,
                    end_line=2,
                    language="python",
                ),
            ],
            edges=[GraphEdge(source=path, target=f"{path}::{name}", kind="contains")],
            entry_roots=[path],
            generated_head="h",
            indexed_hash="i",
            content_fingerprint="c",
        )

    lock = tmp_path / ".devcouncil" / "codeintel" / "writer.lock"
    barrier = threading.Barrier(2)
    results: dict[str, object] = {}

    def writer() -> None:
        barrier.wait()
        write_code_graph(tmp_path, _g("race"), _lease_held=True)
        results["wrote"] = True

    def racer() -> None:
        barrier.wait()
        time.sleep(0.01)
        lease = WriterLease(lock)
        got = lease.acquire()
        results["rival_got_lease"] = got
        if got:
            write_code_graph(tmp_path, _g("rival"), _lease_held=True)
            lease.release()

    with ThreadPoolExecutor(max_workers=2) as pool:
        f1 = pool.submit(writer)
        f2 = pool.submit(racer)
        f1.result()
        f2.result()

    # Direct _lease_held=True remains unsafe by design; the isolated worker path
    # now acquires via graph_build_session instead of relying on the parent.
    assert results.get("wrote") is True
    assert results.get("rival_got_lease") is True


def test_graph_build_session_serializes_writers(tmp_path: Path) -> None:
    """Workers that enter graph_build_session block concurrent WriterLease holders."""
    from concurrent.futures import ThreadPoolExecutor

    from devcouncil.codeintel.build_control import graph_build_session
    from devcouncil.codeintel.sync.lease import WriterLease
    from devcouncil.indexing.graph.build import write_code_graph
    from devcouncil.indexing.graph.schema import CodeGraph, GraphEdge, GraphNode, NodeKind

    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "app.py").write_text("def main():\n    return 1\n", encoding="utf-8")
    (tmp_path / ".devcouncil").mkdir()
    path = "src/app.py"

    def _g(name: str) -> CodeGraph:
        return CodeGraph(
            nodes=[
                GraphNode(id=path, kind=NodeKind.FILE, path=path, name="app.py", language="python"),
                GraphNode(
                    id=f"{path}::{name}",
                    kind=NodeKind.FUNCTION,
                    path=path,
                    name=name,
                    line=1,
                    end_line=2,
                    language="python",
                ),
            ],
            edges=[GraphEdge(source=path, target=f"{path}::{name}", kind="contains")],
            entry_roots=[path],
            generated_head="h",
            indexed_hash="i",
            content_fingerprint="c",
        )

    lock = tmp_path / ".devcouncil" / "codeintel" / "writer.lock"
    barrier = threading.Barrier(2)
    results: dict[str, object] = {}

    def writer() -> None:
        with graph_build_session(tmp_path):
            barrier.wait()
            time.sleep(0.05)
            write_code_graph(tmp_path, _g("owned"), _lease_held=True)
            results["wrote"] = True

    def racer() -> None:
        barrier.wait()
        lease = WriterLease(lock)
        results["rival_got_lease"] = lease.acquire()
        if results["rival_got_lease"]:
            lease.release()

    with ThreadPoolExecutor(max_workers=2) as pool:
        f1 = pool.submit(writer)
        f2 = pool.submit(racer)
        f1.result()
        f2.result()

    assert results.get("wrote") is True
    assert results.get("rival_got_lease") is False


def test_cpu_heartbeat_keeps_a_working_worker_out_of_stalled_state(tmp_path: Path) -> None:
    """Phase counters flat + CPU climbing is a slow phase, not a stall.

    Regression for healthy builds being killed at 90%+ CPU: liveness tokenize
    and SQLite persist emit no phase progress for minutes.
    """
    now = time.time()
    status = BuildStatus(
        build_id="working",
        state="building",
        mode="full",
        pid=os.getpid(),
        phase="liveness:tokens",
        # Last phase counter is far older than the stall budget...
        last_progress_at=now - 600.0,
        # ...but the worker reported CPU a moment ago.
        last_cpu_progress_at=now - 1.0,
        worker_cpu_seconds=512.0,
        stall_timeout_seconds=30.0,
        total_timeout_seconds=7200.0,
    )
    _write_status(tmp_path, status)

    loaded = read_build_status(tmp_path)
    assert loaded.state == "building"
    assert loaded.degraded_reason == ""


def test_stall_still_fires_when_cpu_is_flat_too(tmp_path: Path) -> None:
    """A genuinely wedged worker (no phase progress, no CPU) is still killed."""
    now = time.time()
    status = BuildStatus(
        build_id="wedged",
        state="building",
        mode="full",
        pid=os.getpid(),
        phase="persist:nodes",
        last_progress_at=now - 600.0,
        last_cpu_progress_at=now - 600.0,
        worker_cpu_seconds=12.0,
        stall_timeout_seconds=30.0,
        total_timeout_seconds=7200.0,
    )
    _write_status(tmp_path, status)

    loaded = read_build_status(tmp_path)
    assert loaded.state == "stalled"
    assert "no graph progress or worker CPU" in loaded.degraded_reason


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
def test_pdg_merge_preserves_the_kernel_stamp_and_freshness(tmp_path: Path) -> None:
    """`dev map --pdg` must not turn the graph into a foreign artifact.

    ``_build_pdg_layer`` re-writes ``code_graph.json`` through the Python
    ``write_code_graph``. If that dropped ``meta.map_engine`` or restamped
    freshness from today's tree, ``dev map doctor`` would report a foreign
    writer (CRITICAL) immediately after a successful build. It does not: the
    slim export copies ``meta`` and the fingerprints ride on the graph object,
    and this test is what keeps it that way.
    """
    import subprocess

    from devcouncil import devmap_health
    from devcouncil.indexing.graph.build import (
        build_pdg_for_paths,
        load_code_graph,
        merge_pdg_into_graph,
        write_code_graph,
    )

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
    before = json.loads(graph_path.read_text(encoding="utf-8"))
    assert before["meta"]["map_engine"] == "devmap-rust"

    graph = load_code_graph(tmp_path)
    assert graph is not None
    merge_pdg_into_graph(graph, build_pdg_for_paths(tmp_path, graph))
    write_code_graph(tmp_path, graph)

    after = json.loads(graph_path.read_text(encoding="utf-8"))
    assert after["meta"]["map_engine"] == "devmap-rust", "PDG export orphaned the graph"
    assert "pdg" in after["meta"], "the PDG layer must actually be on disk"
    for stamp in ("generated_head", "indexed_hash", "content_fingerprint"):
        assert after[stamp] == before[stamp], f"PDG export restamped {stamp}"
    doctor = devmap_health.run_doctor(tmp_path)
    failed = [check["name"] for check in doctor["checks"] if check["ok"] is False]
    assert doctor["ok"], f"doctor failed after --pdg: {failed}"
