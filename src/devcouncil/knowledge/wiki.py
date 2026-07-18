"""Codebase wiki: generate and maintain an OKF bundle documenting the repository.

This is DevCouncil's take on the "LLM wiki" pattern (OpenWiki, Karpathy's LLM-wiki
gist, Google's Open Knowledge Format): a directory of markdown concept documents with
YAML frontmatter that agents consult before doing real work, kept up to date by the
tool rather than by hand.

Design:

* **Deterministic skeleton** — every page is derived from ``repo_map.json``
  (:class:`devcouncil.indexing.repo_mapper.RepoMap`), so structure, file lists, and
  cross-links are always correct and generation works offline with no model configured.
* **Optional LLM enrichment** — when a :class:`devcouncil.llm.router.ModelRouter` is
  supplied, new/stale pages get prose sections (overview, key flows, agent guidance)
  written by the ``wiki_writer`` role. Enrichment degrades to the skeleton on any
  model failure (the router's ``fallback`` machinery).
* **OKF-conformant output** — pages are :class:`devcouncil.knowledge.okf.OKFDocument`
  bundles under ``.devcouncil/knowledge/okf/wiki/``, which
  :func:`devcouncil.knowledge.sources.discover_knowledge_sources` already scans — so
  wiki pages flow into planning/council/task prompts with zero extra wiring, selected
  by their tags (subsystem path segments) like any other OKF knowledge.
* **Incremental updates** — a fingerprint per page (hash of the repo-map slice that
  shapes it) is kept in a ``.wiki-state.json`` sidecar. Unchanged pages are skipped on
  regeneration, which both keeps updates cheap and *preserves prior LLM enrichment*.
* **log.md** — an OKF-conventional chronological change log of what each run touched.
"""

from __future__ import annotations

import asyncio
import hashlib
import json
import logging
import re
from datetime import datetime, timezone
from pathlib import Path
from typing import TYPE_CHECKING, Optional

from pydantic import BaseModel, Field

from devcouncil.indexing.repo_mapper import RepoMap, RepoSubsystem
from devcouncil.knowledge.okf import OKFBundle, OKFDocument, read_bundle, validate_bundle, write_bundle
from devcouncil.utils.json_persist import read_json, write_json

if TYPE_CHECKING:
    from devcouncil.llm.router import ModelRouter

logger = logging.getLogger(__name__)

# Bump when skeleton rendering changes shape, so every page is considered stale and
# regenerated even though its repo-map slice is unchanged.
GENERATOR_VERSION = 1

# Default bundle location relative to the configured knowledge directory. Living under
# knowledge/okf/ is what makes prompt injection automatic (sources.py scans it).
WIKI_SUBDIR = "okf/wiki"

_STATE_FILENAME = ".wiki-state.json"
_LOG_MAX_ENTRIES = 50

_ENRICH_SYSTEM = (
    "You are a senior engineer writing agent-facing documentation for a codebase wiki. "
    "You are given structured facts about one subsystem of a repository (its files, "
    "entry points, roles, and neighbors). Write concise, concrete documentation that "
    "helps a coding agent work in this subsystem. Never invent files, APIs, or "
    "behavior not implied by the provided facts. Prefer specifics over generalities."
)


class WikiProse(BaseModel):
    """LLM-written prose sections for one wiki page. All fields optional so the
    enrichment call can degrade to an empty instance (skeleton-only page)."""

    overview: str = Field(
        "", description="2-4 sentences: what this subsystem does and why it exists."
    )
    key_flows: list[str] = Field(
        default_factory=list,
        description="Up to 5 short bullets tracing the important call/data flows.",
    )
    agent_guidance: list[str] = Field(
        default_factory=list,
        description="Up to 5 short bullets: conventions and pitfalls an agent editing this subsystem must respect.",
    )


class WikiResult(BaseModel):
    """Outcome of one generate/update run."""

    wiki_dir: str = ""
    created: list[str] = Field(default_factory=list)
    updated: list[str] = Field(default_factory=list)
    skipped: list[str] = Field(default_factory=list)
    enriched: list[str] = Field(default_factory=list)
    problems: list[str] = Field(default_factory=list)

    @property
    def changed(self) -> list[str]:
        return self.created + self.updated


def slugify(area: str) -> str:
    """Filesystem-safe page name for a subsystem area (``src/devcouncil/council/`` →
    ``src-devcouncil-council``)."""
    return re.sub(r"[^a-z0-9]+", "-", area.lower()).strip("-") or "root"


def _area_tags(area: str) -> list[str]:
    """Tags for a subsystem page: the meaningful path segments of its area. Tags double
    as prompt-selection keywords (sources.py derives keywords from OKF tags), so a goal
    mentioning "council" or "execution" pulls the matching wiki page into context."""
    segments = [seg for seg in re.split(r"[/\\]+", area) if seg]
    # Drop generic roots that would match almost any goal.
    tags = [seg for seg in segments if seg.lower() not in {"src", "lib", "app", "pkg"}]
    return tags or segments


def _now_iso() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


def _fingerprint(payload: object) -> str:
    raw = json.dumps(payload, sort_keys=True, default=str)
    return hashlib.sha256(f"v{GENERATOR_VERSION}:{raw}".encode("utf-8")).hexdigest()[:16]


def _subsystem_payload(subsystem: RepoSubsystem, repo_map: RepoMap) -> dict:
    """The repo-map slice that shapes one subsystem page (also the fingerprint input)."""
    area_prefix = subsystem.area.rstrip("/")
    files = [
        {"path": f.path, "kind": f.kind, "summary": f.summary}
        for f in repo_map.files
        if f.area == subsystem.area or f.path.startswith(area_prefix)
    ][:80]
    return {
        "area": subsystem.area,
        "summary": subsystem.summary,
        "entry_points": subsystem.entry_points,
        "critical_files": subsystem.critical_files,
        "neighbors": subsystem.neighbors,
        "handoff_paths": subsystem.handoff_paths,
        "role_files": subsystem.role_files,
        "files": files,
    }


def _overview_payload(repo_map: RepoMap) -> dict:
    return {
        "languages": repo_map.languages,
        "frameworks": repo_map.frameworks,
        "package_managers": repo_map.package_managers,
        "test_commands": repo_map.test_commands,
        "important_files": repo_map.important_files,
        "subsystems": [s.area for s in repo_map.subsystems],
    }


# --- Skeleton rendering ----------------------------------------------------------


def _bullets(items: list[str], code: bool = True) -> list[str]:
    fmt = "- `{0}`" if code else "- {0}"
    return [fmt.format(item) for item in items]


def _wired_to_links(project_root: Path | None, subsystem: RepoSubsystem) -> list[str]:
    """Graph-derived import neighbors for wiki 'Wired to' sections (OKF-style links).

    Link targets use the same ``files/<path>.md`` layout as
    ``dev graph export --format okf``, via :mod:`export_links`, so wiki pages can
    cross-link into a sibling graph OKF bundle under ``../graph/``.
    """
    if project_root is None:
        return []
    try:
        from devcouncil.indexing.graph.build import load_code_graph
        from devcouncil.indexing.graph.export_links import subsystem_doc_path, wired_to_bullets

        graph = load_code_graph(project_root)
        if graph is None:
            return []
        # Collect files in this area from critical/entry/role lists
        area_files = set(subsystem.entry_points + subsystem.critical_files)
        for paths in (subsystem.role_files or {}).values():
            area_files.update(paths)
        targets: set[str] = set()
        for e in graph.edges:
            if e.kind != "imports":
                continue
            if "::" in e.source or "::" in e.target:
                continue
            if e.source in area_files and e.target not in area_files:
                targets.add(e.target)
        from_rel = subsystem_doc_path(subsystem.area)
        return wired_to_bullets(targets, from_rel=from_rel, link_to_graph=True)
    except Exception:
        return []


def _subsystem_body(
    subsystem: RepoSubsystem,
    slug_by_area: dict[str, str],
    prose: Optional[WikiProse] = None,
    *,
    project_root: Path | None = None,
) -> str:
    lines: list[str] = [f"# {subsystem.area}", "", subsystem.summary.strip()]

    if prose and prose.overview.strip():
        lines += ["", "## Overview", "", prose.overview.strip()]

    if subsystem.entry_points:
        lines += ["", "## Entry points", ""] + _bullets(subsystem.entry_points)
    if subsystem.critical_files:
        lines += ["", "## Critical files", ""] + _bullets(subsystem.critical_files)

    if subsystem.role_files:
        lines += ["", "## Files by role", ""]
        for role, paths in subsystem.role_files.items():
            if paths:
                lines.append(f"- **{role}**: " + ", ".join(f"`{p}`" for p in paths[:8]))

    if prose and prose.key_flows:
        lines += ["", "## Key flows", ""] + _bullets(prose.key_flows, code=False)

    if subsystem.neighbors:
        lines += ["", "## Neighbors", ""]
        for neighbor in subsystem.neighbors:
            slug = slug_by_area.get(neighbor)
            if slug:
                lines.append(f"- [{neighbor}]({slug}.md)")
            else:
                lines.append(f"- `{neighbor}`")
    if subsystem.handoff_paths:
        lines += ["", "## Handoff paths", ""] + _bullets(subsystem.handoff_paths)

    wired = _wired_to_links(project_root, subsystem)
    if wired:
        lines += ["", "## Wired to", ""] + wired

    if prose and prose.agent_guidance:
        lines += ["", "## Guidance for agents", ""] + _bullets(prose.agent_guidance, code=False)

    return "\n".join(lines).strip()


def _development_body(repo_map: RepoMap) -> str:
    lines: list[str] = ["# Development guide", ""]
    if repo_map.languages:
        lines += ["## Languages", ""] + _bullets(repo_map.languages, code=False)
    if repo_map.frameworks:
        lines += ["", "## Frameworks", ""] + _bullets(repo_map.frameworks, code=False)
    if repo_map.package_managers:
        lines += ["", "## Package managers", ""] + _bullets(repo_map.package_managers, code=False)
    if repo_map.test_commands:
        lines += ["", "## Test and check commands", ""] + _bullets(repo_map.test_commands)
    if repo_map.important_files:
        lines += ["", "## Important files", ""] + _bullets(repo_map.important_files)
    return "\n".join(lines).strip()


def _index_body(project_name: str, repo_map: RepoMap, slug_by_area: dict[str, str]) -> str:
    lines = [
        f"# {project_name} codebase wiki",
        "",
        "Agent-facing documentation for this repository, generated and maintained by "
        "`dev wiki` from `.devcouncil/repo_map.json`. Start here, then follow the "
        "subsystem pages. If the wiki and source disagree, trust the source and run "
        "`dev wiki update`.",
        "",
        "## Subsystems",
        "",
    ]
    for subsystem in repo_map.subsystems:
        slug = slug_by_area[subsystem.area]
        summary = subsystem.summary.strip().rstrip(".")
        lines.append(f"- [{subsystem.area}](subsystems/{slug}.md) — {summary}")
    lines += [
        "",
        "## Reference",
        "",
        "- [Development guide](overview/development.md)",
        "- [Change log](log.md)",
    ]
    return "\n".join(lines).strip()


def _build_skeleton(
    repo_map: RepoMap,
    project_name: str,
    timestamp: str,
    prose_by_area: dict[str, WikiProse],
    *,
    project_root: Path | None = None,
) -> list[OKFDocument]:
    slug_by_area = {s.area: slugify(s.area) for s in repo_map.subsystems}
    docs: list[OKFDocument] = [
        OKFDocument(
            type="Codebase Wiki Index",
            title=f"{project_name} codebase wiki",
            description=f"Index of agent-facing wiki pages for {project_name}.",
            tags=[],
            timestamp=timestamp,
            body=_index_body(project_name, repo_map, slug_by_area),
            rel_path="index.md",
        ),
        OKFDocument(
            type="Development Guide",
            title=f"{project_name} development guide",
            description=f"Languages, tooling, and test commands for {project_name}."[:280],
            tags=["development", "testing", "build"],
            timestamp=timestamp,
            body=_development_body(repo_map),
            rel_path="overview/development.md",
        ),
    ]
    for subsystem in repo_map.subsystems:
        slug = slug_by_area[subsystem.area]
        docs.append(
            OKFDocument(
                type="Subsystem",
                title=subsystem.area,
                description=subsystem.summary.strip()[:280],
                resource=subsystem.area,
                tags=["subsystem"] + _area_tags(subsystem.area),
                timestamp=timestamp,
                body=_subsystem_body(
                    subsystem,
                    slug_by_area,
                    prose_by_area.get(subsystem.area),
                    project_root=project_root,
                ),
                rel_path=f"subsystems/{slug}.md",
            )
        )
    return docs


# --- State + change log ------------------------------------------------------------


def _load_state(wiki_dir: Path) -> dict:
    state_path = wiki_dir / _STATE_FILENAME
    try:
        data = read_json(state_path)
        return data if isinstance(data, dict) else {}
    except (OSError, json.JSONDecodeError):
        return {}


def _save_state(wiki_dir: Path, state: dict) -> None:
    wiki_dir.mkdir(parents=True, exist_ok=True)
    write_json(wiki_dir / _STATE_FILENAME, state, sort_keys=True)


def _log_document(wiki_dir: Path, timestamp: str, result: WikiResult) -> OKFDocument:
    """Build log.md: newest entry first, bounded to the last _LOG_MAX_ENTRIES runs."""
    entry_lines = [f"## {timestamp}", ""]
    for label, paths in (
        ("Created", result.created),
        ("Updated", result.updated),
        ("Enriched", result.enriched),
    ):
        if paths:
            entry_lines.append(f"- {label}: " + ", ".join(f"`{p}`" for p in sorted(paths)))
    if not result.created and not result.updated:
        entry_lines.append("- No pages changed (all fingerprints fresh).")
    entry = "\n".join(entry_lines)

    previous_entries: list[str] = []
    log_path = wiki_dir / "log.md"
    if log_path.is_file():
        try:
            existing = OKFDocument.from_markdown(
                log_path.read_text(encoding="utf-8", errors="replace"), rel_path="log.md"
            )
            # Entries are "## <timestamp>" sections after the H1.
            chunks = re.split(r"\n(?=## )", existing.body)
            previous_entries = [c.strip() for c in chunks if c.strip().startswith("## ")]
        except Exception:
            # A corrupt change log is a convenience artifact — restart it rather
            # than failing the whole wiki refresh.
            logger.warning("wiki log.md unreadable; restarting change log", exc_info=True)

    entries = [entry] + previous_entries
    body = "# Wiki change log\n\n" + "\n\n".join(entries[:_LOG_MAX_ENTRIES])
    return OKFDocument(
        type="Change Log",
        title="Wiki change log",
        description="Chronological history of wiki generation runs.",
        tags=[],
        timestamp=timestamp,
        body=body,
        rel_path="log.md",
    )


# --- Enrichment ----------------------------------------------------------------


async def _enrich_area(router: "ModelRouter", payload: dict) -> WikiProse:
    messages = [
        {"role": "system", "content": _ENRICH_SYSTEM},
        {
            "role": "user",
            "content": (
                "Write the wiki prose sections for this subsystem.\n\n"
                f"Subsystem facts (JSON):\n{json.dumps(payload, indent=2)}"
            ),
        },
    ]
    return await router.complete_structured(
        "wiki_writer", messages, WikiProse, fallback=WikiProse(overview="")
    )


async def _enrich_all(router: "ModelRouter", payloads: dict[str, dict]) -> dict[str, WikiProse]:
    areas = list(payloads)
    results = await asyncio.gather(
        *(_enrich_area(router, payloads[a]) for a in areas), return_exceptions=True
    )
    prose: dict[str, WikiProse] = {}
    for area, res in zip(areas, results):
        if isinstance(res, WikiProse):
            prose[area] = res
        else:
            logger.warning("Wiki enrichment failed for %s: %s", area, res)
    return prose


# --- Public API -----------------------------------------------------------------


def wiki_stale_pages(project_root: Path, repo_map: RepoMap, wiki_dir: Path) -> dict[str, str]:
    """Map of rel_path → reason for every page that would be rewritten by an update."""
    state = _load_state(wiki_dir).get("pages", {})
    stale: dict[str, str] = {}
    fingerprints = _page_fingerprints(repo_map)
    for rel_path, fp in fingerprints.items():
        recorded = state.get(rel_path, {}).get("fingerprint")
        if not (wiki_dir / rel_path).is_file():
            stale[rel_path] = "missing"
        elif recorded != fp:
            stale[rel_path] = "outdated"
    return stale


def _page_fingerprints(repo_map: RepoMap) -> dict[str, str]:
    fingerprints = {
        "index.md": _fingerprint(
            {"kind": "index", "areas": [s.area for s in repo_map.subsystems],
             "summaries": [s.summary for s in repo_map.subsystems]}
        ),
        "overview/development.md": _fingerprint({"kind": "development", **_overview_payload(repo_map)}),
    }
    for subsystem in repo_map.subsystems:
        rel = f"subsystems/{slugify(subsystem.area)}.md"
        fingerprints[rel] = _fingerprint(_subsystem_payload(subsystem, repo_map))
    return fingerprints


def generate_wiki(
    project_root: Path,
    repo_map: RepoMap,
    wiki_dir: Path,
    *,
    router: "Optional[ModelRouter]" = None,
    force: bool = False,
    project_name: str = "",
) -> WikiResult:
    """Generate or incrementally update the codebase wiki bundle in ``wiki_dir``.

    Pages whose fingerprint matches the recorded state are left untouched (preserving
    any prior LLM enrichment). New/stale pages are rewritten from the current repo map
    and, when ``router`` is provided, enriched with LLM prose via the ``wiki_writer``
    role (degrading to the deterministic skeleton on any model failure). ``force``
    rewrites (and re-enriches) everything.
    """
    project_name = project_name or (project_root.name or "Project")
    timestamp = _now_iso()
    state = _load_state(wiki_dir)
    pages_state: dict = dict(state.get("pages", {}))
    fingerprints = _page_fingerprints(repo_map)

    result = WikiResult(wiki_dir=str(wiki_dir))

    # Decide which pages need (re)writing before doing any model work.
    to_write: set[str] = set()
    for rel_path, fp in fingerprints.items():
        exists = (wiki_dir / rel_path).is_file()
        if force or not exists or pages_state.get(rel_path, {}).get("fingerprint") != fp:
            to_write.add(rel_path)
            (result.created if not exists else result.updated).append(rel_path)
        else:
            result.skipped.append(rel_path)

    # Enrich only the subsystem pages being written (skeleton-only without a router).
    prose_by_area: dict[str, WikiProse] = {}
    if router is not None:
        payloads = {
            s.area: _subsystem_payload(s, repo_map)
            for s in repo_map.subsystems
            if f"subsystems/{slugify(s.area)}.md" in to_write
        }
        if payloads:
            prose_by_area = asyncio.run(_enrich_all(router, payloads))
            for area, prose in prose_by_area.items():
                if prose.overview or prose.key_flows or prose.agent_guidance:
                    result.enriched.append(f"subsystems/{slugify(area)}.md")

    docs = _build_skeleton(
        repo_map, project_name, timestamp, prose_by_area, project_root=project_root
    )
    bundle = OKFBundle(documents=[d for d in docs if d.rel_path in to_write])
    write_bundle(bundle, wiki_dir)

    for rel_path in to_write:
        pages_state[rel_path] = {
            "fingerprint": fingerprints[rel_path],
            "updated_at": timestamp,
            "enriched": rel_path in result.enriched,
        }
    # Drop state for pages that no longer exist in the map (removed subsystems). Their
    # files are left on disk deliberately — they may hold hand edits — but validation
    # below will flag any broken links from index.md if the map shrank.
    pages_state = {k: v for k, v in pages_state.items() if k in fingerprints}

    # Change log + state sidecar.
    write_bundle(OKFBundle(documents=[_log_document(wiki_dir, timestamp, result)]), wiki_dir)
    _save_state(wiki_dir, {"pages": pages_state, "generated_at": timestamp,
                           "generator_version": GENERATOR_VERSION})

    result.problems = validate_bundle(read_bundle(wiki_dir))
    return result


def _knowledge_dir(root: Path) -> Path:
    """Resolve the configured knowledge directory (no CLI side effects)."""
    directory = ".devcouncil/knowledge"
    try:
        from devcouncil.app.config import load_config

        directory = load_config(root).knowledge.directory
    except Exception as exc:
        logger.debug("wiki refresh: knowledge directory unavailable, using default: %s", exc)
    return root / directory


def wiki_dir_for(root: Path) -> Path:
    return _knowledge_dir(root) / WIKI_SUBDIR


def _project_name(root: Path) -> str:
    name = root.name or "Project"
    try:
        from devcouncil.app.config import load_config

        name = load_config(root).project.name or name
    except Exception as exc:
        logger.debug("wiki refresh: project name unavailable, using directory name: %s", exc)
    return name


def _load_repo_map(root: Path, *, remap: bool) -> RepoMap:
    map_path = root / ".devcouncil" / "repo_map.json"
    if not remap and map_path.is_file():
        try:
            return RepoMap.model_validate_json(map_path.read_text(encoding="utf-8"))
        except Exception:
            # Corrupt/stale-schema map: regenerate instead of crashing — the
            # mapper is deterministic and cheap relative to a failed wiki run.
            logger.warning("repo_map.json unreadable; regenerating for wiki", exc_info=True)
    from devcouncil.indexing.map_artifacts import generate_map_artifacts

    return generate_map_artifacts(root, map_path)


def _build_router(root: Path):
    """Best-effort ModelRouter for wiki enrichment; None degrades to the skeleton."""
    try:
        from devcouncil.app.config import get_api_key, load_config
        from devcouncil.llm.provider import create_provider, validate_model_provider
        from devcouncil.llm.router import ModelRouter

        config = load_config(root)
        validate_model_provider(config.models.provider)
        api_key = get_api_key(config.models.provider, root)
        provider = create_provider(
            config.models.provider, api_key, project_root=root, provider_prefs=config.provider
        )
        role_config = {name: role.model_dump() for name, role in config.models.roles.items()}
        if not role_config:
            return None
        capable = (
            role_config.get("arbiter")
            or role_config.get("planner_a")
            or next(iter(role_config.values()))
        )
        role_config.setdefault("wiki_writer", dict(capable))
        return ModelRouter(provider, role_config, project_root=root)
    except Exception as exc:
        logger.warning("Wiki enrichment unavailable (no model router): %s", exc)
        return None


def refresh_wiki(
    project_root: Path,
    *,
    llm: bool = False,
    force: bool = False,
    remap: bool = False,
) -> WikiResult:
    """Refresh the codebase wiki without CLI/Rich side effects."""
    from devcouncil.telemetry.logging_setup import set_log_dir
    from devcouncil.telemetry.stages import log_stage, log_step

    root = project_root.expanduser().resolve()
    set_log_dir(root)
    logger.info("wiki refresh: llm=%s force=%s remap=%s", llm, force, remap)

    with log_stage("wiki", project_root=root, subcommand="update"):
        log_step("wiki/1: loading repository map", project_root=root, trace=True)
        repo_map = _load_repo_map(root, remap=remap)

        router = _build_router(root) if llm else None

        log_step("wiki/2: generating wiki pages", project_root=root, trace=True)
        result = generate_wiki(
            root,
            repo_map,
            wiki_dir_for(root),
            router=router,
            force=force,
            project_name=_project_name(root),
        )
        log_step(
            "wiki/complete",
            project_root=root,
            created=len(result.created),
            updated=len(result.updated),
            trace=True,
        )
    return result
