"""Best-effort refresh of ``.devcouncil/repo_map.json`` when fingerprints drift."""

from __future__ import annotations

import logging
from pathlib import Path

from devcouncil.utils.json_persist import read_json, write_model_json

logger = logging.getLogger(__name__)


def refresh_stale_map_if_needed(
    project_root: Path,
    *,
    on_checkout: bool = True,
    on_verify: bool = True,
) -> bool:
    """Regenerate ``repo_map.json`` when its tracked-content fingerprint is stale.

    Honors ``execution.refresh_stale_map_on_checkout`` and
    ``execution.refresh_stale_map_on_verify`` (both default on). Pass
    ``on_checkout=False`` or ``on_verify=False`` to skip a context. Never raises;
    returns True when a remap ran successfully.
    """
    try:
        from devcouncil.app.config import load_config

        try:
            cfg = load_config(project_root)
            checkout_enabled = bool(
                getattr(cfg.execution, "refresh_stale_map_on_checkout", True)
            )
            verify_enabled = bool(
                getattr(cfg.execution, "refresh_stale_map_on_verify", True)
            )
        except Exception:
            checkout_enabled = True
            verify_enabled = True

        if on_checkout and not on_verify:
            enabled = checkout_enabled
        elif on_verify and not on_checkout:
            enabled = verify_enabled
        else:
            enabled = checkout_enabled or verify_enabled
        if not enabled:
            return False

        map_path = project_root / ".devcouncil" / "repo_map.json"
        if not map_path.is_file():
            data: dict = {}
        else:
            loaded = read_json(map_path)
            data = loaded if isinstance(loaded, dict) else {}

        from devcouncil.indexing.repo_mapper import RepoMapper

        mapper = RepoMapper(project_root)
        if map_path.is_file() and not mapper.map_is_stale(data):
            return False

        from devcouncil.cli.commands.map import generate_map_artifacts

        generate_map_artifacts(project_root, map_path, quiet=True)
        return True
    except Exception:
        logger.debug("map refresh failed", exc_info=True)
        return False


def refresh_map_for_paths(root, paths, *, liveness: bool = True):
    """Re-export of graph.build.refresh_map_for_paths."""
    from devcouncil.indexing.graph.build import refresh_map_for_paths as _refresh

    return _refresh(root, paths, liveness=liveness)


def refresh_repo_map_from_graph(
    root,
    graph,
    affected: set[str],
    files: list[str],
    *,
    mapper=None,
) -> None:
    """Update repo-map compatibility fields from an already committed graph.

    Incremental code-intelligence sync must not trigger a second repository-wide
    graph build merely to refresh ``repo_map.json``. This function keeps the
    compatibility artifact aligned using the graph and affected-file set that the
    caller has already computed.
    """
    path = root / ".devcouncil" / "repo_map.json"
    if not path.is_file():
        return
    try:
        from devcouncil.indexing.repo_mapper import RepoMap, RepoMapper

        repo_mapper = mapper or RepoMapper(root)
        repo_map = RepoMap.model_validate(read_json(path))
        by_path = {
            entry.path: entry
            for entry in repo_map.files
            if entry.path not in affected
        }
        file_set = set(files)
        for rel in affected:
            if rel in file_set and (root / rel).is_file():
                by_path[rel] = repo_mapper.describe_file(rel)
        repo_map.files = [by_path[rel] for rel in sorted(by_path)]
        repo_map.entry_roots = list(graph.entry_roots)
        cap = repo_mapper._LIVENESS_CAP
        repo_map.unwired_candidates = list(graph.unwired_candidates)[:cap]
        repo_map.unreachable_files = list(graph.unreachable_files)[:cap]
        repo_map.liveness_unreachable_unreliable = bool(
            graph.meta.get("liveness_unreachable_unreliable")
        )
        repo_map.dead_symbol_candidates = list(
            graph.meta.get("legacy_dead_symbol_candidates") or []
        )[:cap]
        repo_map.generated_head = graph.generated_head
        repo_map.indexed_hash = graph.indexed_hash
        repo_map.content_fingerprint = graph.content_fingerprint
        write_model_json(path, repo_map)
    except Exception:
        logger.warning("incremental repo-map compatibility refresh failed", exc_info=True)

# Back-compat alias for plan expected tests.
refresh_repository_map = refresh_stale_map_if_needed
