"""Markdown + YAML frontmatter: the single split/build implementation.

Both the skills library (:mod:`devcouncil.skills.registry`) and the knowledge formats
(OKF, design.md) store structured metadata in a leading ``---`` YAML block followed by a
markdown body. Keeping one parser/serializer here means a fix to frontmatter handling
applies everywhere rather than drifting between copies.
"""

from __future__ import annotations

from typing import Any

import yaml


def split_frontmatter(text: str) -> tuple[dict[str, Any], str]:
    """Split ``text`` into (frontmatter dict, body).

    Returns ``({}, text)`` when there is no leading ``---`` block or the block does not
    parse to a mapping. Mirrors the historical behavior of
    ``skills.registry._split_frontmatter`` so existing skill files keep parsing.
    """
    if text.startswith("---"):
        parts = text.split("---", 2)
        if len(parts) == 3:
            try:
                meta = yaml.safe_load(parts[1]) or {}
            except yaml.YAMLError:
                meta = {}
            return (meta if isinstance(meta, dict) else {}), parts[2].lstrip("\r\n")
    return {}, text


def build_frontmatter_markdown(meta: dict[str, Any], body: str) -> str:
    """Render a ``---`` YAML frontmatter block above ``body``.

    Empty/``None`` values are dropped so the frontmatter stays minimal (OKF and design.md
    both favor only-what-you-have metadata). Key order is preserved as given by the caller
    (``sort_keys=False``); Unicode is kept literal rather than escaped.
    """
    clean = {k: v for k, v in meta.items() if v not in (None, "", [], {})}
    front = yaml.safe_dump(
        clean,
        sort_keys=False,
        default_flow_style=False,
        allow_unicode=True,
    ).strip()
    body = body.strip()
    if not front:
        return f"{body}\n" if body else ""
    return f"---\n{front}\n---\n\n{body}\n" if body else f"---\n{front}\n---\n"
