"""Project-scoped owner of a root and its runtime evidence.

This class used to own the Python code-intelligence *graph* store: ``persist``
and ``load`` around ``CodeIntelStore.save_graph`` / ``load_graph``, a
generation-keyed ``cached_query``, a ``status`` view of the store, and an
``index_freshness`` probe comparing the committed generation's head against
git HEAD. All five had lost their production callers by the time the Rust
kernel became the only writer of the graph — `write_code_graph` and
`load_code_graph` were the last of them — so the generation they keyed on was
never created and every one of them answered about an empty file.
``index_freshness`` in particular reported ``fresh: None, "no committed index
generation"`` on every repository, which is how a staleness banner comes to be
silent.

What remains is what still has a writer and a reader: the opt-in debug tracer
records runtime observations, and ``run_cypher`` merges them into the kernel's
graph.
"""

from __future__ import annotations

import threading
from pathlib import Path

from devcouncil.codeintel.store import RuntimeEvidenceStore
from devcouncil.indexing.graph.schema import CodeGraph, Confidence, GraphEdge


def canonical_project_root(root: Path) -> Path:
    """Resolve a caller-supplied path to the owning project root.

    Explicit project paths remain authoritative. For a nested path, walk upward
    to the nearest ``.devcouncil`` or ``.git`` marker. This prevents MCP's process
    working directory from silently selecting a different repository.
    """

    resolved = root.expanduser().resolve()
    if resolved.is_file():
        resolved = resolved.parent
    for candidate in (resolved, *resolved.parents):
        if (candidate / ".devcouncil").is_dir() or (candidate / ".git").exists():
            return candidate
    return resolved


class CodeIntelService:
    """Owns one root and its runtime-evidence store."""

    def __init__(self, project_root: Path):
        self.project_root = canonical_project_root(project_root)
        self.store = RuntimeEvidenceStore(self.project_root)

    def merge_runtime_observations(self, graph: CodeGraph) -> CodeGraph:
        """Add this root's fingerprint-matched runtime edges to ``graph``.

        The graph itself is the kernel's — read from ``code_graph.json`` by the
        caller. This used to load it from the Python ``index.sqlite`` store, and
        that store had had no writer since ``write_code_graph`` lost its last
        production caller: every ``run_cypher`` on a real repository answered
        ``"No committed graph generation."``, on both the CLI and the
        ``devcouncil_graph_cypher`` MCP tool. The runtime half of the store is
        still written — by the opt-in debug tracer — so the merge survives the
        graph half and takes the graph as an argument instead of owning it.

        Runtime edges are additive and never overwrite an extracted edge: an
        observation that duplicates a static edge is dropped, so provenance on
        an existing edge cannot be rewritten by a later session. Sampled kinds
        are `INFERRED`, because a stack sample witnesses that a call happened,
        not that the edge is the only one it could have been.
        """
        # Fingerprinting shells out to git (diff + untracked hashing) — only pay
        # that per-query cost when runtime evidence actually exists to match.
        if not self.store.has_runtime_observations():
            return graph
        from devcouncil.codeintel.debug.fingerprint import source_fingerprint

        fingerprint = source_fingerprint(self.project_root)
        runtime = self.store.runtime_observations(source_fingerprint=fingerprint)
        existing = {(edge.source, edge.target, edge.kind) for edge in graph.edges}
        for observation in runtime:
            kind = str(observation["kind"])
            key = (str(observation["source"]), str(observation["target"]), kind)
            if key in existing:
                continue
            sampled = kind in {"sampled_calls", "sampled_stack"}
            graph.edges.append(GraphEdge(
                source=key[0],
                target=key[1],
                kind=kind,
                confidence=Confidence.INFERRED if sampled else Confidence.EXTRACTED,
                reason="fingerprint-matched runtime observation",
                extras={
                    "provenance": "runtime",
                    "confidence_score": 0.75 if sampled else 1.0,
                    "source_fingerprint": fingerprint,
                    "runtime_session": observation["session_id"],
                    "count": observation["count"],
                    "evidence": observation["evidence"],
                },
            ))
            existing.add(key)
        return graph


_SERVICES: dict[Path, CodeIntelService] = {}
_SERVICES_LOCK = threading.Lock()


def get_codeintel_service(root: Path) -> CodeIntelService:
    canonical = canonical_project_root(root)
    with _SERVICES_LOCK:
        service = _SERVICES.get(canonical)
        if service is None:
            service = CodeIntelService(canonical)
            _SERVICES[canonical] = service
        return service
