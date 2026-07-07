"""Goal-driven knowledge source selection shared by CLI and MCP."""

from __future__ import annotations

from pathlib import Path


def select_knowledge_payload(root: Path, goal: str) -> dict[str, object]:
    """Return MCP/CLI-compatible knowledge selection for ``goal``."""
    from devcouncil.knowledge.resource_discovery import knowledge_settings
    from devcouncil.knowledge.sources import render_knowledge_preamble, select_knowledge_sources

    try:
        directory, design_always = knowledge_settings(root)
        if directory is None:
            sources = []
        else:
            sources = select_knowledge_sources(
                goal, root, directory=directory, design_always=design_always
            )
        preamble = render_knowledge_preamble(sources)
        return {
            "ok": True,
            "goal": goal,
            "sources": [
                {"name": source.name, "kind": source.kind, "description": source.description}
                for source in sources
            ],
            "preamble": preamble,
        }
    except Exception as exc:
        return {
            "ok": True,
            "goal": goal,
            "sources": [],
            "preamble": "",
            "note": f"knowledge unavailable: {exc}",
        }
