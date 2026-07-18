"""Repo-map artifact writers (JSON + agent guides) — indexing leaf, no CLI import.

CLI ``dev map``, init, wiki remap, and map_refresh all call into this module so
indexing/verification do not import ``cli.commands.map``.
"""

from __future__ import annotations

import logging
from dataclasses import dataclass
from pathlib import Path

from rich.console import Console

from devcouncil.indexing.repo_mapper import RepoMap, RepoMapper
from devcouncil.indexing.graph.schema import CodeGraph
from devcouncil.integrations.code_review_graph import CodeReviewGraphAdapter
from devcouncil.utils.json_persist import write_model_json

logger = logging.getLogger(__name__)
status_console = Console(stderr=True)

AGENT_GUIDE_MARKER = "<!-- Managed by dev map: keep this file in sync with .devcouncil/repo_map.json. -->"


def _stamp_existing_map_degraded(output: Path, *, reason: str) -> None:
    """Mark an on-disk repo map degraded without rewriting fingerprints as fresh."""
    if not output.is_file():
        return
    try:
        from devcouncil.utils.json_persist import read_json

        repo_map = RepoMap.model_validate(read_json(output))
        repo_map.graph_degraded = True
        repo_map.graph_degraded_reason = reason[:500]
        write_model_json(output, repo_map)
    except Exception:
        logger.debug("failed to stamp graph_degraded on partial map", exc_info=True)


@dataclass
class GraphRefreshResult:
    repo_map: RepoMap
    graph: CodeGraph | None
    generation: int | None
    mode: str
    degraded: bool = False
    reason: str = ""
    # True when SQLite committed but code_graph.json export was skipped (size cap).
    # Not fail-closed for sync/watch — doctor/export health owns this signal.
    compatibility_export_degraded: bool = False


def _important_surfaces(repo_map: RepoMap) -> list[str]:
    """Derive the 'important surfaces' list from the computed map."""
    lines: list[str] = []
    for index, subsystem in enumerate(repo_map.subsystems[:6], start=1):
        lines.append(f"{index}. `{subsystem.area}/` — {subsystem.summary}")
    if not lines:
        for index, path in enumerate(repo_map.important_files[:6], start=1):
            lines.append(f"{index}. `{path}`")
    return lines or ["1. See `.devcouncil/repo_map.json` for the file index."]


def _wiki_index_rel(repo_root: Path) -> str | None:
    """Relative path to the codebase wiki index, when a generated wiki exists."""
    from devcouncil.knowledge.wiki import wiki_dir_for

    index = wiki_dir_for(repo_root) / "index.md"
    if not index.is_file():
        return None
    try:
        return index.relative_to(repo_root).as_posix()
    except ValueError:
        return str(index)


def agent_guide_text(repo_map_path: Path, repo_root: Path, repo_map: RepoMap) -> str:
    wiki_index = _wiki_index_rel(repo_root)
    wiki_lines = (
        [
            "",
            f"Codebase wiki: `{wiki_index}` — agent-facing subsystem docs (OKF bundle). "
            "Read the relevant subsystem page before working in it; refresh with `dev wiki update`.",
        ]
        if wiki_index
        else []
    )
    return "\n".join(
        [
            AGENT_GUIDE_MARKER,
            "",
            "# Agent Workspace Guide",
            "",
            "Use `.devcouncil/repo_map.json` as the primary file index for this workspace.",
            f"Repo map: `{repo_map_path.relative_to(repo_root).as_posix() if repo_map_path.is_relative_to(repo_root) else repo_map_path}`",
            "Code graph: `.devcouncil/graph/code_graph.json` (symbol-level; query with `dev graph`).",
            *wiki_lines,
            "",
            "Workflow for agents:",
            "1. Open `.devcouncil/repo_map.json` before guessing at file locations.",
            "2. Use the `files` list to resolve module ownership and nearby siblings.",
            "3. Use `subsystems` for subsystem-level navigation.",
            "4. In `subsystems`, use `entry_points` + `critical_files` for entry points and starting context.",
            "5. Use `role_files` in `subsystems` for subsystem role buckets (entry, runtime, policy, adapters, etc.).",
            "6. Use `neighbors` and `handoff_paths` in `subsystems` to follow cross-subsystem flow.",
            "7. Prefer `dev graph dead --confidence extracted` + file greps for dead code. "
            "Treat `inferred` as unconfirmed. Prefer `unwired_candidates` / "
            "`dead_symbol_candidates` over `unreachable_files` (static BFS is often "
            "noisy for routers / dynamic imports / JSX). If `entry_roots` are empty / "
            "`liveness_unreachable_unreliable`, ignore `unreachable_files` and mass inferred dead. "
            "Check `unwired_candidates` / `dead_symbol_candidates` before creating new modules — "
            "wire what you create into a real caller.",
            "8. Use `dev graph query <name>` / `dev graph trace <a> <b>` / `dev graph dead` "
            "for symbol callers, paths, and dead-code tiers; `dev graph html` for the visualizer. "
            "SQLite (`.devcouncil/codeintel/index.sqlite`) is canonical — prefer "
            "`dev graph` commands when `code_graph.json` is missing or a size-capped stub.",
            "9. Run `dev map` (or `dev map --watch` / `dev graph watch`) after large refactors.",
            "",
            "Important surfaces:",
            *_important_surfaces(repo_map),
            "",
            "If the map and source disagree, trust the source and regenerate the map.",
        ]
    )


def write_agent_guides(repo_root: Path, repo_map_path: Path, repo_map: RepoMap) -> None:
    for filename in ("AGENTS.md", "CLAUDE.md"):
        path = repo_root / filename
        existing: str | None = None
        if path.exists():
            try:
                existing = path.read_text(encoding="utf-8", errors="replace")
            except OSError:
                continue
            if AGENT_GUIDE_MARKER not in existing:
                continue
        text = agent_guide_text(repo_map_path, repo_root, repo_map) + "\n"
        # Skip rewrite when unchanged so content_fingerprint (size+mtime) stays stable
        # across consecutive identical `dev map` runs.
        if existing == text:
            continue
        path.write_text(text, encoding="utf-8")


def generate_map_artifacts(
    root: Path,
    output: Path,
    goal: str = "",
    *,
    scan_dependencies: bool = False,
    liveness: bool = True,
    lsp_refs: bool = False,
    quiet: bool = False,
    graph=None,  # noqa: ANN001
    paths: list[str] | None = None,
) -> RepoMap:
    """Build the repo map and write repo_map.json + agent guides (no LLM, no re-init).

    Assumes ``.devcouncil/`` already exists. Shared by the ``dev map`` command and
    by project initialization so a freshly set-up repo is immediately navigable.
    ``scan_dependencies`` is opt-in (off for init and default mapping) because it can
    shell out to dependency auditors.
    ``lsp_refs`` opts into live LSP confirmation of dead-symbol candidates.
    ``quiet`` suppresses stderr status lines (for ``dev status --json`` auto-init).
    """
    return refresh_map_artifacts(
        root,
        output,
        goal,
        scan_dependencies=scan_dependencies,
        liveness=liveness,
        lsp_refs=lsp_refs,
        quiet=quiet,
        graph=graph,
        paths=paths,
    ).repo_map


def refresh_map_artifacts(
    root: Path,
    output: Path,
    goal: str = "",
    *,
    scan_dependencies: bool = False,
    liveness: bool = True,
    lsp_refs: bool = False,
    quiet: bool = False,
    graph=None,  # noqa: ANN001
    paths: list[str] | None = None,
) -> GraphRefreshResult:
    """Refresh graph and map once, falling back to a lean map on graph failure."""
    import time

    from devcouncil.codeintel import get_codeintel_service
    from devcouncil.codeintel.build_control import (
        GraphBuildBusy,
        graph_build_session,
        run_isolated_full_build,
    )

    t0 = time.perf_counter()
    root = root.expanduser().resolve()
    mode = "incremental" if paths is not None else "full"
    degraded = False
    reason = ""
    compatibility_export_degraded = False
    with graph_build_session(root):
        if graph is None:
            try:
                if paths is not None and get_codeintel_service(root).load() is not None:
                    from devcouncil.codeintel.sync.incremental import sync_affected_paths

                    graph = sync_affected_paths(
                        get_codeintel_service(root),
                        paths,
                        liveness=liveness,
                    )
                    # Export-only skips are recorded on build status; SQLite is healthy.
                    try:
                        from devcouncil.codeintel.build_control import read_build_status

                        status = read_build_status(root)
                        if status.compatibility_export == "degraded":
                            compatibility_export_degraded = True
                            reason = status.degraded_reason or reason
                    except Exception:
                        logger.debug("build status read after incremental failed", exc_info=True)
                else:
                    isolated = run_isolated_full_build(
                        root,
                        changed_paths=None if paths is None else set(paths),
                        liveness=liveness,
                    )
                    graph = isolated.graph
                    mode = "full"
                    # Compatibility JSON size limits must not stamp graph_degraded
                    # (that would force perpetual --if-stale rebuilds while SQLite is fine).
                    if isolated.status.compatibility_export == "degraded":
                        compatibility_export_degraded = True
                        reason = isolated.status.degraded_reason or reason
                    elif isolated.status.state == "degraded" and graph is None:
                        degraded = True
                        reason = isolated.status.degraded_reason or reason
            except GraphBuildBusy:
                # Lease contention after/during a concurrent writer must not stamp a
                # lean/degraded map over a healthy SQLite generation (retry storm).
                raise
            except Exception as exc:  # noqa: BLE001
                logger.warning("graph refresh failed; writing lean repo map", exc_info=True)
                degraded = True
                reason = f"{type(exc).__name__}: {exc}"
                mode = "lean"

        # True lean/unavailable only — never export-only.
        if graph is None:
            degraded = True
            if mode != "lean":
                mode = "lean"

        mapper = RepoMapper(root)
        if graph is not None:
            mapper._prebuilt_code_graph = graph  # type: ignore[attr-defined]
        else:
            mapper._skip_code_graph_build = True  # type: ignore[attr-defined]
        try:
            repo_map = mapper.map_repo(
                goal,
                scan_dependencies=scan_dependencies,
                liveness=liveness,
                lsp_refs=lsp_refs,
            )
        except Exception as exc:
            # Graph may already be committed (incremental/full) while the map
            # rebuild failed — fail closed so --if-stale / verify keep retrying.
            logger.warning("repo map rebuild failed after graph commit", exc_info=True)
            _stamp_existing_map_degraded(
                output if output.is_absolute() else root / output,
                reason=f"{type(exc).__name__}: {exc}",
            )
            raise
        # Stamp degraded handshake before first write so agents never see a
        # fingerprint-fresh lean map without graph_degraded=True.
        repo_map.graph_degraded = bool(degraded)
        repo_map.graph_degraded_reason = reason if degraded else ""
        elapsed = time.perf_counter() - t0
        if not quiet:
            status_console.print(f"[dim]map completed in {elapsed:.2f}s[/dim]")
            if degraded:
                status_console.print(
                    f"[yellow]Graph degraded; wrote {mode} map: {reason}[/yellow]"
                )
            elif compatibility_export_degraded:
                status_console.print(
                    f"[yellow]Compatibility export degraded (SQLite ok; "
                    f"stub/compact JSON may still exist): {reason}[/yellow]"
                )
        graph_context = CodeReviewGraphAdapter(root).get_context()
        output = output if output.is_absolute() else root / output
        output.parent.mkdir(parents=True, exist_ok=True)
        write_model_json(output, repo_map)
        write_agent_guides(root, output, repo_map)
        # Agent guides can add/change tracked files after the fingerprint was taken;
        # re-stamp so the on-disk map is not immediately stale for checkout/verify.
        mapper = RepoMapper(root)
        try:
            files = mapper.get_git_files()
            repo_map.generated_head = mapper._git_head()
            repo_map.indexed_hash = mapper._files_fingerprint(files)
            repo_map.content_fingerprint = mapper._content_fingerprint(files)
            repo_map.graph_degraded = bool(degraded)
            repo_map.graph_degraded_reason = reason if degraded else ""
            write_model_json(output, repo_map)
        except Exception:
            logger.debug("Failed to re-stamp map fingerprints after agent guides", exc_info=True)
        if graph_context.available:
            graph_output = output.with_name("code_review_graph_context.json")
            write_model_json(graph_output, graph_context)
            if not quiet:
                status_console.print(
                    f"[green]Wrote code-review-graph context to {graph_output}[/green]"
                )
    return GraphRefreshResult(
        repo_map=repo_map,
        graph=graph,
        generation=get_codeintel_service(root).store.current_generation(),
        mode=mode,
        degraded=degraded,
        reason=reason,
        compatibility_export_degraded=compatibility_export_degraded,
    )
