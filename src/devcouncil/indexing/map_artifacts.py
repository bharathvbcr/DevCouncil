"""Repo-map artifact writers (JSON + agent guides) — indexing leaf, no CLI import.

CLI ``dev map``, init, wiki remap, verify/checkout refresh and MCP ingest all
call into this module so indexing/verification do not import
``cli.commands.map``.

**One writer.** Every path here builds through the Rust kernel
(`devcouncil.devmap_engine.build_map`). This module used to run the Python
indexer, and after `dev map` moved to the kernel it kept doing so for every
*other* caller — verify, task checkout, `dev plan`, `dev map init/ingest/sync`,
MCP `devcouncil_graph_ingest`. Two engines wrote `repo_map.json` from two
stores at two generations (76 and 511 on this repository), and whichever ran
last won. Nothing reported it, because each writer stamped the map fresh.
"""

from __future__ import annotations

import logging
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from rich.console import Console

from devcouncil.indexing.graph.schema import CodeGraph
from devcouncil.indexing.repo_mapper import RepoMap, RepoMapper

logger = logging.getLogger(__name__)
status_console = Console(stderr=True)

AGENT_GUIDE_MARKER = "<!-- Managed by dev map: keep this file in sync with .devcouncil/repo_map.json. -->"

#: The value the Rust kernel stamps into `repo_map.json` and `code_graph.json`.
MAP_ENGINE = "devmap-rust"


@dataclass
class GraphRefreshResult:
    repo_map: RepoMap
    graph: CodeGraph | None
    generation: int | None
    mode: str
    degraded: bool = False
    reason: str = ""
    # True when SQLite committed but code_graph.json export was skipped (size cap).
    # Retained for callers that still read it; the kernel writes both artifacts
    # from one invocation, so it is always False on this path.
    compatibility_export_degraded: bool = False
    # True when the graph build timed out / failed but a healthy prior generation
    # was still available. The kernel fails closed instead, so this is always
    # False on this path; retained for callers that still read it.
    build_incomplete: bool = False
    # What the kernel reports about its store after the build: pending and
    # quarantined paths, freshness, degraded reason. ``None`` when the client
    # could not reach the store — a status that could not be read is reported as
    # unknown, never as healthy.
    kernel_status: Any = None


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
            "Code graph: `.devcouncil/graph/code_graph.json` (symbol-level; query with `dev map`).",
            *wiki_lines,
            "",
            "Workflow for agents:",
            "1. Open `.devcouncil/repo_map.json` before guessing at file locations.",
            "2. Use the `files` list to resolve module ownership and nearby siblings.",
            "3. Use `subsystems` for subsystem-level navigation.",
            "4. In `subsystems`, use `entry_points` + `critical_files` for entry points and starting context.",
            "5. Use `role_files` in `subsystems` for subsystem role buckets (entry, runtime, policy, adapters, etc.).",
            "6. Use `neighbors` and `handoff_paths` in `subsystems` to follow cross-subsystem flow.",
            "7. Prefer `dev map dead --confidence extracted` + file greps for dead code. "
            "Treat `inferred` as unconfirmed. Prefer `unwired_candidates` / "
            "`dead_symbol_candidates` over `unreachable_files` (static BFS is often "
            "noisy for routers / dynamic imports / JSX). If `entry_roots` are empty / "
            "`liveness_unreachable_unreliable`, ignore `unreachable_files` and mass inferred dead. "
            "Check `unwired_candidates` / `dead_symbol_candidates` before creating new modules — "
            "wire what you create into a real caller.",
            "8. Use `dev map query <name>` / `dev map trace <a> <b>` / `dev map dead` "
            "for symbol callers, paths, and dead-code tiers; `dev map graph-html` "
            "(or `dev map html --symbols`) for the symbol visualizer. "
            "The kernel store (`.devcouncil/codeintel/devmap.sqlite`) is canonical — prefer "
            "`dev map` commands when `code_graph.json` is missing or a size-capped stub.",
            "9. Run `dev map` (or `dev map --watch`) after large refactors; "
            "`dev map status` / `dev map doctor` report engine, store and freshness.",
            "",
            "DevCouncil loop:",
            "- Prefer DevCouncil MCP tools (`devcouncil_status`, `devcouncil_checkout_task`, "
            "`devcouncil_verify_task`, …) for task state; do not guess.",
            "- Interactive Shell does not need a lease under assist (`hook_gate.mode=off`); "
            "checkout before writes only when write-gates / contain mode are active.",
            "- Engineering skills live under `.claude/skills/` and `.cursor/skills/` "
            "(`dev skills scaffold` / `dev integrate cursor --apply`).",
            "",
            "Important surfaces:",
            *_important_surfaces(repo_map),
            "",
            "If the map and source disagree, trust the source and regenerate the map.",
        ]
    )


def write_agent_guides(repo_root: Path, repo_map_path: Path, repo_map: RepoMap) -> bool:
    """Write the marker-guarded guides. Returns True when any file changed on disk.

    The return value matters: a guide this creates or rewrites is a file in the
    tree, and unless the repository ignores it, it is part of the inventory the
    freshness stamps were computed over. A caller that changed the tree after
    stamping must restamp, or the map it just wrote reads stale at once.
    """
    changed = False
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
        changed = True
    return changed


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


def _kernel_status(root: Path):
    """The kernel's own view of the store after a build, or ``None`` if unreadable."""
    try:
        from devcouncil.devmap_client import DevMapClient, DevMapClientError

        try:
            # A probe, not a session: never spawn a daemon to answer it.
            return DevMapClient(root, autospawn=False).status()
        except DevMapClientError as exc:
            logger.debug("kernel status unavailable after build: %s", exc)
            return None
    except Exception:  # noqa: BLE001 - a status probe must never fail a build
        logger.debug("kernel status probe failed", exc_info=True)
        return None


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
    full: bool = False,
) -> GraphRefreshResult:
    """Build the map through the Rust kernel and layer on what the kernel does not do.

    The kernel writes `repo_map.json` and `code_graph.json` from one generation.
    Afterwards, and only on top of those artifacts:

    - ``goal`` ranks ``candidate_files`` with the ripgrep-based scorer;
    - ``scan_dependencies`` runs the local dependency auditors into
      ``dependency_risks``;
    - the marker-guarded agent guides (``AGENTS.md`` / ``CLAUDE.md``) are
      regenerated.

    Both enrichments are merged into the artifact the kernel wrote — every other
    key, including the freshness stamps and ``map_engine``, is preserved — and
    written atomically.

    ``liveness``, ``lsp_refs``, ``graph`` and ``paths`` are accepted for callers
    that still pass them and have no effect: the kernel always computes
    liveness, the LSP adjunct was cut with the Python engine, and the kernel
    decides for itself whether a build is incremental. ``full`` forces a cold
    rebuild.

    Raises ``DevMapEngineError`` when the kernel cannot build. There is no
    fallback; callers that must not fail (verify, checkout) catch it.
    """
    del liveness, lsp_refs, graph, paths, quiet
    from devcouncil.devmap_engine import (
        DevMapEngineError,
        build_map,
        write_json_atomically,
    )
    from devcouncil.utils.json_persist import read_json

    root = root.expanduser().resolve()
    output = Path(output).expanduser()
    output = output if output.is_absolute() else root / output

    written = build_map(root, output=output, full=full)
    payload = read_json(written)
    if not isinstance(payload, dict):
        raise DevMapEngineError(f"the kernel wrote an unreadable map at {written}")
    repo_map = RepoMap.model_validate(payload)

    enrichment: dict[str, object] = {}
    if goal:
        mapper = RepoMapper(root)
        enrichment["candidate_files"] = mapper._ripgrep_search(
            goal, [entry.path for entry in repo_map.files]
        )
    if scan_dependencies:
        enrichment["dependency_risks"] = RepoMapper(root)._scan_dependency_risks()
    if enrichment:
        payload.update(enrichment)
        write_json_atomically(written, payload)
        repo_map = RepoMap.model_validate(payload)

    if write_agent_guides(root, written, repo_map):
        # The guides are files in the tree. In a repository that does not
        # ignore them (every fresh checkout without a `.gitignore` entry) they
        # join the inventory the kernel indexed a moment ago, so the store is
        # one generation behind the tree and the map it just wrote reads stale
        # to `map_is_stale` immediately — measured: a three-file repository was
        # stale on a map one second old, because `AGENTS.md` and `CLAUDE.md`
        # had appeared after the stamp. Restamping the fingerprint alone was
        # the first fix and left the store behind: the next `dev map` on an
        # untouched tree then committed a *new* generation, because to the
        # kernel two files had appeared. So build again — incremental, two
        # files — and the store, the artifacts and the stamps all describe the
        # same tree. Only when a guide actually changed, which on a repository
        # that already carries them is never.
        written = build_map(root, output=output)
        payload = read_json(written)
        if not isinstance(payload, dict):
            raise DevMapEngineError(f"the kernel wrote an unreadable map at {written}")
        if enrichment:
            payload.update(enrichment)
            write_json_atomically(written, payload)
        repo_map = RepoMap.model_validate(payload)

    kernel = _kernel_status(root)
    reason = ""
    if kernel is not None and (kernel.degraded_reason or not kernel.is_fresh):
        reason = kernel.degraded_reason or (
            f"{kernel.pending_count} path(s) still pending in the kernel store"
        )
    return GraphRefreshResult(
        repo_map=repo_map,
        graph=None,
        generation=(kernel.generation_id if kernel is not None else None),
        mode=MAP_ENGINE,
        degraded=False,
        reason=reason,
        kernel_status=kernel,
    )
