"""MCP corpus resource read/list helpers shared by CLI and MCP server."""

from __future__ import annotations

from pathlib import Path

from devcouncil.cli.commands.init import initialize_project
from devcouncil.knowledge.resource_discovery import (
    discover_knowledge_sources,
    knowledge_source_uri,
)
from devcouncil.live.summary import live_review_summary
from devcouncil.reporting.report_builder import ReportBuilder
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import ArtifactGraphRepository, GapRepository, TaskRepository
from devcouncil.utils.json_persist import dump_json


def read_mcp_resource(project_root: Path, uri: str) -> str:
    """Return the body for one ``devcouncil://`` resource URI."""
    initialize_project(project_root, quiet=True)
    db = get_db(project_root)
    key = uri.rstrip("/")

    if key == "devcouncil://report":
        if not db:
            return "DevCouncil is not initialized in this directory."
        with db.get_session() as session:
            graph = ArtifactGraphRepository(session).load_graph()
        return ReportBuilder.build_markdown(graph, live_review=live_review_summary(project_root))

    if key == "devcouncil://tasks":
        if not db:
            return dump_json({"tasks": []}, indent=2)
        with db.get_session() as session:
            tasks = [t.model_dump() for t in TaskRepository(session).get_all()]
        return dump_json({"tasks": tasks}, indent=2)

    if key == "devcouncil://gaps":
        if not db:
            return dump_json({"gaps": []}, indent=2)
        with db.get_session() as session:
            gaps = [g.model_dump() for g in GapRepository(session).get_all()]
        return dump_json({"gaps": gaps}, indent=2)

    if key == "devcouncil://cards":
        return dump_json(live_review_summary(project_root), indent=2)

    if key.startswith("devcouncil://task/"):
        task_id = key.rsplit("/", 1)[-1]
        if not db:
            return dump_json({"ok": False, "error": "not initialized"}, indent=2)
        with db.get_session() as session:
            task = TaskRepository(session).get_by_id(task_id)
            if not task:
                return dump_json({"ok": False, "error": f"Task {task_id} not found."}, indent=2)
            gaps = [g.model_dump() for g in GapRepository(session).get_for_task(task_id)]
        return dump_json({"task": task.model_dump(), "gaps": gaps}, indent=2)

    if key == "devcouncil://knowledge":
        sources = discover_knowledge_sources(project_root)
        if not sources:
            return "# Project knowledge\n\nNo OKF or design knowledge has been ingested for this project."
        lines = ["# Project knowledge", "", "Ingested OKF and design knowledge for this project.", ""]
        for kind in ("design", "okf"):
            kind_sources = [s for s in sources if s.kind == kind]
            if not kind_sources:
                continue
            lines.append(f"## {kind.upper() if kind == 'okf' else kind.capitalize()}")
            lines.append("")
            for source in kind_sources:
                link = knowledge_source_uri(source.kind, source.name)
                desc = source.description or source.name
                lines.append(f"- [{desc}]({link})")
            lines.append("")
        return "\n".join(lines).strip()

    if key.startswith("devcouncil://knowledge/"):
        for source in discover_knowledge_sources(project_root):
            if knowledge_source_uri(source.kind, source.name) == key:
                body = source.render() or source.body
                return str(body) if body is not None else ""
        return f"Knowledge source not found: {key}"

    raise ValueError(f"Unknown resource: {uri}")


def list_mcp_resource_uris(project_root: Path) -> list[dict[str, str]]:
    """Stable resource descriptors for MCP ``list_resources`` (without pydantic types)."""
    initialize_project(project_root, quiet=True)
    resources: list[dict[str, str]] = [
        {
            "uri": "devcouncil://report",
            "name": "DevCouncil report",
            "description": "Coverage report, requirement/task mapping, and blocking gaps.",
            "mimeType": "text/markdown",
        },
        {
            "uri": "devcouncil://tasks",
            "name": "Tasks",
            "description": "All planned tasks with scope and status.",
            "mimeType": "application/json",
        },
        {
            "uri": "devcouncil://gaps",
            "name": "Gaps",
            "description": "All open verification gaps.",
            "mimeType": "application/json",
        },
        {
            "uri": "devcouncil://cards",
            "name": "Live review",
            "description": "Live-review summary: cards, signals, and blockers.",
            "mimeType": "application/json",
        },
    ]
    db = get_db(project_root)
    if db:
        with db.get_session() as session:
            for task in TaskRepository(session).get_all():
                resources.append({
                    "uri": f"devcouncil://task/{task.id}",
                    "name": f"Task {task.id}: {task.title}",
                    "description": f"Scope, status, and gaps for {task.id}.",
                    "mimeType": "application/json",
                })
    knowledge_sources = discover_knowledge_sources(project_root)
    if knowledge_sources:
        resources.append({
            "uri": "devcouncil://knowledge",
            "name": "Project knowledge",
            "description": "Index of ingested OKF and design knowledge for this project.",
            "mimeType": "text/markdown",
        })
        for source in knowledge_sources:
            resources.append({
                "uri": knowledge_source_uri(source.kind, source.name),
                "name": f"Knowledge ({source.kind}): {source.description or source.name}",
                "description": source.description or source.name,
                "mimeType": "text/markdown",
            })
    return resources
