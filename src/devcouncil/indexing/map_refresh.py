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

        from devcouncil.indexing.map_artifacts import generate_map_artifacts

        generate_map_artifacts(project_root, map_path, quiet=True)
        return True
    except Exception as exc:
        # Concurrent MCP/watcher builds commonly hold the writer lease during
        # checkout/verify. Defer quietly — map stays stale until the next
        # successful refresh rather than failing the lease acquisition path.
        try:
            from devcouncil.codeintel.build_control import GraphBuildBusy
        except Exception:
            GraphBuildBusy = ()  # type: ignore[misc, assignment]
        if isinstance(exc, GraphBuildBusy):
            logger.info("map refresh deferred (writer lease busy): %s", exc)
            return False
        logger.warning("map refresh failed", exc_info=True)
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

    When affected paths include code or manifests, header fields
    (``languages`` / frameworks / package_managers / test_commands) and
    ``subsystems`` are rebuilt via mapper helpers so incremental Swift/Kotlin
    edits do not leave stale map headers.
    """
    path = root / ".devcouncil" / "repo_map.json"
    if not path.is_file():
        return
    try:
        from devcouncil.codeintel.languages import code_extensions, markup_extensions
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

        # Rebuild header classification when code/markup/manifest edits land so
        # languages[] / package_managers[] do not stay stale after incremental sync.
        # Markup (md/html/…) is included for languages[] only — not code_extensions.
        _MANIFEST_NAMES = frozenset({
            "Package.swift", "Cargo.toml", "go.mod", "go.sum",
            "package.json", "pyproject.toml", "requirements.txt",
            "build.gradle", "build.gradle.kts",
            "settings.gradle", "settings.gradle.kts", "gradlew",
            "AndroidManifest.xml",
        })
        header_exts = code_extensions() | markup_extensions()
        needs_header = any(
            Path(rel).suffix.lower() in header_exts
            or Path(rel.replace("\\", "/")).name in _MANIFEST_NAMES
            for rel in affected
        )
        if needs_header:
            all_files = list(files)
            repo_map.languages = repo_mapper.detect_languages(all_files)
            repo_map.frameworks = repo_mapper.detect_frameworks(all_files)
            repo_map.package_managers = repo_mapper.detect_package_managers(all_files)
            repo_map.test_commands = repo_mapper.detect_test_commands(all_files)
            try:
                repo_mapper._source_root = repo_mapper.detect_source_root(all_files)
                # Mirror map_repo: generic when the tree is not DevCouncil itself.
                repo_mapper._use_generic = not any(
                    f.replace("\\", "/").startswith("src/devcouncil/") for f in all_files
                )
                repo_map.subsystems = repo_mapper._build_subsystem_index(all_files)
            except Exception:
                logger.debug("incremental subsystem rebuild failed", exc_info=True)

        # Do NOT stamp fingerprints or clear graph_degraded here. A later
        # ``mapper.map_repo`` failure in ``refresh_map_artifacts`` would otherwise
        # leave a fingerprint-fresh, non-degraded map while subsystems are stale.
        # Full map writes own the freshness handshake.
        write_model_json(path, repo_map)
    except Exception:
        logger.warning("incremental repo-map compatibility refresh failed", exc_info=True)

# Back-compat alias for plan expected tests.
refresh_repository_map = refresh_stale_map_if_needed
