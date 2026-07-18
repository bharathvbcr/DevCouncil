"""Read-only wiki page lookup shared by CLI and MCP."""

from __future__ import annotations

from pathlib import Path
from typing import Any


def wiki_dir_for(root: Path) -> Path:
    from devcouncil.knowledge.wiki import wiki_dir_for as _wiki_dir_for

    return _wiki_dir_for(root)


def _summary(doc: Any) -> dict[str, object]:
    return {
        "page": doc.rel_path,
        "type": doc.type,
        "title": doc.title,
        "description": doc.description,
        "tags": doc.tags,
    }


def read_wiki_page(
    root: Path,
    *,
    page: str | None = None,
    query: str | None = None,
) -> dict[str, object]:
    """Return MCP/CLI-compatible payload for wiki listing, page fetch, or search."""
    from devcouncil.knowledge.okf import read_bundle

    wiki_dir = wiki_dir_for(root)
    if not (wiki_dir / "index.md").is_file():
        return {
            "ok": False,
            "error": "No codebase wiki has been generated. Run `dev wiki update` to create it.",
            "code": "not_found",
        }

    bundle = read_bundle(wiki_dir)
    pages = {
        doc.rel_path: doc
        for doc in bundle.documents
        if doc.rel_path not in ("log.md",)
    }

    if page:
        doc = pages.get(page.strip())
        if doc is None:
            return {
                "ok": False,
                "error": f"Wiki page {page!r} not found.",
                "code": "not_found",
                "available": sorted(pages),
            }
        return {"ok": True, **_summary(doc), "body": doc.body, "truncated": False}

    if query:
        terms = [term for term in query.lower().split() if term]
        scored: list[tuple[int, Any]] = []
        for doc in pages.values():
            haystack = " ".join([doc.title, doc.description, " ".join(doc.tags), doc.rel_path]).lower()
            hits = sum(1 for term in terms if term in haystack)
            if hits:
                scored.append((hits, doc))
        scored.sort(key=lambda item: (-item[0], item[1].rel_path))
        matches = [doc for _, doc in scored[:5]]
        if not matches:
            return {
                "ok": True,
                "query": query,
                "matches": [],
                "available": [_summary(doc) for doc in pages.values()],
            }
        top = matches[0]
        return {
            "ok": True,
            "query": query,
            "matches": [_summary(doc) for doc in matches],
            **_summary(top),
            "body": top.body,
            "truncated": False,
        }

    return {"ok": True, "pages": [_summary(doc) for doc in pages.values()]}
