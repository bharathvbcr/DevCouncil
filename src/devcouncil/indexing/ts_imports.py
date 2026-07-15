"""Tree-sitter import / symbol extraction for Rust, Go, and JS/TS.

``tree-sitter`` and ``tree-sitter-language-pack`` are core dependencies. Helpers
still return ``None`` on parse failure so callers can keep regex/AST fallbacks.
"""

from __future__ import annotations

import logging
from functools import lru_cache
from typing import Iterable, List, Optional, Sequence, Tuple

logger = logging.getLogger(__name__)

# language name for tree-sitter-language-pack → file suffixes that use it
_LANG_BY_SUFFIX = {
    ".rs": "rust",
    ".go": "go",
    ".ts": "typescript",
    ".tsx": "tsx",
    ".js": "javascript",
    ".jsx": "javascript",
    ".mjs": "javascript",
    ".cjs": "javascript",
}

_SymbolHit = Tuple[str, str, int, str]  # kind, name, line, text


def tree_sitter_available() -> bool:
    """True when both the core binding and language pack import cleanly."""
    try:
        import tree_sitter  # noqa: F401
        from tree_sitter_language_pack import get_parser  # noqa: F401
    except Exception:
        return False
    return True


@lru_cache(maxsize=16)
def _parser_for(language: str):
    if not tree_sitter_available():
        return None
    try:
        from tree_sitter_language_pack import get_parser

        return get_parser(language)
    except Exception:
        logger.debug("tree-sitter parser unavailable for %s", language, exc_info=True)
        return None


def _parse(language: str, source: bytes):
    parser = _parser_for(language)
    if parser is None:
        return None
    try:
        return parser.parse(source)
    except Exception:
        logger.debug("tree-sitter parse failed for %s", language, exc_info=True)
        return None


def _node_text(node, source: bytes) -> str:
    return source[node.start_byte : node.end_byte].decode("utf-8", errors="replace")


def _walk(node) -> Iterable:
    yield node
    for child in node.children:
        yield from _walk(child)


# ---------------------------------------------------------------------------
# Go imports
# ---------------------------------------------------------------------------


def extract_go_import_specs(source: str | bytes) -> Optional[List[str]]:
    """File-level Go import path strings, or None when tree-sitter is unavailable."""
    if not tree_sitter_available():
        return None
    raw = source.encode("utf-8") if isinstance(source, str) else source
    tree = _parse("go", raw)
    if tree is None:
        return None
    specs: List[str] = []
    for node in _walk(tree.root_node):
        if node.type != "interpreted_string_literal":
            continue
        # Only count string literals under import_spec.
        parent = node.parent
        while parent is not None and parent.type not in {"import_spec", "source_file"}:
            parent = parent.parent
        if parent is None or parent.type != "import_spec":
            continue
        text = _node_text(node, raw).strip()
        if len(text) >= 2 and text[0] in "\"`" and text[-1] == text[0]:
            text = text[1:-1]
        if text:
            specs.append(text)
    return specs


# ---------------------------------------------------------------------------
# Rust mod / use
# ---------------------------------------------------------------------------


def _rust_path_segments(node, source: bytes) -> List[str]:
    """Flatten a use/mod path node into identifier segments (skip crate/super/self)."""
    segs: List[str] = []

    def collect(n) -> None:
        t = n.type
        if t in {"identifier", "type_identifier", "field_identifier"}:
            segs.append(_node_text(n, source))
            return
        if t in {"crate", "super", "self"}:
            segs.append(t)
            return
        if t == "use_wildcard":
            for c in n.children:
                collect(c)
            return
        if t in {"scoped_identifier", "scoped_type_identifier", "scoped_use_list", "use_list", "use_as_clause"}:
            for c in n.children:
                if c.type in {",", "{", "}", "::", "as", "*"}:
                    continue
                collect(c)
            return
        for c in n.children:
            collect(c)

    collect(node)
    return segs


def extract_rust_import_refs(source: str | bytes) -> Optional[List[dict]]:
    """Extract Rust ``mod`` / ``use`` references.

    Returns a list of dicts:
      ``{"kind": "mod", "name": "foo"}`` for ``mod foo;`` (file-level, no body)
      ``{"kind": "use", "segments": ["crate", "foo", "Bar"]}`` for use paths

    ``None`` when tree-sitter is unavailable.
    """
    if not tree_sitter_available():
        return None
    raw = source.encode("utf-8") if isinstance(source, str) else source
    tree = _parse("rust", raw)
    if tree is None:
        return None
    refs: List[dict] = []
    for node in tree.root_node.children:
        if node.type == "mod_item":
            # File-level mod only when there is no declaration_list body.
            has_body = any(c.type == "declaration_list" for c in node.children)
            name = None
            for c in node.children:
                if c.type == "identifier":
                    name = _node_text(c, raw)
                    break
            if name and not has_body:
                refs.append({"kind": "mod", "name": name})
        elif node.type == "use_declaration":
            # Path is the non-keyword child after `use` / visibility.
            path_node = None
            for c in node.children:
                if c.type in {"use", "visibility_modifier", ";"}:
                    continue
                path_node = c
                break
            if path_node is None:
                continue
            segments = _rust_path_segments(path_node, raw)
            if segments:
                refs.append({"kind": "use", "segments": segments})
    return refs


# ---------------------------------------------------------------------------
# Symbol matching (AstMatcher)
# ---------------------------------------------------------------------------


def extract_symbols(language: str, source: str | bytes) -> Optional[List[_SymbolHit]]:
    """Named top-level (and nested) definitions for ``language``.

    ``language`` is an AstMatcher language id: typescript, javascript, go, rust.
    Returns ``None`` when tree-sitter cannot parse (caller should fall back).
    """
    if not tree_sitter_available():
        return None
    lang_key = {
        "typescript": "typescript",
        "javascript": "javascript",
        "go": "go",
        "rust": "rust",
    }.get(language)
    if lang_key is None:
        return None
    raw = source.encode("utf-8") if isinstance(source, str) else source
    # Prefer tsx grammar when JSX-looking; callers pass typescript for .tsx too.
    tree = _parse(lang_key, raw)
    if tree is None and language == "typescript":
        tree = _parse("tsx", raw)
    if tree is None:
        return None

    if language in {"typescript", "javascript"}:
        return _symbols_js(tree.root_node, raw)
    if language == "go":
        return _symbols_go(tree.root_node, raw)
    if language == "rust":
        return _symbols_rust(tree.root_node, raw)
    return None


def _line_of(node) -> int:
    return int(node.start_point[0]) + 1


def _first_named(node, types: Sequence[str]) -> Optional[str]:
    for c in node.children:
        if c.type in types:
            return str(c.text.decode("utf-8", errors="replace"))
    return None


def _symbols_js(root, source: bytes) -> List[_SymbolHit]:
    hits: List[_SymbolHit] = []

    def consider(node, kind: str, name_types: Sequence[str] = ("identifier", "type_identifier")) -> None:
        name = _first_named(node, name_types)
        if not name:
            return
        line = _line_of(node)
        text = source.splitlines()[line - 1].decode("utf-8", errors="replace").strip() if line else name
        hits.append((kind, name, line, text))

    def walk(node) -> None:
        t = node.type
        if t == "function_declaration":
            consider(node, "function")
        elif t == "class_declaration":
            consider(node, "class", ("type_identifier", "identifier"))
        elif t == "interface_declaration":
            consider(node, "interface", ("type_identifier", "identifier"))
        elif t == "type_alias_declaration":
            consider(node, "type", ("type_identifier", "identifier"))
        elif t == "lexical_declaration":
            for c in node.children:
                if c.type != "variable_declarator":
                    continue
                name = _first_named(c, ("identifier",))
                if not name:
                    continue
                line = _line_of(c)
                text = (
                    source.splitlines()[line - 1].decode("utf-8", errors="replace").strip()
                    if line
                    else name
                )
                hits.append(("function", name, line, text))
        elif t == "export_statement":
            for c in node.children:
                walk(c)
            return
        for c in node.children:
            walk(c)

    walk(root)
    return hits


def _symbols_go(root, source: bytes) -> List[_SymbolHit]:
    hits: List[_SymbolHit] = []
    for node in _walk(root):
        if node.type == "function_declaration":
            name = _first_named(node, ("identifier",))
            if not name:
                continue
            line = _line_of(node)
            text = source.splitlines()[line - 1].decode("utf-8", errors="replace").strip()
            hits.append(("function", name, line, text))
        elif node.type == "method_declaration":
            name = None
            for c in node.children:
                if c.type == "field_identifier":
                    name = c.text.decode("utf-8", errors="replace")
                    break
            if not name:
                continue
            line = _line_of(node)
            text = source.splitlines()[line - 1].decode("utf-8", errors="replace").strip()
            hits.append(("function", name, line, text))
        elif node.type == "type_declaration":
            for c in node.children:
                if c.type != "type_spec":
                    continue
                name = _first_named(c, ("type_identifier", "identifier"))
                if not name:
                    continue
                kind = "struct"
                for gc in c.children:
                    if gc.type == "struct_type":
                        kind = "struct"
                    elif gc.type == "interface_type":
                        kind = "interface"
                    elif gc.type == "type_identifier":
                        continue
                line = _line_of(c)
                text = source.splitlines()[line - 1].decode("utf-8", errors="replace").strip()
                hits.append((kind, name, line, text))
    return hits


def _symbols_rust(root, source: bytes) -> List[_SymbolHit]:
    hits: List[_SymbolHit] = []
    kind_map = {
        "function_item": "function",
        "struct_item": "struct",
        "enum_item": "enum",
        "trait_item": "trait",
        "type_item": "type",
    }
    for node in _walk(root):
        kind = kind_map.get(node.type)
        if not kind:
            continue
        name = _first_named(node, ("identifier", "type_identifier"))
        if not name:
            continue
        line = _line_of(node)
        text = source.splitlines()[line - 1].decode("utf-8", errors="replace").strip()
        hits.append((kind, name, line, text))
    return hits


def language_for_suffix(suffix: str) -> Optional[str]:
    """Map a file suffix to a language-pack name, or None."""
    return _LANG_BY_SUFFIX.get(suffix.lower())
