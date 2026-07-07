"""Knowledge source discovery for MCP resources and OKF selection."""

from __future__ import annotations

import logging
from pathlib import Path
from urllib.parse import quote

logger = logging.getLogger(__name__)


def knowledge_source_uri(kind: str, name: str) -> str:
    """Stable, parseable resource URI for one ingested knowledge source."""
    return f"devcouncil://knowledge/{kind}/{quote(name, safe='')}"


def knowledge_settings(root: Path) -> tuple[str | None, bool]:
    """Resolve (directory, design_always) for knowledge exposure from project config."""
    try:
        from devcouncil.app.config import load_config

        cfg = load_config(root).knowledge
        return (None if not cfg.enabled else cfg.directory), cfg.design_always
    except Exception as exc:
        logger.debug("Could not load knowledge settings for %s: %s", root, exc)
        return ".devcouncil/knowledge", True


def discover_knowledge_sources(root: Path) -> list:
    """Best-effort enumeration of ingested OKF/design knowledge for the project."""
    try:
        from devcouncil.knowledge.sources import discover_knowledge_sources as _discover

        directory, design_always = knowledge_settings(root)
        if directory is None:
            return []
        return _discover(root, directory=directory, design_always=design_always)
    except Exception as exc:
        logger.debug("Knowledge source discovery failed for %s: %s", root, exc)
        return []
