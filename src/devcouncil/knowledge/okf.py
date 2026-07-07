"""Open Knowledge Format (OKF) v0.1 — model, bundle I/O, and validation.

OKF (Google Cloud) formalizes the "LLM-wiki" pattern: a directory of markdown files,
each carrying a small YAML frontmatter header, cross-linked with plain markdown links to
form a portable, vendor-neutral knowledge graph. The only required frontmatter field is
``type``; everything else is producer-defined.

DevCouncil uses this module in both directions:

* **Export** — :mod:`devcouncil.reporting.okf_bundle_writer` builds an :class:`OKFBundle`
  from the artifact graph and calls :func:`write_bundle`.
* **Ingest** — :func:`read_bundle` parses an external bundle so it can be surfaced as
  planning context (:mod:`devcouncil.knowledge.sources`).
"""

from __future__ import annotations

import re
from pathlib import Path
from typing import Any

from pydantic import BaseModel, Field

from devcouncil.knowledge.frontmatter import build_frontmatter_markdown, split_frontmatter
from devcouncil.utils.fsio import atomic_write_text

# Markdown inline links: [text](target). We only resolve relative, non-anchor, non-URL
# targets into intra-bundle edges; external resources live in the `resource` field.
_LINK_RE = re.compile(r"\[(?P<text>[^\]]+)\]\((?P<target>[^)]+)\)")
_URL_RE = re.compile(r"^[a-zA-Z][a-zA-Z0-9+.-]*://")


class OKFDocument(BaseModel):
    """A single OKF document: YAML frontmatter header + markdown body.

    ``rel_path`` is the document's POSIX path relative to the bundle root (e.g.
    ``tasks/TASK-001.md``); it is the node identity used when resolving links. ``links``
    are resolved intra-bundle edges (relative link targets normalized to bundle-relative
    POSIX paths), computed by :func:`read_bundle`.
    """

    type: str
    title: str = ""
    description: str = ""
    resource: str = ""
    tags: list[str] = Field(default_factory=list)
    timestamp: str = ""
    body: str = ""
    rel_path: str = ""
    links: list[str] = Field(default_factory=list)

    def to_markdown(self) -> str:
        """Render this document as OKF markdown (frontmatter + body)."""
        meta: dict[str, Any] = {
            "type": self.type,
            "title": self.title,
            "description": self.description,
            "resource": self.resource,
            "tags": self.tags,
            "timestamp": self.timestamp,
        }
        return build_frontmatter_markdown(meta, self.body)

    @classmethod
    def from_markdown(cls, text: str, rel_path: str = "") -> "OKFDocument":
        """Parse OKF markdown into a document (links are resolved by :func:`read_bundle`)."""
        meta, body = split_frontmatter(text)
        tags = meta.get("tags") or []
        if isinstance(tags, str):
            tags = [tags]
        return cls(
            type=str(meta.get("type") or ""),
            title=str(meta.get("title") or ""),
            description=str(meta.get("description") or ""),
            resource=str(meta.get("resource") or ""),
            tags=[str(t) for t in tags],
            timestamp=str(meta.get("timestamp") or ""),
            body=body.strip(),
            rel_path=rel_path,
        )


class OKFBundle(BaseModel):
    """A collection of OKF documents keyed by bundle-relative path."""

    documents: list[OKFDocument] = Field(default_factory=list)

    def by_path(self) -> dict[str, OKFDocument]:
        return {doc.rel_path: doc for doc in self.documents if doc.rel_path}


def _resolve_link(source_rel_path: str, target: str) -> str | None:
    """Resolve a markdown link target found in ``source_rel_path`` to a bundle-relative
    POSIX path, or ``None`` if it is external (URL), an in-page anchor, a non-document
    asset, or escapes root.

    Only ``.md`` targets are treated as intra-bundle document edges: a bundle's document
    set is markdown-only, so links to images (``![alt](x.png)``) or other assets must not
    be recorded as edges — otherwise ``validate_bundle`` would flag every such link as a
    broken intra-bundle reference.
    """
    target = target.strip()
    if not target or target.startswith("#") or _URL_RE.match(target) or target.startswith("mailto:"):
        return None
    target = target.split("#", 1)[0].strip()  # drop any anchor fragment
    if not target or not target.endswith(".md"):
        return None
    source_dir = PurePosix(source_rel_path).parent
    try:
        resolved = (source_dir / target).resolve_relative()
    except ValueError:
        return None
    return resolved


class PurePosix:
    """Tiny relative-POSIX-path helper.

    ``pathlib.PurePosixPath`` does not collapse ``..`` segments (it has no filesystem to
    resolve against), so this resolves ``a/b/../c`` → ``a/c`` purely lexically and rejects
    paths that escape the bundle root. Kept local to avoid pulling in os.path semantics
    that differ on Windows.
    """

    def __init__(self, raw: str) -> None:
        self.parts = [p for p in raw.replace("\\", "/").split("/") if p not in ("", ".")]

    @property
    def parent(self) -> "PurePosix":
        p = PurePosix("")
        p.parts = self.parts[:-1]
        return p

    def __truediv__(self, other: str) -> "PurePosix":
        p = PurePosix("")
        p.parts = self.parts + [seg for seg in other.replace("\\", "/").split("/") if seg not in ("", ".")]
        return p

    def resolve_relative(self) -> str:
        out: list[str] = []
        for seg in self.parts:
            if seg == "..":
                if not out:
                    raise ValueError("path escapes bundle root")
                out.pop()
            else:
                out.append(seg)
        return "/".join(out)


def read_bundle(bundle_dir: Path) -> OKFBundle:
    """Read an OKF bundle from ``bundle_dir``: parse every ``*.md`` file and resolve
    intra-bundle markdown links into :attr:`OKFDocument.links` edges."""
    bundle_dir = bundle_dir.expanduser().resolve()
    docs: list[OKFDocument] = []
    for path in sorted(bundle_dir.rglob("*.md")):
        rel = path.relative_to(bundle_dir).as_posix()
        doc = OKFDocument.from_markdown(path.read_text(encoding="utf-8"), rel_path=rel)
        links: list[str] = []
        seen_links: set[str] = set()
        for match in _LINK_RE.finditer(doc.body):
            resolved = _resolve_link(rel, match.group("target"))
            if resolved and resolved not in seen_links:
                seen_links.add(resolved)
                links.append(resolved)
        doc.links = links
        docs.append(doc)
    return OKFBundle(documents=docs)


def write_bundle(bundle: OKFBundle, bundle_dir: Path) -> list[Path]:
    """Write every document in ``bundle`` to ``bundle_dir`` at its ``rel_path``.

    Returns the list of written file paths. Documents without a ``rel_path`` are skipped.
    """
    bundle_dir = bundle_dir.expanduser().resolve()
    written: list[Path] = []
    for doc in bundle.documents:
        if not doc.rel_path:
            continue
        target = bundle_dir / doc.rel_path
        target.parent.mkdir(parents=True, exist_ok=True)
        atomic_write_text(target, doc.to_markdown())
        written.append(target)
    return written


def validate_bundle(bundle: OKFBundle) -> list[str]:
    """Return human-readable validation problems for ``bundle`` (empty list == valid).

    Checks the OKF invariants DevCouncil relies on: every document declares a ``type``,
    and every intra-bundle link resolves to a document actually present in the bundle.
    """
    problems: list[str] = []
    present = set(bundle.by_path().keys())
    for doc in bundle.documents:
        where = doc.rel_path or doc.title or "<unknown>"
        if not doc.type.strip():
            problems.append(f"{where}: missing required 'type' frontmatter field")
        for link in doc.links:
            if link not in present:
                problems.append(f"{where}: broken link to '{link}' (no such document in bundle)")
    return problems
