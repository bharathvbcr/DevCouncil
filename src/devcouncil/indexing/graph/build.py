"""Build / load / incrementally refresh the code knowledge graph (schema v2)."""

from __future__ import annotations

import ast
import hashlib
import json
import logging
import os
import tempfile
import weakref
from pathlib import Path
from typing import Any, Callable, Dict, Iterable, List, Optional, Set

from devcouncil.codeintel.languages.registry import LANGUAGE_SPECS
from devcouncil.indexing.graph.cache import (
    PARSE_CACHE_VERSION,
    extract_cached,
    load_parse_cache,
    merge_parse_cache,
)
from devcouncil.indexing.graph.extract_python import FileExtraction
from devcouncil.indexing.graph.liveness import (
    build_liveness_shard,
    file_liveness,
    legacy_dead_strings,
    symbol_reachability_dead,
    _token_scan_dead,
)
from devcouncil.indexing.graph.resolve import (
    build_file_and_symbol_nodes,
    contains_and_defines_edges,
    decorator_edges,
    inherit_edges,
    named_import_edges,
    resolve_calls,
    resolve_import_edges,
    import_graph_edges,
)
from devcouncil.indexing.graph.schema import NodeKind
from devcouncil.indexing.graph.schema import CodeGraph, SCHEMA_VERSION
from devcouncil.utils.json_persist import read_json

logger = logging.getLogger(__name__)
_PENDING_ANALYSIS_SHARDS: dict[int, dict[str, dict[str, object]]] = {}

GRAPH_REL = Path(".devcouncil") / "graph" / "code_graph.json"

_CODE_SUFFIXES = {
    extension.lower()
    for spec in LANGUAGE_SPECS
    for extension in spec.extensions
}


class CompatibilityGraphTooLarge(ValueError):
    """Compatibility JSON exceeded the configured import/export boundary."""


def graph_path(root: Path) -> Path:
    return root / GRAPH_REL


def content_fingerprint(root: Path, files: List[str]) -> str:
    """sha1 over sorted ``(path, size, mtime_ns)`` so content edits mark staleness."""
    lines: List[str] = []
    for rel in sorted(files):
        try:
            st = (root / rel).stat()
            lines.append(f"{rel}\0{st.st_size}\0{st.st_mtime_ns}")
        except OSError:
            lines.append(f"{rel}\0-1\0-1")
    return hashlib.sha1("\n".join(lines).encode("utf-8")).hexdigest()


def _git_head(root: Path) -> str:
    from devcouncil.utils.proc import git_output

    return git_output(["rev-parse", "HEAD"], cwd=root, default="").strip()


def _files_fingerprint(files: List[str]) -> str:
    return hashlib.sha1("\n".join(sorted(files)).encode("utf-8")).hexdigest()


def _code_files(files: Iterable[str]) -> List[str]:
    from devcouncil.indexing.wiring import is_vendored_path

    return sorted(
        f
        for f in files
        if Path(f).suffix.lower() in _CODE_SUFFIXES and not is_vendored_path(f)
    )


def _area_fn_for(root: Path, mapper: Optional[Any] = None):
    """Prefer RepoMapper area bucketing; reuse one shared mapper instance."""

    def _area(path: str) -> str:
        try:
            if mapper is not None:
                return str(mapper._area_for_file(path))
            from devcouncil.indexing.repo_mapper import RepoMapper

            return RepoMapper(root)._area_for_file(path)
        except Exception:
            parts = path.replace("\\", "/").split("/")
            return parts[0] if parts else "root"

    return _area


def _cache_entry_matches_disk(root: Path, rel: str, entry: dict) -> bool:
    """True when cached sha256 (and size/mtime when present) still match disk."""
    path = root / rel
    try:
        st = path.stat()
    except OSError:
        return False
    if "size" in entry and entry["size"] is not None:
        try:
            if int(entry["size"]) != st.st_size:
                return False
        except (TypeError, ValueError):
            return False
    if "mtime_ns" in entry and entry["mtime_ns"] is not None:
        try:
            if int(entry["mtime_ns"]) != st.st_mtime_ns:
                return False
        except (TypeError, ValueError):
            return False
    stored = entry.get("sha256")
    if not isinstance(stored, str) or not stored:
        return False
    try:
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
    except OSError:
        return False
    return stored == digest


def extract_all(
    root: Path,
    files: List[str],
    *,
    changed_paths: Optional[Set[str]] = None,
    progress: Callable[[str, int, int], None] | None = None,
) -> Dict[str, FileExtraction]:
    """Extract (cache v5) all code files; only force-reparse ``changed_paths`` when set.

    Incremental cache hits still verify sha256 (and size/mtime if stored) so a
    concurrent edit to a non-listed path cannot stamp a fresh fingerprint over
    stale symbols. Cache-hit entries stay in ``updates`` so merge does not evict them.
    """
    code = _code_files(files)
    cache = load_parse_cache(root)
    extractions: Dict[str, FileExtraction] = {}
    updates: Dict[str, dict] = {}
    force_set = changed_paths if changed_paths is not None else None

    for index, rel in enumerate(code, start=1):
        if progress is not None:
            progress("extract", index - 1, len(code))
        force = force_set is not None and rel in force_set
        # When incremental: prefer warm cache for non-listed paths, but never
        # trust an entry whose digests no longer match disk.
        if force_set is not None and rel not in force_set:
            entry = cache.get(rel)
            if (
                isinstance(entry, dict)
                and isinstance(entry.get("symbols"), list)
                and isinstance(entry.get("import_details"), list)
                and _cache_entry_matches_disk(root, rel, entry)
            ):
                from devcouncil.indexing.graph.cache import extraction_from_cache_entry

                extractions[rel] = extraction_from_cache_entry(rel, entry)
                # Keep cache-hit entries in ``updates`` — ``merge_parse_cache``
                # prunes managed paths absent from updates, so omitting them
                # would evict every unchanged file from the warm cache.
                updates[rel] = entry
                if progress is not None:
                    progress("extract", index, len(code))
                continue
        ext, entry = extract_cached(root, rel, cache=cache, force=bool(force))
        extractions[rel] = ext
        updates[rel] = entry

        if progress is not None:
            progress("extract", index, len(code))

    managed = {k for k in cache if Path(k).suffix.lower() in _CODE_SUFFIXES} | set(code)
    if updates:
        merge_parse_cache(root, updates, managed)
    return extractions


def extract_paths(
    root: Path,
    paths: Iterable[str],
) -> Dict[str, FileExtraction]:
    """Extract only existing changed paths and evict deleted cache entries."""
    selected = {
        path.replace("\\", "/")
        for path in paths
        if Path(path).suffix.lower() in _CODE_SUFFIXES
    }
    cache = load_parse_cache(root)
    updates: Dict[str, dict] = {}
    extractions: Dict[str, FileExtraction] = {}
    for rel in sorted(selected):
        if not (root / rel).is_file():
            continue
        extraction, entry = extract_cached(root, rel, cache=cache, force=True)
        extractions[rel] = extraction
        updates[rel] = entry
    merge_parse_cache(root, updates, selected)
    return extractions


def assemble_graph(
    root: Path,
    files: List[str],
    extractions: Dict[str, FileExtraction],
    *,
    liveness: bool = True,
    lsp_refs: bool = False,
    mapper: Optional[Any] = None,
    progress: Callable[[str, int, int], None] | None = None,
) -> CodeGraph:
    """Resolve + liveness over extractions; always full resolve."""
    from devcouncil.indexing.repo_mapper import RepoMapper

    if mapper is None:
        mapper = RepoMapper(root)
    if progress is not None:
        progress("assemble_nodes", 0, 1)
    area_fn = _area_fn_for(root, mapper=mapper)
    nodes, symbol_index = build_file_and_symbol_nodes(extractions, area_fn=area_fn)
    class_ids = {n.id for n in nodes if n.kind == NodeKind.CLASS}
    edges = []
    edges.extend(contains_and_defines_edges(extractions))
    edges.extend(inherit_edges(extractions, symbol_index))
    edges.extend(decorator_edges(extractions, symbol_index))

    file_edges = resolve_import_edges(extractions, files, root=root, mapper=mapper)
    edges.extend(import_graph_edges(file_edges))
    edges.extend(named_import_edges(extractions, symbol_index, file_edges))
    edges.extend(
        resolve_calls(extractions, symbol_index, file_edges, class_ids=class_ids)
    )
    if progress is not None:
        progress("assemble_nodes", 1, 1)

    semantic_meta: Dict[str, Any] = {}
    try:
        from devcouncil.codeintel.resolution import enrich_semantic_edges

        enrich_budget = 120.0
        try:
            from devcouncil.app.config import load_config

            enrich_budget = float(
                load_config(root).indexing.semantic_enrich_timeout_seconds
            )
        except Exception:
            logger.debug("semantic enrich budget config load failed", exc_info=True)

        semantic_graph = CodeGraph(nodes=nodes, edges=edges)
        enrich_semantic_edges(
            semantic_graph,
            root=root,
            extractions=extractions,
            progress=progress,
            budget_seconds=enrich_budget,
        )
        nodes = semantic_graph.nodes
        edges = semantic_graph.edges
        semantic_meta = semantic_graph.meta
    except Exception:
        logger.debug("semantic graph enrichment failed", exc_info=True)

    entry_roots: List[str] = []
    unwired: List[str] = []
    unreachable: List[str] = []
    unreachable_unreliable = False
    dead_code = []
    if liveness:
        if progress is not None:
            progress("liveness", 0, 1)
        from devcouncil.indexing.graph.liveness import project_call_edges_to_files

        liveness_edges = sorted(set(file_edges) | project_call_edges_to_files(edges))
        entry_roots, unwired, unreachable, unreachable_unreliable = file_liveness(
            root, files, liveness_edges, cap=0
        )
        # Empty prod roots → fail-soft empty unreachable (already); keep symbol
        # reachability from treating every file as unreachable.
        reach_unreachable = [] if unreachable_unreliable else unreachable
        token_dead, _idx, token_keys = _token_scan_dead(
            root, files, cap=0, lsp_refs=lsp_refs, mapper=mapper
        )
        dead_code = symbol_reachability_dead(
            root,
            files,
            nodes,
            edges,
            extractions,
            entry_roots,
            token_dead_keys=token_keys,
            file_edges=file_edges,
            unreachable=reach_unreachable,
        )
        # Uncapped in code_graph meta; repo_map.json applies _LIVENESS_CAP on write.
        legacy = legacy_dead_strings(dead_code, token_dead, cap=None)
        if progress is not None:
            progress("liveness", 1, 1)
    else:
        legacy = []
        token_dead = []

    graph = CodeGraph(
        schema_version=SCHEMA_VERSION,
        nodes=nodes,
        edges=edges,
        dead_code=dead_code,
        entry_roots=entry_roots,
        unwired_candidates=unwired,
        unreachable_files=unreachable,
        generated_head=_git_head(root),
        indexed_hash=_files_fingerprint(files),
        content_fingerprint=content_fingerprint(root, files),
        meta={
            "parse_cache_version": PARSE_CACHE_VERSION,
            "legacy_dead_symbol_candidates": legacy,
            "file_edge_count": len(file_edges),
            "token_dead_count": len(token_dead) if liveness else 0,
            "liveness_unreachable_unreliable": unreachable_unreliable,
            **semantic_meta,
        },
    )
    try:
        from devcouncil.indexing.graph.intel import enrich_graph_intel

        enrich_graph_intel(graph, root=root)
    except Exception:
        logger.debug("graph intel enrichment failed", exc_info=True)
    return graph


def build_code_graph(
    root: Path,
    files: Optional[List[str]] = None,
    *,
    changed_paths: Optional[Iterable[str]] = None,
    liveness: bool = True,
    lsp_refs: bool = False,
    mapper: Optional[Any] = None,
    progress: Callable[[str, int, int], None] | None = None,
) -> CodeGraph:
    """Full (or incremental-extract) graph build."""
    from devcouncil.indexing.repo_mapper import RepoMapper

    root = root.expanduser().resolve()
    if mapper is None:
        mapper = RepoMapper(root)
    if files is None:
        files = mapper.get_git_files()
    changed = {p.replace("\\", "/") for p in changed_paths} if changed_paths else None
    extractions = extract_all(root, files, changed_paths=changed, progress=progress)
    graph = assemble_graph(
        root,
        files,
        extractions,
        liveness=liveness,
        lsp_refs=lsp_refs,
        mapper=mapper,
        progress=progress,
    )
    _PENDING_ANALYSIS_SHARDS[id(graph)] = {
        path: build_liveness_shard(root, extraction)
        for path, extraction in extractions.items()
    }
    weakref.finalize(graph, _PENDING_ANALYSIS_SHARDS.pop, id(graph), None)
    return graph


def _slim_graph_export(graph: CodeGraph) -> CodeGraph:
    """Copy for JSON export: drop bulky meta already recoverable elsewhere.

    ``node_communities`` duplicates per-node ``community``; legacy dead strings
    remain uncapped on the in-memory/SQLite graph for repo_map consumers.
    Volatile PageRank floats are stripped from ``god_nodes`` (degree/fan metrics
    remain); incremental bookkeeping keys stay but no longer re-inflate via
    pretty-indent or duplicated community maps.
    """
    meta = dict(graph.meta or {})
    meta.pop("node_communities", None)
    meta.pop("legacy_dead_symbol_candidates", None)
    gods = meta.get("god_nodes")
    if isinstance(gods, list):
        cleaned: list[dict[str, object]] = []
        for row in gods:
            if not isinstance(row, dict):
                continue
            item = dict(row)
            item.pop("pagerank", None)
            cleaned.append(item)
        meta["god_nodes"] = cleaned
    meta["compatibility_export_tier"] = "slim"
    return graph.model_copy(update={"meta": meta})


def _compact_graph_export(graph: CodeGraph) -> CodeGraph:
    """Aggressive slim: strip node/edge extras and cap noisy liveness lists."""
    slim = _slim_graph_export(graph)
    nodes = [
        n.model_copy(update={"extras": {}})
        for n in slim.nodes
    ]
    edges = [
        e.model_copy(update={"extras": {}, "reason": ""})
        for e in slim.edges
    ]
    meta = dict(slim.meta or {})
    meta["compatibility_export_tier"] = "compact"
    meta["unreachable_omitted"] = len(slim.unreachable_files or [])
    meta["unwired_omitted"] = max(0, len(slim.unwired_candidates or []) - 200)
    return slim.model_copy(
        update={
            "nodes": nodes,
            "edges": edges,
            "unreachable_files": [],
            "unwired_candidates": list(slim.unwired_candidates or [])[:200],
            "dead_code": list(slim.dead_code or [])[:500],
            "meta": meta,
        }
    )


def _stub_graph_export(graph: CodeGraph) -> CodeGraph:
    """Pointer-only compatibility JSON when even compact export exceeds the limit."""
    return CodeGraph(
        schema_version=graph.schema_version,
        nodes=[],
        edges=[],
        dead_code=[],
        entry_roots=list(graph.entry_roots or [])[:200],
        unwired_candidates=list(graph.unwired_candidates or [])[:100],
        unreachable_files=[],
        generated_head=graph.generated_head,
        indexed_hash=graph.indexed_hash,
        content_fingerprint=graph.content_fingerprint,
        meta={
            "compatibility_export_tier": "stub",
            "sqlite_canonical": True,
            "node_count": len(graph.nodes),
            "edge_count": len(graph.edges),
            "dead_code_count": len(graph.dead_code or []),
            "unwired_count": len(graph.unwired_candidates or []),
            "unreachable_count": len(graph.unreachable_files or []),
            "liveness_unreachable_unreliable": bool(
                (graph.meta or {}).get("liveness_unreachable_unreliable")
            ),
            "compatibility_export_reason": (
                "exceeded indexing.graph_json_max_bytes; prefer SQLite-backed "
                "`dev graph` commands"
            ),
        },
    )


def _graph_json_indent(root: Path) -> int | None:
    """Compact JSON by default; honor ``indexing.compact_graph_json``."""
    try:
        from devcouncil.app.config import load_config

        if bool(load_config(root).indexing.compact_graph_json):
            return None
        return 2
    except Exception:
        return None


def _graph_json_max_bytes(root: Path) -> int:
    try:
        from devcouncil.app.config import load_config

        return int(load_config(root).indexing.graph_json_max_bytes)
    except Exception:
        return 128 * 1024 * 1024


def _write_graph_json_bounded(
    path: Path,
    graph: CodeGraph,
    *,
    indent: int | None,
    max_bytes: int,
) -> None:
    """Stream JSON to a sibling temporary file and atomically publish it."""
    path.parent.mkdir(parents=True, exist_ok=True)
    encoder = json.JSONEncoder(
        indent=indent,
        ensure_ascii=False,
        separators=(",", ":") if indent is None else None,
    )
    payload = graph.model_dump(mode="json")
    fd, raw_temp = tempfile.mkstemp(prefix=f".{path.name}.", suffix=".tmp", dir=path.parent)
    temp = Path(raw_temp)
    written = 0
    try:
        os.chmod(temp, (path.stat().st_mode & 0o777) if path.exists() else 0o644)
        with os.fdopen(fd, "wb") as handle:
            for chunk in encoder.iterencode(payload):
                encoded = chunk.encode("utf-8")
                written += len(encoded)
                if written + 1 > max_bytes:
                    raise CompatibilityGraphTooLarge(
                        f"compatibility graph export exceeds {max_bytes} bytes"
                    )
                handle.write(encoded)
            handle.write(b"\n")
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temp, path)
    finally:
        temp.unlink(missing_ok=True)


def _write_compatibility_export_tiers(root: Path, path: Path, graph: CodeGraph) -> str:
    """Try slim → compact → stub until the size cap fits. Returns the tier used.

    Raises ``CompatibilityGraphTooLarge`` only when even the stub cannot be written
    (misconfigured tiny limit). When the stub tier is used, still raises after a
    successful write so callers mark ``compatibility_export=degraded`` while
    leaving a usable pointer JSON on disk.
    """
    from devcouncil.codeintel import get_codeintel_service

    indent = _graph_json_indent(root)
    max_bytes = _graph_json_max_bytes(root)
    # The stub is a bounded pointer JSON (~1 KB floor plus capped entry lists);
    # always leave it on disk even under a tiny configured cap, backstopped at
    # 1 MiB against pathological entry-root lists.
    stub_cap = max(max_bytes, 1024 * 1024)
    tiers: list[tuple[str, CodeGraph, int]] = [
        ("slim", _slim_graph_export(graph), max_bytes),
        ("compact", _compact_graph_export(graph), max_bytes),
        ("stub", _stub_graph_export(graph), stub_cap),
    ]
    last_err: CompatibilityGraphTooLarge | None = None
    for tier_name, export_graph, tier_cap in tiers:
        try:
            _write_graph_json_bounded(
                path, export_graph, indent=indent, max_bytes=tier_cap
            )
        except CompatibilityGraphTooLarge as exc:
            last_err = exc
            logger.warning(
                "compatibility export tier %s exceeded %s bytes; trying next",
                tier_name,
                max_bytes,
            )
            continue
        get_codeintel_service(root).store.record_compatibility_export(path, export_graph)
        if tier_name == "stub":
            raise CompatibilityGraphTooLarge(
                f"compatibility graph export exceeded {max_bytes} bytes; "
                "wrote stub JSON (SQLite remains canonical)"
            )
        return tier_name
    raise last_err or CompatibilityGraphTooLarge(
        f"compatibility graph export exceeds {max_bytes} bytes"
    )


def write_code_graph(
    root: Path,
    graph: CodeGraph,
    *,
    changed_paths: Set[str] | None = None,
    analysis_shards: dict[str, dict[str, object]] | None = None,
    _lease_held: bool = False,
) -> Path:
    if not _lease_held:
        from devcouncil.codeintel.build_control import graph_build_session

        with graph_build_session(root):
            return write_code_graph(
                root,
                graph,
                changed_paths=changed_paths,
                analysis_shards=analysis_shards,
                _lease_held=True,
            )
    # SQLite is the canonical store. Keep the deterministic JSON artifact as a
    # compatibility/export boundary for existing consumers and older clients.
    from devcouncil.codeintel import get_codeintel_service

    root = root.expanduser().resolve()
    shards = analysis_shards or _PENDING_ANALYSIS_SHARDS.pop(id(graph), None)
    get_codeintel_service(root).persist(
        graph,
        changed_paths=changed_paths,
        analysis_shards=shards,
    )
    path = graph_path(root)
    path.parent.mkdir(parents=True, exist_ok=True)
    _write_compatibility_export_tiers(root, path, graph)
    return path


def export_code_graph_json(root: Path) -> Optional[Path]:
    """Rewrite the compatibility ``code_graph.json`` from the canonical store.

    Self-heal for a deleted or failed JSON export: loads from SQLite only (no
    re-persist) and rewrites the artifact + export handshake. Returns the path,
    or None when the store has no graph or the write fails.
    """
    from devcouncil.codeintel import get_codeintel_service

    root = root.expanduser().resolve()
    try:
        service = get_codeintel_service(root)
        graph = service.load()
        if graph is None:
            return None
        path = graph_path(root)
        path.parent.mkdir(parents=True, exist_ok=True)
        try:
            _write_compatibility_export_tiers(root, path, graph)
        except CompatibilityGraphTooLarge:
            # Stub may still be on disk — treat as healed when the file exists.
            if not path.is_file():
                raise
        return path
    except Exception:
        logger.warning("code graph JSON re-export failed", exc_info=True)
        return None


def load_code_graph(root: Path) -> Optional[CodeGraph]:
    from devcouncil.codeintel import get_codeintel_service

    root = root.expanduser().resolve()
    path = graph_path(root)
    service = get_codeintel_service(root)
    # SQLite is canonical. Import the JSON compatibility artifact only when the
    # store is empty/missing — never let an external JSON rewrite clobber a
    # committed generation (accidental or malicious mtime/digest churn).
    try:
        if not service.store.exists() and path.is_file():
            if path.stat().st_size > _graph_json_max_bytes(root):
                raise CompatibilityGraphTooLarge(
                    f"compatibility graph import exceeds {_graph_json_max_bytes(root)} bytes"
                )
            data = read_json(path)
            exported = CodeGraph.model_validate(data)
            # Import is a store write: hold the writer lease like every other
            # persist path. GraphBuildBusy lands in the except below and the
            # read falls through to whatever generation the builder commits.
            from devcouncil.codeintel.build_control import graph_build_session

            with graph_build_session(root):
                service.persist(exported)
                service.store.record_compatibility_export(path, exported)
            return _annotate_graph_degraded(root, exported)
        recorded_digest, recorded_mtime = service.store.compatibility_export_state()
        if (
            path.is_file()
            and recorded_mtime is not None
            and path.stat().st_mtime_ns != recorded_mtime
        ):
            # Refresh the handshake when the on-disk export matches the store;
            # otherwise leave SQLite authoritative and let ``dev graph doctor``
            # / export self-heal report drift.
            from devcouncil.codeintel.store.sqlite import compatibility_graph_digest

            if path.stat().st_size <= _graph_json_max_bytes(root):
                data = read_json(path)
                exported = CodeGraph.model_validate(data)
                if compatibility_graph_digest(exported) == recorded_digest:
                    service.store.record_compatibility_export(path, exported)
                else:
                    logger.info(
                        "ignoring external compatibility graph that diverges from "
                        "canonical store (sqlite wins); re-export with "
                        "`dev graph export` / map refresh if JSON must catch up"
                    )
    except Exception:
        logger.debug("Failed to reconcile compatibility graph export", exc_info=True)
    try:
        graph = service.load()
        if graph is not None:
            return _annotate_graph_degraded(root, graph)
    except Exception:
        # A corrupt/newer database must not strand users who still have the
        # versioned JSON export. ``dev graph doctor`` reports the store failure;
        # compatibility reads remain available until it is repaired/rebuilt.
        logger.debug("Failed to load canonical code-intelligence store", exc_info=True)
    if not path.is_file():
        return None
    try:
        if path.stat().st_size > _graph_json_max_bytes(root):
            raise CompatibilityGraphTooLarge(
                f"compatibility graph import exceeds {_graph_json_max_bytes(root)} bytes"
            )
        data = read_json(path)
        return _annotate_graph_degraded(root, CodeGraph.model_validate(data))
    except Exception:
        logger.debug("Failed to load code graph", exc_info=True)
        return None


def _annotate_graph_degraded(root: Path, graph: CodeGraph) -> CodeGraph:
    """Surface repo_map lean/degraded handshake on graph payloads for consumers."""
    map_path = root / ".devcouncil" / "repo_map.json"
    if not map_path.is_file():
        return graph
    try:
        data = read_json(map_path)
        if not isinstance(data, dict) or not data.get("graph_degraded"):
            return graph
        graph.meta["graph_degraded"] = True
        graph.meta["graph_degraded_reason"] = str(data.get("graph_degraded_reason") or "")
    except Exception:
        logger.debug("graph_degraded annotation failed", exc_info=True)
    return graph


def refresh_map_for_paths(
    root: Path,
    paths: List[str],
    *,
    liveness: bool = True,
    fail_on_degraded: bool = True,
) -> CodeGraph | None:
    """Refresh graph and repo map once; never retry a failed graph assembly.

    When ``fail_on_degraded`` is True (default for watch/sync), a lean/degraded
    refresh raises so ``sync_now`` returns False and pending work is not cleared
    as healthy. One-shot CLI callers that intentionally accept lean maps should
    pass ``fail_on_degraded=False`` or call ``refresh_map_artifacts`` directly.
    """
    from devcouncil.indexing.map_artifacts import refresh_map_artifacts

    root = root.expanduser().resolve()
    result = refresh_map_artifacts(
        root,
        root / ".devcouncil" / "repo_map.json",
        paths=paths,
        liveness=liveness,
        quiet=True,
    )
    if fail_on_degraded and result.degraded:
        raise RuntimeError(result.reason or "graph refresh degraded")
    return result.graph


# --- Opt-in PDG layer (CFG / reaching-def / CDG / taint) ---


def build_pdg_for_paths(
    root: Path,
    graph: CodeGraph,
    *,
    paths: Optional[Iterable[str]] = None,
):
    """Analyze Python files and return a PDG layer."""
    from devcouncil.indexing.graph.pdg.cdg import build_cdg
    from devcouncil.indexing.graph.pdg.cfg import build_cfg_for_function
    from devcouncil.indexing.graph.pdg.reaching_def import compute_reaching_defs
    from devcouncil.indexing.graph.pdg.schema import FilePDG, FunctionPDG, PDGLayer, PDG_VERSION
    from devcouncil.indexing.graph.pdg.taint import analyze_taint

    root = root.expanduser().resolve()
    if paths is None:
        paths = sorted({n.path for n in graph.nodes if n.path.endswith(".py")})

    def _python_functions(tree: ast.AST) -> List[tuple[str, ast.AST]]:
        out: List[tuple[str, ast.AST]] = []
        if isinstance(tree, ast.Module):
            for node in tree.body:
                if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
                    out.append((node.name, node))
                elif isinstance(node, ast.ClassDef):
                    for item in node.body:
                        if isinstance(item, (ast.FunctionDef, ast.AsyncFunctionDef)):
                            out.append((f"{node.name}.{item.name}", item))
        seen: Set[str] = set()
        unique: List[tuple[str, ast.AST]] = []
        for qual, fn in out:
            if qual in seen:
                continue
            seen.add(qual)
            unique.append((qual, fn))
        return unique

    layer = PDGLayer(version=PDG_VERSION)
    for raw in paths:
        rel = raw.replace("\\", "/")
        path = root / rel
        if path.suffix.lower() != ".py":
            continue
        try:
            source = path.read_text(encoding="utf-8")
            tree = ast.parse(source, filename=rel)
        except (OSError, SyntaxError, UnicodeDecodeError):
            logger.debug("PDG skip %s", rel, exc_info=True)
            continue
        lines = source.splitlines()
        functions: List[FunctionPDG] = []
        for qualname, fn_node in _python_functions(tree):
            start_line = int(getattr(fn_node, "lineno", 0) or 0)
            end_line = int(getattr(fn_node, "end_lineno", start_line) or start_line)
            cfg = build_cfg_for_function(rel, qualname, fn_node, lines)
            reaching = compute_reaching_defs(cfg, fn_node)
            cdg = build_cdg(cfg, fn_node)
            taint = analyze_taint(rel, qualname, fn_node, reaching)
            functions.append(
                FunctionPDG(
                    path=rel,
                    qualname=qualname,
                    start_line=start_line,
                    end_line=end_line,
                    blocks=cfg.blocks,
                    cfg_edges=cfg.edges,
                    reaching_def=reaching,
                    cdg=cdg,
                    taint=taint,
                )
            )
        if functions:
            file_pdg = FilePDG(path=rel, language="python", functions=functions)
            layer.files[rel] = file_pdg
            for fn in functions:
                layer.taint_findings.extend(fn.taint)
    return layer

def merge_pdg_into_graph(graph: CodeGraph, layer) -> Dict[str, dict[str, object]]:
    """Persist PDG summary in graph.meta and return analysis shards."""
    graph.meta["pdg"] = layer.to_meta()
    shards: Dict[str, dict[str, object]] = {}
    for path in layer.files:
        payload = layer.shard_payload(path)
        if payload is not None:
            shards[path] = payload
    return shards


def load_pdg_layer(graph: CodeGraph):
    from devcouncil.indexing.graph.pdg.schema import PDGLayer, PDG_VERSION, TaintFinding

    raw = graph.meta.get("pdg")
    if not isinstance(raw, dict):
        return None
    layer = PDGLayer(version=int(raw.get("version") or PDG_VERSION))
    for item in raw.get("taint_findings") or []:
        if isinstance(item, dict):
            layer.taint_findings.append(TaintFinding.from_dict(item))
    return layer
