"""Affected-file graph replacement without global static re-resolution."""

from __future__ import annotations

import logging
from dataclasses import asdict
from pathlib import Path

from devcouncil.codeintel.resolution import enrich_semantic_edges
from devcouncil.codeintel.service import CodeIntelService
from devcouncil.codeintel.build_control import (
    graph_build_session,
    record_inline_build_status,
    run_isolated_full_build,
)
from devcouncil.indexing.graph.build import (
    _CODE_SUFFIXES,
    _code_files,
    _files_fingerprint,
    _git_head,
    content_fingerprint,
    extract_all,
    extract_paths,
    write_code_graph,
)
from devcouncil.indexing.graph.liveness import (
    build_liveness_shard,
    dynamic_import_index_from_shards,
    extraction_from_liveness_shard,
    file_liveness_from_shards,
    legacy_dead_strings,
    project_call_edges_to_files,
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
from devcouncil.indexing.map_refresh import refresh_repo_map_from_graph
from devcouncil.indexing.repo_mapper import RepoMapper

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
    with graph_build_session(service.project_root):
        return _sync_affected_paths_locked(service, paths, liveness=liveness)


def _sync_affected_paths_locked(
    service: CodeIntelService,
    paths: list[str],
    *,
    liveness: bool,
) -> CodeGraph:
    root = service.project_root
    current = service.load()
    if current is None:
        result = run_isolated_full_build(
            root,
            changed_paths={_normal(path) for path in paths},
            liveness=liveness,
        )
        return result.graph
    previous_communities = {
        (node.id, node.path, node.line, node.kind.value): node.community
        for node in current.nodes
    }

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
    live_paths = set(code_files)
    stale_shard_paths = set(shards) - live_paths
    shards = {
        path: shard
        for path, shard in shards.items()
        if path in live_paths
    }
    code_changed = {
        path for path in changed
        if path in live_paths or path in shards or Path(path).suffix.lower() in _CODE_SUFFIXES
    }
    if not _incremental_surface_is_stable(code_changed, shards, selected):
        result = run_isolated_full_build(
            root,
            changed_paths=code_changed,
            liveness=liveness,
        )
        refresh_repo_map_from_graph(
            root,
            result.graph,
            changed,
            files,
            mapper=mapper,
        )
        return result.graph
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
        and edge.source not in affected
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
    unresolved_refs: list[dict[str, object]] = []
    replacement_edges.extend(
        resolve_calls(
            selected,
            symbol_index,
            file_edges,
            class_ids=class_ids,
            unresolved_out=unresolved_refs,
        )
    )

    graph = current.model_copy(deep=True)
    graph.nodes = nodes
    graph.edges = kept_edges + replacement_edges
    enrich_budget = 120.0
    try:
        from devcouncil.app.config import load_config

        enrich_budget = float(load_config(root).indexing.semantic_enrich_timeout_seconds)
    except Exception:
        logger.debug("semantic enrich budget config load failed", exc_info=True)
    enrich_semantic_edges(
        graph,
        root=root,
        paths=affected,
        extractions=extractions,
        budget_seconds=enrich_budget,
    )
    referenced_ids = {
        identifier
        for edge in graph.edges
        for identifier in (edge.source, edge.target)
    }
    graph.nodes = [
        node for node in graph.nodes
        if node.path
        or node.kind not in {NodeKind.EVENT, NodeKind.DYNAMIC}
        or node.id in referenced_ids
    ]

    if liveness:
        from devcouncil.indexing.wiring import build_dynamic_import_index, entry_roots

        all_roots = entry_roots(root, files)
        fresh_roots = entry_roots(root, files, production_only=True)
        if any("dynamic_import_keys" not in shard for shard in shards.values()):
            dynamic_index = build_dynamic_import_index(root, files)
            for shard in shards.values():
                shard["dynamic_import_keys"] = []
            for key, referencing_paths in dynamic_index.items():
                for referencing_path in referencing_paths:
                    target_shard = shards.get(referencing_path)
                    if target_shard is not None:
                        raw_keys = target_shard.setdefault("dynamic_import_keys", [])
                        if isinstance(raw_keys, list):
                            raw_keys.append(key)
        else:
            dynamic_index = dynamic_import_index_from_shards(shards)
            unsharded_files = [path for path in files if path not in shards]
            for key, referencing_paths in build_dynamic_import_index(
                root, unsharded_files
            ).items():
                dynamic_index.setdefault(key, set()).update(referencing_paths)
        # Same call-edge projection as the full build (see assemble_graph) so a
        # Go same-package callee doesn't flip back to unwired on the first
        # incremental refresh after a full map said it was live.
        liveness_file_edges = sorted(
            set(file_edges) | project_call_edges_to_files(graph.edges)
        )
        entry_roots_list, unwired, unreachable, unreliable = file_liveness_from_shards(
            files,
            liveness_file_edges,
            shards,
            root=root,
            entry_roots=all_roots,
            production_entry_roots=fresh_roots,
            dynamic_index=dynamic_index,
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
            entry_roots_list,
            token_dead_keys=token_keys,
            file_edges=file_edges,
            unreachable=[] if unreliable else unreachable,
            dynamic_index=dynamic_index,
        )
        graph.entry_roots = entry_roots_list
        graph.unwired_candidates = unwired
        graph.unreachable_files = unreachable
        graph.meta["liveness_unreachable_unreliable"] = unreliable
        graph.meta["token_dead_count"] = len(token_dead)
        prior_unresolved = [
            ref
            for ref in (current.meta.get("unresolved_references") or [])
            if isinstance(ref, dict) and str(ref.get("path") or "") not in affected
        ]
        merged_unresolved = prior_unresolved + unresolved_refs
        graph.meta["unresolved_references"] = merged_unresolved[:200]
        graph.meta["unresolved_total"] = len(merged_unresolved)
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
    community_changed_paths = {
        node.path
        for node in graph.nodes
        if node.path
        and previous_communities.get(
            (node.id, node.path, node.line, node.kind.value)
        ) != node.community
    }
    # Pathless semantic channel/bridge nodes must be replaced as a group; SQLite
    # otherwise copies them from the prior generation as "unaffected".
    persistence_paths = (
        affected
        | liveness_changed_paths
        | community_changed_paths
        | stale_shard_paths
        | {""}
    )
    graph.meta["liveness_changed_paths"] = sorted(liveness_changed_paths)
    graph.meta["community_changed_paths"] = sorted(community_changed_paths)
    from devcouncil.indexing.graph.build import CompatibilityGraphTooLarge

    generation_before = service.store.current_generation()
    compatibility_reason = ""
    try:
        write_code_graph(
            root,
            graph,
            changed_paths=persistence_paths,
            analysis_shards=shards,
        )
    except CompatibilityGraphTooLarge as exc:
        compatibility_reason = str(exc)
        logger.warning("graph committed but compatibility export was skipped: %s", exc)
    generation_after = service.store.current_generation()
    record_inline_build_status(
        root,
        state="degraded" if compatibility_reason else "complete",
        mode="incremental",
        phase="complete",
        generation_before=generation_before,
        generation_after=generation_after,
        reason=compatibility_reason,
        compatibility_export="degraded" if compatibility_reason else "healthy",
    )
    refresh_repo_map_from_graph(
        root,
        graph,
        affected,
        files,
        mapper=mapper,
    )
    return service.load() or graph


def _resolution_surface(extraction) -> dict[str, object]:  # noqa: ANN001
    """Fields whose changes can alter resolution outside the edited file."""
    return {
        "language": extraction.language,
        "imports": list(extraction.imports),
        "import_details": [asdict(value) for value in extraction.import_details],
        "symbols": [
            {
                key: value
                for key, value in asdict(symbol).items()
                if key not in {"line", "end_line"}
            }
            for symbol in extraction.symbols
        ],
        "all_exports": list(extraction.all_exports),
        "reexports": list(extraction.reexports),
    }


def _incremental_surface_is_stable(
    changed: set[str],
    shards: dict[str, dict[str, object]],
    selected: dict,
) -> bool:
    """Only existing files with an unchanged external resolution surface are safe."""
    for path in changed:
        old_shard = shards.get(path)
        new_extraction = selected.get(path)
        if old_shard is None or new_extraction is None:
            return False
        old_extraction = extraction_from_liveness_shard(old_shard)
        if _resolution_surface(old_extraction) != _resolution_surface(new_extraction):
            return False
    return True


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


def _normal(path: str) -> str:
    normalized = path.replace("\\", "/")
    return normalized[2:] if normalized.startswith("./") else normalized
