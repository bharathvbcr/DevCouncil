"""Affected-file graph replacement without global static re-resolution."""

from __future__ import annotations

import logging
from pathlib import Path

from devcouncil.codeintel.resolution import enrich_semantic_edges
from devcouncil.codeintel.service import CodeIntelService
from devcouncil.indexing.graph.build import (
    _code_files,
    _files_fingerprint,
    _git_head,
    content_fingerprint,
    extract_all,
    extract_paths,
    refresh_map_for_paths,
    write_code_graph,
)
from devcouncil.indexing.graph.liveness import (
    build_liveness_shard,
    extraction_from_liveness_shard,
    file_liveness_from_shards,
    legacy_dead_strings,
    symbol_reachability_dead,
    token_dead_from_shards,
)
from devcouncil.indexing.graph.resolve import (
    build_file_and_symbol_nodes,
    contains_and_defines_edges,
    decorator_edges,
    import_graph_edges,
    inherit_edges,
    named_import_edges,
    resolve_calls,
    resolve_import_edges,
)
from devcouncil.indexing.graph.schema import CodeGraph, NodeKind
from devcouncil.indexing.repo_mapper import RepoMap, RepoMapper
from devcouncil.utils.json_persist import read_json, write_model_json

logger = logging.getLogger(__name__)
# Kept as a patch point for existing correctness tests; this is the shard-based
# implementation, not the repository-scanning legacy helper.
_token_scan_dead = token_dead_from_shards


def sync_affected_paths(
    service: CodeIntelService,
    paths: list[str],
    *,
    liveness: bool = True,
) -> CodeGraph:
    """Replace affected rows and dependent resolution as one committed generation."""
    root = service.project_root
    current = service.load()
    if current is None:
        return refresh_map_for_paths(root, paths, liveness=liveness)

    mapper = RepoMapper(root)
    files = mapper.get_git_files()
    code_files = _code_files(files)
    changed = {_normal(path) for path in paths}
    affected = _affected_closure(current, changed, service)
    selected = extract_paths(root, affected)
    shards = service.store.analysis_shards()
    if not shards:
        # One-time migration path for v1 stores. Subsequent edits use persisted
        # shards and never scan unchanged source files.
        migrated = extract_all(root, files, changed_paths=affected)
        shards = {
            path: build_liveness_shard(root, extraction)
            for path, extraction in migrated.items()
        }
    for path in affected:
        shards.pop(path, None)
    shards.update({
        path: build_liveness_shard(root, extraction)
        for path, extraction in selected.items()
    })
    extractions = {
        path: extraction_from_liveness_shard(shard)
        for path, shard in shards.items()
    }
    replacement_nodes, _ = build_file_and_symbol_nodes(
        selected,
        area_fn=mapper._area_for_file,
    )

    old_ids = {node.id for node in current.nodes if node.path in affected}
    live_paths = set(code_files)
    nodes = [
        node for node in current.nodes
        if node.path not in affected and (not node.path or node.path in live_paths)
    ]
    nodes.extend(replacement_nodes)
    symbol_index = _symbol_index(nodes)
    class_ids = {node.id for node in nodes if node.kind == NodeKind.CLASS}

    kept_edges = [
        edge for edge in current.edges
        if edge.source not in old_ids
        and edge.target not in old_ids
        and edge.source not in affected
        and edge.target not in affected
    ]
    old_file_edges = {
        (edge.source, edge.target)
        for edge in kept_edges
        if edge.kind == "imports" and "::" not in edge.source and "::" not in edge.target
    }
    resolved_file_edges = {
        edge for edge in resolve_import_edges(selected, files, root=root, mapper=mapper)
        if edge[0] in affected
    }
    file_edges = sorted(old_file_edges | resolved_file_edges)
    # File-to-file imports are rebuilt from the updated adjacency below.
    # Preserve symbol-level named imports for unchanged files: dropping them
    # makes their targets appear newly dead and expands a one-file write into
    # hundreds or thousands of unrelated liveness payloads.
    kept_edges = [
        edge for edge in kept_edges
        if not (
            edge.kind == "imports"
            and "::" not in edge.source
            and "::" not in edge.target
        )
    ]

    replacement_edges = []
    replacement_edges.extend(contains_and_defines_edges(selected))
    replacement_edges.extend(inherit_edges(selected, symbol_index))
    replacement_edges.extend(decorator_edges(selected, symbol_index))
    replacement_edges.extend(import_graph_edges(file_edges))
    replacement_edges.extend(named_import_edges(selected, symbol_index, file_edges))
    replacement_edges.extend(
        resolve_calls(selected, symbol_index, file_edges, class_ids=class_ids)
    )

    graph = current.model_copy(deep=True)
    graph.nodes = nodes
    graph.edges = kept_edges + replacement_edges
    enrich_semantic_edges(
        graph,
        root=root,
        paths=affected,
        extractions=extractions,
    )

    if liveness:
        entry_roots, unwired, unreachable, unreliable = file_liveness_from_shards(
            files,
            file_edges,
            shards,
            entry_roots=current.entry_roots,
        )
        token_dead, _token_index, token_keys = _token_scan_dead(
            graph.nodes, shards
        )
        graph.dead_code = symbol_reachability_dead(
            root,
            files,
            graph.nodes,
            graph.edges,
            extractions,
            entry_roots,
            token_dead_keys=token_keys,
            file_edges=file_edges,
            unreachable=[] if unreliable else unreachable,
            dynamic_index={},
        )
        graph.entry_roots = entry_roots
        graph.unwired_candidates = unwired
        graph.unreachable_files = unreachable
        graph.meta["liveness_unreachable_unreliable"] = unreliable
        graph.meta["token_dead_count"] = len(token_dead)
        graph.meta["legacy_dead_symbol_candidates"] = legacy_dead_strings(
            graph.dead_code,
            token_dead,
            cap=None,
        )

    try:
        from devcouncil.indexing.graph.intel import enrich_graph_intel

        enrich_graph_intel(graph, root=root)
    except Exception:
        logger.debug("incremental graph-intelligence refresh failed", exc_info=True)

    graph.generated_head = _git_head(root)
    graph.indexed_hash = _files_fingerprint(files)
    graph.content_fingerprint = content_fingerprint(root, files)
    graph.meta.update({
        "incremental": True,
        "changed_paths": sorted(changed),
        "affected_paths": sorted(affected),
        "affected_fraction": len(affected) / max(1, len(code_files)),
        "resolution_scope": "affected",
    })
    previous_dead = {
        (entry.id, entry.confidence.value, entry.reason)
        for entry in current.dead_code
    }
    next_dead = {
        (entry.id, entry.confidence.value, entry.reason)
        for entry in graph.dead_code
    }
    dead_changed_ids = {
        item[0] for item in previous_dead.symmetric_difference(next_dead)
    }
    liveness_changed_paths = {
        node.path for node in graph.nodes
        if node.id in dead_changed_ids and node.path
    } | {
        entry.path for entry in current.dead_code
        if entry.id in dead_changed_ids and entry.path
    }
    persistence_paths = affected | liveness_changed_paths
    graph.meta["liveness_changed_paths"] = sorted(liveness_changed_paths)
    write_code_graph(
        root,
        graph,
        changed_paths=persistence_paths,
        analysis_shards=shards,
    )
    _refresh_repo_map(root, mapper, graph, affected, files)
    return service.load() or graph


def _affected_closure(
    graph: CodeGraph,
    changed: set[str],
    service: CodeIntelService,
) -> set[str]:
    reverse: dict[str, set[str]] = {}
    for edge in graph.edges:
        if edge.kind == "imports" and "::" not in edge.source and "::" not in edge.target:
            reverse.setdefault(edge.target, set()).add(edge.source)
    affected = set(changed)
    queue = list(changed)
    while queue:
        target = queue.pop()
        for importer in reverse.get(target, set()):
            if importer not in affected:
                affected.add(importer)
                queue.append(importer)

    changed_names = {
        node.name for node in graph.nodes
        if node.path in changed
        and node.name
        and node.kind not in {
            NodeKind.FILE,
            NodeKind.DYNAMIC,
            NodeKind.EVENT,
            NodeKind.ROUTE,
            NodeKind.STATE,
            NodeKind.PROVIDER,
        }
    }
    affected.update(
        str(row["path"])
        for row in service.store.unresolved_references()
        if row.get("path") and str(row.get("name") or "") in changed_names
    )
    changed_semantic_kinds = {
        node.kind
        for node in graph.nodes
        if node.path in changed
        and node.kind in {NodeKind.ROUTE, NodeKind.EVENT, NodeKind.PROVIDER}
    }
    if changed_semantic_kinds:
        affected.update(
            node.path for node in graph.nodes
            if node.path and node.kind in changed_semantic_kinds
        )
    return affected


def _symbol_index(nodes) -> dict[str, str]:  # noqa: ANN001
    index: dict[str, str] = {}
    for node in nodes:
        if node.kind == NodeKind.FILE or not node.path:
            continue
        qualname = str(node.extras.get("qualname") or node.id.split("::", 1)[-1].split("#", 1)[0])
        index[f"{node.path}::{qualname}"] = node.id
    return index


def _refresh_repo_map(
    root: Path,
    mapper: RepoMapper,
    graph: CodeGraph,
    affected: set[str],
    files: list[str],
) -> None:
    path = root / ".devcouncil" / "repo_map.json"
    if not path.is_file():
        return
    try:
        repo_map = RepoMap.model_validate(read_json(path))
        by_path = {entry.path: entry for entry in repo_map.files if entry.path not in affected}
        file_set = set(files)
        for rel in affected:
            if rel in file_set and (root / rel).is_file():
                by_path[rel] = mapper.describe_file(rel)
        repo_map.files = [by_path[rel] for rel in sorted(by_path)]
        repo_map.entry_roots = list(graph.entry_roots)
        repo_map.unwired_candidates = list(graph.unwired_candidates)[: mapper._LIVENESS_CAP]
        repo_map.unreachable_files = list(graph.unreachable_files)[: mapper._LIVENESS_CAP]
        repo_map.liveness_unreachable_unreliable = bool(
            graph.meta.get("liveness_unreachable_unreliable")
        )
        repo_map.dead_symbol_candidates = list(
            graph.meta.get("legacy_dead_symbol_candidates") or []
        )[: mapper._LIVENESS_CAP]
        repo_map.generated_head = graph.generated_head
        repo_map.indexed_hash = graph.indexed_hash
        repo_map.content_fingerprint = graph.content_fingerprint
        write_model_json(path, repo_map)
    except Exception:
        logger.warning("incremental repo-map compatibility refresh failed", exc_info=True)


def _normal(path: str) -> str:
    normalized = path.replace("\\", "/")
    return normalized[2:] if normalized.startswith("./") else normalized
