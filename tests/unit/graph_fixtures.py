"""Hand-built ``CodeGraph`` fixtures for tests whose subject is a graph *consumer*.

These tests used to get their graph by running the Python extractor and resolver
(``build_code_graph``) over a temporary source tree. That engine was retired: the
Rust kernel extracts and resolves the graph now, and the Python side only reads it
back. A consumer test that still built its own graph would have been testing the
retired builder as much as the consumer.

So the graphs here are written directly, in the same node/edge vocabulary the kernel
emits into ``code_graph.json`` — see ``node_kind_label`` / ``edge_kind_label`` in
``rust-port/crates/devmap-query/src/code_graph.rs``, which map the kernel's own kinds
onto exactly these strings. That keeps the consumer under test and makes the fixture
say plainly what shape of graph each assertion depends on, instead of leaving it to
whatever the extractor happened to produce.

Extraction and resolution themselves are covered by the kernel's own suites
(``devmap-extract``, ``devmap-resolve``, ``devmap-analyze``).
"""

from __future__ import annotations

import json
import subprocess
from pathlib import Path
from typing import Any, Iterable, Mapping, Sequence

from devcouncil.indexing.graph.schema import (
    CodeGraph,
    Confidence,
    DeadCodeEntry,
    GraphEdge,
    GraphNode,
    NodeKind,
)

#: Suffix → the ``language`` string the kernel stamps on a file node.
_LANGUAGE_BY_SUFFIX = {
    ".py": "python",
    ".ts": "typescript",
    ".tsx": "typescript",
    ".js": "javascript",
    ".jsx": "javascript",
    ".go": "go",
    ".rs": "rust",
}

# A symbol is a bare name, or a name paired with its kind, or both plus extras.
SymbolSpec = "str | tuple[str, NodeKind] | tuple[str, NodeKind, dict[str, Any]]"


def _language_for(path: str) -> str:
    return _LANGUAGE_BY_SUFFIX.get(Path(path).suffix.lower(), "")


def _area_for(path: str) -> str:
    parts = path.replace("\\", "/").split("/")
    return parts[0] if len(parts) > 1 else "root"


def _normalize(spec: Any) -> tuple[str, NodeKind, dict[str, Any]]:
    if isinstance(spec, str):
        return spec, NodeKind.FUNCTION, {}
    if len(spec) == 2:
        return spec[0], spec[1], {}
    return spec[0], spec[1], dict(spec[2])


def code_graph(
    files: Mapping[str, Sequence[Any]],
    *,
    imports: Iterable[tuple[str, str]] = (),
    calls: Iterable[tuple[str, str]] = (),
    extra_edges: Iterable[GraphEdge] = (),
    extra_nodes: Iterable[GraphNode] = (),
    dead_code: Iterable[DeadCodeEntry] = (),
    entry_roots: Iterable[str] = (),
    unwired_candidates: Iterable[str] = (),
    unreachable_files: Iterable[str] = (),
    meta: Mapping[str, Any] | None = None,
) -> CodeGraph:
    """Build a graph from ``{path: [symbol, ...]}`` plus import and call pairs.

    ``imports`` are ``(importer_path, imported_path)``; ``calls`` are
    ``(caller_id, callee_id)`` where an id is ``path::qualname``. Every file gets a
    ``file`` node and a ``contains`` edge to each of its symbols, which is the shape
    the kernel writes and every consumer here reads.
    """
    nodes: list[GraphNode] = []
    edges: list[GraphEdge] = []

    for line_base, (path, symbols) in enumerate(sorted(files.items())):
        nodes.append(
            GraphNode(
                id=path,
                kind=NodeKind.FILE,
                path=path,
                name=Path(path).name,
                area=_area_for(path),
                language=_language_for(path),
                exported=True,
            )
        )
        for index, spec in enumerate(symbols):
            name, kind, extras = _normalize(spec)
            symbol_id = f"{path}::{name}"
            line = 1 + index * 4
            nodes.append(
                GraphNode(
                    id=symbol_id,
                    kind=kind,
                    path=path,
                    name=name,
                    line=line,
                    end_line=line + 2,
                    area=_area_for(path),
                    language=_language_for(path),
                    exported=not name.rsplit(".", 1)[-1].startswith("_"),
                    extras=extras,
                )
            )
            edges.append(GraphEdge(source=path, target=symbol_id, kind="contains"))

    for importer, imported in imports:
        edges.append(GraphEdge(source=importer, target=imported, kind="imports"))
    for caller, callee in calls:
        edges.append(GraphEdge(source=caller, target=callee, kind="calls"))
    edges.extend(extra_edges)
    nodes.extend(extra_nodes)

    return CodeGraph(
        nodes=nodes,
        edges=edges,
        dead_code=list(dead_code),
        entry_roots=list(entry_roots),
        unwired_candidates=list(unwired_candidates),
        unreachable_files=list(unreachable_files),
        meta=dict(meta or {}),
    )


def write_sources(root: Path, files: Mapping[str, str]) -> None:
    """Write ``{relative path: content}`` under ``root``, creating parents."""
    for rel, content in files.items():
        path = root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")


def git_init_commit(root: Path) -> None:
    """Make ``root`` a git repository with everything in it committed.

    Several consumers read ``git ls-files`` or ``git rev-parse HEAD``; without a
    commit they fall back to a directory walk and an empty head, which changes what
    the assertion is actually measuring.
    """
    identity = ["-c", "user.email=t@t", "-c", "user.name=t"]

    def _git(*args: str) -> None:
        subprocess.run(
            ["git", *args], cwd=root, check=True, capture_output=True, text=True
        )

    _git("init")
    _git(*identity, "add", "-A")
    _git(*identity, "commit", "-m", "init")


def write_graph_artifact(root: Path, graph: CodeGraph) -> Path:
    """Put ``graph`` on disk where the kernel puts ``code_graph.json``.

    Tests that need a consumer to *find* a graph used ``build.write_code_graph``
    for this. That function was the Python SQLite store's write path: it took a
    writer lease, persisted every node and edge into ``index.sqlite``, and only
    then wrote the JSON through a slim/compact/stub size-tiering ladder. It was
    deleted with the store — the kernel is the only writer of this artifact, and
    the last production caller of the Python writer went in Lane M3.

    What a consumer test actually needs is the file, so this writes the file:
    one ``json.dump`` in the same shape ``read_code_graph`` validates. Untiered,
    which is what the kernel's own artifact is, so a test fixture cannot
    accidentally assert against a capped export.
    """
    path = root / ".devcouncil" / "graph" / "code_graph.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(graph.model_dump(mode="json"), ensure_ascii=False, sort_keys=True),
        encoding="utf-8",
    )
    return path


__all__ = [
    "CodeGraph",
    "Confidence",
    "DeadCodeEntry",
    "GraphEdge",
    "GraphNode",
    "NodeKind",
    "code_graph",
    "git_init_commit",
    "write_graph_artifact",
    "write_sources",
]


def _have_kernel() -> bool:
    from devcouncil.devmap_engine import DevMapEngineError, find_engine_binary

    try:
        find_engine_binary()
    except DevMapEngineError:
        return False
    return True


def kernel_graph(root: Path) -> "CodeGraph":
    """The graph the *kernel* builds over the sources already written under ``root``.

    For consumer tests that keep their source fixtures: the graph comes from the
    real producer (``devmap build`` through ``refresh_map_artifacts``) and is read
    back through ``read_code_graph``, exactly as production does. Skips when no
    kernel binary is built, so the suite stays honest rather than green by
    accident.

    It read back through ``load_code_graph`` until this lane. That imports
    ``code_graph.json`` into the Python ``index.sqlite`` cache on first read, so
    every test using this fixture created and populated a store no production
    consumer reads any more -- and a fixture that keeps the retired path warm is
    how a consumer that regressed onto it would go on passing. The two routes
    were measured to return the same graph on this repository as a corpus
    (1,637 files): identical node, edge and dead-entry counts and identical
    node-id sets.
    """
    import pytest

    from devcouncil.indexing.graph.build import read_code_graph
    from devcouncil.indexing.map_artifacts import refresh_map_artifacts

    if not _have_kernel():
        pytest.skip("devmap kernel not built (cargo build --release -p devmap-cli)")
    root = Path(root)
    (root / ".devcouncil").mkdir(exist_ok=True)
    refresh_map_artifacts(root, root / ".devcouncil" / "repo_map.json", quiet=True)
    graph = read_code_graph(root)
    assert graph is not None, "the kernel wrote no graph the Python side could read"
    return graph


def kernel_client(root: Path):
    """A live ``DevMapClient`` over a store the kernel just built under ``root``.

    The sibling of :func:`kernel_graph` for consumers that have moved off the
    retired Python read path: same producer, same sources, but the answer comes
    back over the client rather than as a whole materialised ``CodeGraph``.
    Nothing here touches ``index.sqlite``.

    Skips when no kernel binary is built, and when the build committed a store
    with nothing in it — ``try_connect`` refuses that case in production too,
    and a test that accepted it would assert against confident zeroes.
    """
    import pytest

    from devcouncil.devmap_client import try_connect
    from devcouncil.indexing.map_artifacts import refresh_map_artifacts

    if not _have_kernel():
        pytest.skip("devmap kernel not built (cargo build --release -p devmap-cli)")
    root = Path(root)
    (root / ".devcouncil").mkdir(exist_ok=True)
    refresh_map_artifacts(root, root / ".devcouncil" / "repo_map.json", quiet=True)
    client = try_connect(root)
    if client is None:
        pytest.skip("the kernel built no usable generation over this fixture tree")
    return client
