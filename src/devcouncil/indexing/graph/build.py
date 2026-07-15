"""Build / load / incrementally refresh the code knowledge graph (schema v2)."""

from __future__ import annotations

import hashlib
import logging
import weakref
from pathlib import Path
from typing import Any, Dict, Iterable, List, Optional, Set

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
from devcouncil.utils.json_persist import read_json, write_model_json

logger = logging.getLogger(__name__)
_PENDING_ANALYSIS_SHARDS: dict[int, dict[str, dict[str, object]]] = {}

GRAPH_REL = Path(".devcouncil") / "graph" / "code_graph.json"

_CODE_SUFFIXES = {
    extension.lower()
    for spec in LANGUAGE_SPECS
    for extension in spec.extensions
}


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

    return [
        f
        for f in files
        if Path(f).suffix.lower() in _CODE_SUFFIXES and not is_vendored_path(f)
    ]


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

    for rel in code:
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
                continue
        ext, entry = extract_cached(root, rel, cache=cache, force=bool(force))
        extractions[rel] = ext
        updates[rel] = entry

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
) -> CodeGraph:
    """Resolve + liveness over extractions; always full resolve."""
    from devcouncil.indexing.repo_mapper import RepoMapper

    if mapper is None:
        mapper = RepoMapper(root)
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

    semantic_meta: Dict[str, Any] = {}
    try:
        from devcouncil.codeintel.resolution import enrich_semantic_edges

        semantic_graph = CodeGraph(nodes=nodes, edges=edges)
        enrich_semantic_edges(semantic_graph, root=root, extractions=extractions)
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
        entry_roots, unwired, unreachable, unreachable_unreliable = file_liveness(
            root, files, file_edges, cap=0
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
) -> CodeGraph:
    """Full (or incremental-extract) graph build."""
    from devcouncil.indexing.repo_mapper import RepoMapper

    root = root.expanduser().resolve()
    if mapper is None:
        mapper = RepoMapper(root)
    if files is None:
        files = mapper.get_git_files()
    changed = {p.replace("\\", "/") for p in changed_paths} if changed_paths else None
    extractions = extract_all(root, files, changed_paths=changed)
    graph = assemble_graph(
        root,
        files,
        extractions,
        liveness=liveness,
        lsp_refs=lsp_refs,
        mapper=mapper,
    )
    _PENDING_ANALYSIS_SHARDS[id(graph)] = {
        path: build_liveness_shard(root, extraction)
        for path, extraction in extractions.items()
    }
    weakref.finalize(graph, _PENDING_ANALYSIS_SHARDS.pop, id(graph), None)
    return graph


def write_code_graph(
    root: Path,
    graph: CodeGraph,
    *,
    changed_paths: Set[str] | None = None,
    analysis_shards: dict[str, dict[str, object]] | None = None,
) -> Path:
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
    write_model_json(path, graph)
    get_codeintel_service(root).store.record_compatibility_export(path, graph)
    return path


def load_code_graph(root: Path) -> Optional[CodeGraph]:
    from devcouncil.codeintel import get_codeintel_service

    root = root.expanduser().resolve()
    path = graph_path(root)
    service = get_codeintel_service(root)
    # Compatibility clients may still replace the documented JSON artifact.
    # The export handshake distinguishes an external replacement from our own
    # post-commit JSON write without repeatedly parsing a large artifact.
    try:
        recorded_digest, recorded_mtime = service.store.compatibility_export_state()
        if path.is_file() and path.stat().st_mtime_ns != recorded_mtime:
            data = read_json(path)
            exported = CodeGraph.model_validate(data)
            from devcouncil.codeintel.store.sqlite import compatibility_graph_digest

            if not service.store.exists() or compatibility_graph_digest(exported) != recorded_digest:
                service.persist(exported)
                service.store.record_compatibility_export(path, exported)
                return exported
            service.store.record_compatibility_export(path, exported)
    except Exception:
        logger.debug("Failed to import newer compatibility graph export", exc_info=True)
    try:
        graph = service.load()
        if graph is not None:
            return graph
    except Exception:
        # A corrupt/newer database must not strand users who still have the
        # versioned JSON export. ``dev graph doctor`` reports the store failure;
        # compatibility reads remain available until it is repaired/rebuilt.
        logger.debug("Failed to load canonical code-intelligence store", exc_info=True)
    if not path.is_file():
        return None
    try:
        data = read_json(path)
        return CodeGraph.model_validate(data)
    except Exception:
        logger.debug("Failed to load code graph", exc_info=True)
        return None


def refresh_map_for_paths(
    root: Path,
    paths: List[str],
    *,
    liveness: bool = True,
) -> CodeGraph:
    """Re-extract only ``paths``, full re-resolve, rewrite graph + repo_map artifacts.

    When subsystem topology (area set) changes, also rewrites marker-guarded
    ``AGENTS.md`` / ``CLAUDE.md`` and re-stamps map fingerprints.

    Target: warm refresh well under 1s on this repo's size.
    """
    from devcouncil.indexing.repo_mapper import RepoMapper
    from devcouncil.utils.json_persist import read_json, write_model_json

    root = root.expanduser().resolve()
    norm_paths = [
        p.replace("\\", "/")[2:] if p.replace("\\", "/").startswith("./") else p.replace("\\", "/")
        for p in paths
    ]
    out = root / ".devcouncil" / "repo_map.json"
    prev_areas: set[str] = set()
    if out.is_file():
        try:
            prev = read_json(out) or {}
            prev_areas = {
                str(s.get("area") or "")
                for s in (prev.get("subsystems") or [])
                if isinstance(s, dict) and s.get("area")
            }
        except Exception:
            prev_areas = set()

    mapper = RepoMapper(root)
    # Facade map_repo builds + writes the graph; pass changed paths via env-ish attr.
    mapper._graph_changed_paths = set(norm_paths)  # type: ignore[attr-defined]
    repo_map = mapper.map_repo(liveness=liveness)

    out.parent.mkdir(parents=True, exist_ok=True)
    write_model_json(out, repo_map)

    new_areas = {s.area for s in repo_map.subsystems if s.area}
    if new_areas != prev_areas:
        try:
            from devcouncil.cli.commands.map import _write_agent_guides

            _write_agent_guides(root, out, repo_map)
            files = mapper.get_git_files()
            repo_map.generated_head = mapper._git_head()
            repo_map.indexed_hash = mapper._files_fingerprint(files)
            repo_map.content_fingerprint = mapper._content_fingerprint(files)
            write_model_json(out, repo_map)
        except Exception:
            logger.debug("agent guide rewrite after topology change failed", exc_info=True)

    return load_code_graph(root) or build_code_graph(root, liveness=liveness)
