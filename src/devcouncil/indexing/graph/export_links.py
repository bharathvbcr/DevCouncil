"""Shared markdown link conventions for wiki + code-graph OKF bundles.

Wiki lives under ``.devcouncil/knowledge/okf/wiki/`` and graph export defaults to
``.devcouncil/knowledge/okf/graph/``. Relative links between them use
:data:`GRAPH_FROM_WIKI` / :data:`WIKI_FROM_GRAPH` so subsystem pages can
cross-link into file docs and vice versa.

devcouncil: allow-unwired — shared helpers; imported by wiki + graph export.
"""

from __future__ import annotations

import re
from typing import Iterable

_SLUG_RE = re.compile(r"[^a-z0-9]+")

# Default sibling-bundle relative roots (wiki <-> graph under knowledge/okf/).
GRAPH_FROM_WIKI = "../graph"
WIKI_FROM_GRAPH = "../wiki"


def slugify_area(area: str) -> str:
    """Filesystem-safe page name for a subsystem area (matches wiki.slugify)."""
    return _SLUG_RE.sub("-", area.lower()).strip("-") or "root"


def subsystem_doc_path(area: str) -> str:
    """Bundle-relative path for a subsystem page."""
    return f"subsystems/{slugify_area(area)}.md"


def file_doc_path(code_path: str) -> str:
    """Bundle-relative path for a code-file page (``files/<path>.md``)."""
    norm = code_path.replace("\\", "/").lstrip("./")
    return f"files/{norm}.md"


def relative_md_link(rel_from: str, rel_to: str, text: str) -> str:
    """Markdown link from one bundle document to another (POSIX-relative)."""
    from_parts = [p for p in rel_from.replace("\\", "/").split("/")[:-1] if p]
    to_parts = [p for p in rel_to.replace("\\", "/").split("/") if p]
    i = 0
    while i < len(from_parts) and i < len(to_parts) - 1 and from_parts[i] == to_parts[i]:
        i += 1
    up = [".."] * (len(from_parts) - i)
    down = to_parts[i:]
    rel = "/".join(up + down) or to_parts[-1]
    return f"[{text}]({rel})"


def cross_bundle_file_link(
    *,
    from_rel: str,
    code_path: str,
    text: str | None = None,
    graph_prefix: str = GRAPH_FROM_WIKI,
) -> str:
    """Link from a wiki (or other) doc into a graph-bundle file page."""
    target = f"{graph_prefix.rstrip('/')}/{file_doc_path(code_path)}"
    return relative_md_link(from_rel, target, text or code_path)


def wired_to_bullets(
    targets: Iterable[str],
    *,
    from_rel: str,
    link_to_graph: bool = True,
    graph_prefix: str = GRAPH_FROM_WIKI,
    limit: int = 24,
) -> list[str]:
    """OKF-style ``- [path](...)`` bullets for import neighbors."""
    out: list[str] = []
    for t in sorted({x.replace("\\", "/") for x in targets})[:limit]:
        if link_to_graph:
            out.append(
                f"- {cross_bundle_file_link(from_rel=from_rel, code_path=t, graph_prefix=graph_prefix)}"
            )
        else:
            out.append(f"- {relative_md_link(from_rel, file_doc_path(t), t)}")
    return out
