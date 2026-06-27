"""Render an OKF bundle as a self-contained, browsable static HTML site.

Parity with the upstream OKF HTML visualizer: a directory of markdown nodes becomes a
directory of HTML pages — one ``index.html`` listing every document grouped by OKF
``type``, plus one page per document showing its frontmatter header and rendered body.
Intra-bundle markdown links (the relative ``*.md`` targets that
:func:`devcouncil.knowledge.okf.read_bundle` resolves into :attr:`OKFDocument.links`) are
rewritten to point at the generated ``*.html`` pages so navigation works offline; external
URLs and the ``resource`` field are preserved as-is.

Security: every string taken from the bundle — titles, descriptions, tags, body text,
code, link labels, and link targets — is HTML-escaped, and link hrefs are scheme-checked
so a hostile or malformed bundle cannot inject markup or a ``javascript:`` href. Only the
structural HTML this module emits is literal. The output is one inlined ``<style>`` block
with no external assets, no network calls, and no JavaScript.
"""

from __future__ import annotations

import html
import posixpath
import re
from pathlib import Path

from devcouncil.knowledge.okf import OKFBundle, OKFDocument, _resolve_link

# Inline markdown constructs. Applied in a deliberate order (code, then links, then
# bold/italic on already-escaped text) via placeholder stashing so escaping never mangles
# generated tags and generated tags never get re-parsed.
_CODE_RE = re.compile(r"`([^`]+)`")
_LINK_RE = re.compile(r"\[([^\]]+)\]\(([^)]+)\)")
_BOLD_RE = re.compile(r"\*\*([^*]+)\*\*")
_ITALIC_RE = re.compile(r"(?<!\*)\*(?!\*)([^*]+)\*(?!\*)")
_PLACEHOLDER_RE = re.compile("(\\d+)")

_URL_SCHEME_RE = re.compile(r"^[a-zA-Z][a-zA-Z0-9+.-]*://")
_BAD_SCHEME_RE = re.compile(r"^\s*(?:javascript|data|vbscript)\s*:", re.IGNORECASE)

_STYLE = """
body{font:16px/1.5 -apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,sans-serif;
max-width:52rem;margin:2rem auto;padding:0 1rem;color:#1a1a1a;background:#fff}
h1,h2,h3,h4{line-height:1.25}a{color:#0b66c3}code{background:#f3f3f3;padding:.1em .3em;
border-radius:3px;font-size:.9em}pre{background:#f6f8fa;padding:1rem;border-radius:6px;
overflow:auto}pre code{background:none;padding:0}table{border-collapse:collapse;width:100%}
th,td{border:1px solid #ddd;padding:.4rem .6rem;text-align:left}
.meta{color:#555;font-size:.9em}.tag{display:inline-block;background:#eef3fb;color:#0b66c3;
border-radius:10px;padding:.05em .6em;margin:.1em .2em .1em 0;font-size:.8em}
.doc-list{list-style:none;padding:0}.doc-list li{margin:.4rem 0}
.type-group{margin:1.4rem 0}.crumb{font-size:.9em;margin-bottom:1rem}
""".strip()


def _md_to_html_rel(rel_md: str) -> str:
    """Map a bundle-relative ``*.md`` path to its generated ``*.html`` page path."""
    return rel_md[:-3] + ".html" if rel_md.endswith(".md") else rel_md + ".html"


def _rel_href(src_html: str, dst_html: str) -> str:
    """Relative href from one generated page to another (POSIX, URL-style)."""
    src_dir = posixpath.dirname(src_html)
    return posixpath.relpath(dst_html, src_dir or ".")


def _safe_external(target: str) -> str:
    """Escape an external/anchor link target, neutralizing dangerous schemes to ``#``."""
    if _BAD_SCHEME_RE.match(target):
        return "#"
    return html.escape(target.strip(), quote=True)


def _rewrite_link(target: str, current_md_rel: str, present: set[str]) -> str:
    """Resolve a markdown link target into an escaped, safe href for the current page.

    Intra-bundle ``*.md`` targets that resolve to a document present in the bundle become a
    relative link to the generated ``*.html`` page (anchors preserved); everything else
    (external URLs, mailto, in-page anchors, unresolved targets) is escaped and scheme-
    checked but otherwise left to behave as a normal link.
    """
    t = target.strip()
    if not t or _BAD_SCHEME_RE.match(t):
        return "#"
    if t.startswith("#") or t.startswith("mailto:") or _URL_SCHEME_RE.match(t):
        return _safe_external(t)
    base, sep, anchor = t.partition("#")
    resolved = _resolve_link(current_md_rel, base)
    if resolved and resolved in present:
        dst = _md_to_html_rel(resolved)
        href = _rel_href(_md_to_html_rel(current_md_rel), dst)
        if sep:
            href = f"{href}#{anchor}"
        return html.escape(href, quote=True)
    return _safe_external(t)


def _render_inline(text: str, current_md_rel: str, present: set[str]) -> str:
    """Render inline markdown (code, links, bold, italic) with everything escaped."""
    stash: list[str] = []

    def _stash(fragment: str) -> str:
        stash.append(fragment)
        return f"{len(stash) - 1}"

    text = _CODE_RE.sub(lambda m: _stash(f"<code>{html.escape(m.group(1))}</code>"), text)
    text = _LINK_RE.sub(
        lambda m: _stash(
            f'<a href="{_rewrite_link(m.group(2), current_md_rel, present)}">'
            f"{html.escape(m.group(1))}</a>"
        ),
        text,
    )
    text = html.escape(text)
    text = _BOLD_RE.sub(r"<strong>\1</strong>", text)
    text = _ITALIC_RE.sub(r"<em>\1</em>", text)
    return _PLACEHOLDER_RE.sub(lambda m: stash[int(m.group(1))], text)


def _render_table(rows: list[str], current_md_rel: str, present: set[str]) -> str:
    """Render a GitHub-style pipe table (header row + ``---`` separator + body rows)."""
    def cells(line: str) -> list[str]:
        return [c.strip() for c in line.strip().strip("|").split("|")]

    header = cells(rows[0])
    body = [cells(r) for r in rows[2:]]
    head_html = "".join(f"<th>{_render_inline(c, current_md_rel, present)}</th>" for c in header)
    body_html = "".join(
        "<tr>" + "".join(f"<td>{_render_inline(c, current_md_rel, present)}</td>" for c in r) + "</tr>"
        for r in body
    )
    return f"<table><thead><tr>{head_html}</tr></thead><tbody>{body_html}</tbody></table>"


def _is_table_sep(line: str) -> bool:
    return bool(re.fullmatch(r"\s*\|?[\s:|-]*-[\s:|-]*\|?\s*", line)) and "-" in line


def render_markdown(body: str, current_md_rel: str, present: set[str]) -> str:
    """Convert an OKF document body to HTML (minimal, dependency-free, fully escaped)."""
    lines = body.splitlines()
    out: list[str] = []
    i, n = 0, len(lines)
    para: list[str] = []
    list_items: list[str] = []
    list_tag = ""

    def flush_para() -> None:
        if para:
            out.append(f"<p>{_render_inline(' '.join(para), current_md_rel, present)}</p>")
            para.clear()

    def flush_list() -> None:
        nonlocal list_tag
        if list_items:
            inner = "".join(f"<li>{it}</li>" for it in list_items)
            out.append(f"<{list_tag}>{inner}</{list_tag}>")
            list_items.clear()
            list_tag = ""

    while i < n:
        line = lines[i]
        stripped = line.strip()

        if stripped.startswith("```"):  # fenced code block
            flush_para(); flush_list()
            i += 1
            code: list[str] = []
            while i < n and not lines[i].strip().startswith("```"):
                code.append(lines[i]); i += 1
            i += 1  # consume closing fence
            out.append(f"<pre><code>{html.escape(chr(10).join(code))}</code></pre>")
            continue

        if not stripped:  # blank line
            flush_para(); flush_list(); i += 1; continue

        heading = re.match(r"^(#{1,6})\s+(.*)$", stripped)
        if heading:
            flush_para(); flush_list()
            level = len(heading.group(1))
            out.append(f"<h{level}>{_render_inline(heading.group(2), current_md_rel, present)}</h{level}>")
            i += 1; continue

        if re.fullmatch(r"(?:-\s*){3,}|(?:\*\s*){3,}|(?:_\s*){3,}", stripped):
            flush_para(); flush_list(); out.append("<hr>"); i += 1; continue

        if "|" in line and i + 1 < n and _is_table_sep(lines[i + 1]):
            flush_para(); flush_list()
            table_rows = [line, lines[i + 1]]
            i += 2
            while i < n and "|" in lines[i] and lines[i].strip():
                table_rows.append(lines[i]); i += 1
            out.append(_render_table(table_rows, current_md_rel, present))
            continue

        ul = re.match(r"^[-*+]\s+(.*)$", stripped)
        ol = re.match(r"^\d+[.)]\s+(.*)$", stripped)
        if ul or ol:
            flush_para()
            tag = "ul" if ul else "ol"
            if list_tag and list_tag != tag:
                flush_list()
            list_tag = tag
            content = (ul or ol).group(1)
            list_items.append(_render_inline(content, current_md_rel, present))
            i += 1; continue

        flush_list()
        para.append(stripped)
        i += 1

    flush_para(); flush_list()
    return "\n".join(out)


def _page(title: str, breadcrumb: str, content: str) -> str:
    """Wrap rendered content in a complete, self-contained HTML document."""
    return (
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n"
        "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n"
        f"<title>{html.escape(title)}</title>\n<style>{_STYLE}</style>\n</head>\n<body>\n"
        f"{breadcrumb}{content}\n</body>\n</html>\n"
    )


def _doc_header(doc: OKFDocument) -> str:
    """The frontmatter block (type, title, description, resource, tags, timestamp)."""
    parts = [f"<h1>{html.escape(doc.title or doc.rel_path or doc.type)}</h1>"]
    meta: list[str] = []
    if doc.type:
        meta.append(f"<strong>{html.escape(doc.type)}</strong>")
    if doc.timestamp:
        meta.append(html.escape(doc.timestamp))
    if meta:
        parts.append(f'<p class="meta">{" · ".join(meta)}</p>')
    if doc.description:
        parts.append(f"<p>{html.escape(doc.description)}</p>")
    if doc.resource:
        href = _safe_external(doc.resource)
        parts.append(f'<p class="meta">Resource: <a href="{href}">{html.escape(doc.resource)}</a></p>')
    if doc.tags:
        tags = "".join(f'<span class="tag">{html.escape(str(t))}</span>' for t in doc.tags)
        parts.append(f"<p>{tags}</p>")
    return "\n".join(parts)


def render_bundle_html(bundle: OKFBundle) -> dict[str, str]:
    """Render ``bundle`` into a map of ``{relative_html_path: html_string}``.

    Always includes ``index.html``; documents with a ``rel_path`` each get a page at the
    same path with a ``.html`` extension. Documents without a ``rel_path`` are skipped
    (they have no stable node identity to link to), matching :func:`write_bundle`.
    """
    present = set(bundle.by_path().keys())
    pages: dict[str, str] = {}

    # Per-document pages.
    by_type: dict[str, list[OKFDocument]] = {}
    for doc in bundle.documents:
        if not doc.rel_path:
            continue
        by_type.setdefault(doc.type or "Untyped", []).append(doc)
        page_rel = _md_to_html_rel(doc.rel_path)
        depth = page_rel.count("/")
        crumb = f'<p class="crumb"><a href="{"../" * depth}index.html">← Index</a></p>'
        content = _doc_header(doc) + "\n" + render_markdown(doc.body, doc.rel_path, present)
        pages[page_rel] = _page(doc.title or doc.rel_path, crumb, content)

    # Index, grouped by type.
    groups: list[str] = []
    for type_name in sorted(by_type):
        docs = sorted(by_type[type_name], key=lambda d: (d.title or d.rel_path).lower())
        items: list[str] = []
        for doc in docs:
            href = html.escape(_rel_href("index.html", _md_to_html_rel(doc.rel_path)), quote=True)
            label = html.escape(doc.title or doc.rel_path)
            desc = f" — {html.escape(doc.description)}" if doc.description else ""
            items.append(f'<li><a href="{href}">{label}</a><span class="meta">{desc}</span></li>')
        groups.append(
            f'<div class="type-group"><h2>{html.escape(type_name)} '
            f'<span class="meta">({len(docs)})</span></h2>'
            f'<ul class="doc-list">{"".join(items)}</ul></div>'
        )
    body = f"<h1>OKF Bundle</h1><p class=\"meta\">{len(present)} document(s)</p>" + "".join(groups)
    pages["index.html"] = _page("OKF Bundle", "", body)
    return pages


def write_bundle_html(bundle: OKFBundle, out_dir: Path) -> list[Path]:
    """Render ``bundle`` and write the static site under ``out_dir``; return written paths."""
    out_dir = out_dir.expanduser().resolve()
    written: list[Path] = []
    for rel, content in render_bundle_html(bundle).items():
        target = out_dir / rel
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content, encoding="utf-8")
        written.append(target)
    return written
