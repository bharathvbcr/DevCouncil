"""TS/JS/Go/Rust symbol + call extraction via tree-sitter with regex fallback.

devcouncil: allow-unwired — package-private; reached only via graph.cache/build.
"""

from __future__ import annotations

import re
from pathlib import Path
from typing import List, Optional, Set

from devcouncil.indexing.graph.extract_python import (
    ExtractedCall,
    ExtractedImport,
    ExtractedSymbol,
    FileExtraction,
    extract_rationales,
    _enclosing_qualname,
)

# ---------------------------------------------------------------------------
# Regex fallbacks
# ---------------------------------------------------------------------------

_JS_EXPORT_RE = re.compile(
    r"(?m)^\s*export\s+(?:default\s+)?(?:async\s+)?"
    r"(?:function|class|const|let|var)\s+([A-Za-z_][A-Za-z0-9_]*)"
)
_JS_FUNCTION_RE = re.compile(
    r"(?m)^\s*(?:export\s+)?(?:async\s+)?function\s+([A-Za-z_][A-Za-z0-9_]*)\s*\("
)
_JS_CLASS_RE = re.compile(
    r"(?m)^\s*(?:export\s+)?class\s+([A-Za-z_][A-Za-z0-9_]*)"
)
_JS_CALL_RE = re.compile(
    r"\b([A-Za-z_][A-Za-z0-9_]*)\s*\.\s*([A-Za-z_][A-Za-z0-9_]*)\s*\("
    r"|\b([A-Za-z_][A-Za-z0-9_]*)\s*\("
)
_JS_IMPORT_RE = re.compile(
    r"""(?:import|export)\s[^'"]*?from\s*['"](?P<spec>[^'"]+)['"]"""
    r"""|(?:require|import)\s*\(\s*['"](?P<spec2>[^'"]+)['"]\s*\)"""
)
_JS_BARE_IMPORT_RE = re.compile(r"""^\s*import\s*['"](?P<spec>[^'"]+)['"]""", re.M)
_JS_EXPORT_LIST_RE = re.compile(
    r"^\s*export\s*\{([^}]+)\}"
    r"|^\s*export\s+default\s+(?:async\s+)?(?:function\s+)?([A-Za-z_][A-Za-z0-9_]*)",
    re.M,
)
_JS_RATIONALE_RE = re.compile(
    r"^\s*(?://|/\*)\s*(NOTE|WHY)\s*:\s*(.+?)(?:\*/)?$"
    r"|^\s*(?://|/\*).*\b(ADR[- ]?\d+)\b",
    re.M | re.IGNORECASE,
)

_GO_FUNC_RE = re.compile(
    r"(?m)^func\s+(?:\((?P<recv>[^)]+)\)\s+)?(?P<name>[A-Za-z_][A-Za-z0-9_]*)\s*\("
)
_GO_IMPORT_BLOCK_RE = re.compile(r"import\s*\((?P<body>[^)]*)\)", re.DOTALL)
_GO_IMPORT_SINGLE_RE = re.compile(
    r"""^\s*import\s+(?:[A-Za-z_.]\w*\s+)?['"](?P<spec>[^'"]+)['"]""", re.M
)
_GO_IMPORT_SPEC_RE = re.compile(r"""['"](?P<spec>[^'"]+)['"]""")
_GO_CALL_RE = re.compile(
    r"\b([A-Za-z_][A-Za-z0-9_]*)\s*\.\s*([A-Za-z_][A-Za-z0-9_]*)\s*\("
    r"|\b([A-Za-z_][A-Za-z0-9_]*)\s*\("
)
_GO_RECV_TYPE_RE = re.compile(r"\*?\s*([A-Za-z_][A-Za-z0-9_]*)\s*$")

_RUST_FN_RE = re.compile(
    r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)\s*[<(]"
)
_RUST_STRUCT_RE = re.compile(r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?struct\s+([A-Za-z_][A-Za-z0-9_]*)")
_RUST_ENUM_RE = re.compile(r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?enum\s+([A-Za-z_][A-Za-z0-9_]*)")
_RUST_TRAIT_RE = re.compile(r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?trait\s+([A-Za-z_][A-Za-z0-9_]*)")
_RUST_USE_RE = re.compile(r"(?m)^\s*(?:pub\s+)?use\s+([^;]+);")
_RUST_MOD_RE = re.compile(r"(?m)^\s*(?:pub\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*;")
_RUST_CALL_RE = re.compile(
    r"\b([A-Za-z_][A-Za-z0-9_]*)\s*\.\s*([A-Za-z_][A-Za-z0-9_]*)\s*\("
    r"|\b([A-Za-z_][A-Za-z0-9_]*)\s*\("
)
_RUST_RATIONALE_RE = re.compile(
    r"^\s*//[!/]?\s*(NOTE|WHY)\s*:\s*(.+)$"
    r"|^\s*//[!/]?.*\b(ADR[- ]?\d+)\b",
    re.M | re.IGNORECASE,
)

_JS_SUFFIXES = {".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs"}
_JS_CALL_SKIP = {
    "if", "for", "while", "switch", "catch", "function", "return", "await", "new",
    "typeof", "instanceof", "void", "delete", "yield", "import", "super",
}
_GO_CALL_SKIP = {
    "if", "for", "switch", "return", "go", "defer", "func", "make", "new",
    "len", "cap", "append", "copy", "delete", "panic", "recover", "range",
}
_RUST_CALL_SKIP = {
    "if", "for", "while", "loop", "match", "return", "unsafe", "async", "await",
    "box", "move", "ref", "mut", "self", "Self", "super", "crate",
}


def _js_import_specs(source: str) -> List[str]:
    specs: List[str] = []
    for m in _JS_IMPORT_RE.finditer(source):
        spec = m.group("spec") or m.group("spec2")
        if spec:
            specs.append(spec)
    for m in _JS_BARE_IMPORT_RE.finditer(source):
        specs.append(m.group("spec"))
    return specs


def _go_import_specs_regex(source: str) -> List[str]:
    specs: List[str] = []
    for m in _GO_IMPORT_BLOCK_RE.finditer(source):
        specs.extend(_GO_IMPORT_SPEC_RE.findall(m.group("body")))
    for m in _GO_IMPORT_SINGLE_RE.finditer(source):
        specs.append(m.group("spec"))
    return specs


def _line_span(node) -> tuple[int, int]:
    return int(node.start_point[0]) + 1, int(node.end_point[0]) + 1


def _node_text(node, source: bytes) -> str:
    return source[node.start_byte : node.end_byte].decode("utf-8", errors="replace")


def _first_child(node, types: Set[str]):
    for c in node.children:
        if c.type in types:
            return c
    return None


def _first_named_text(node, source: bytes, types: Set[str]) -> str:
    c = _first_child(node, types)
    return _node_text(c, source) if c is not None else ""


def _walk(node):
    yield node
    for child in node.children:
        yield from _walk(child)


def _parse_lang(language: str, source: str):
    try:
        from tree_sitter_language_pack import get_parser
    except Exception:
        return None, b""
    try:
        parser = get_parser(language)
        raw = source.encode("utf-8")
        return parser.parse(raw), raw
    except Exception:
        return None, b""


# ---------------------------------------------------------------------------
# JS / TS regex fallback
# ---------------------------------------------------------------------------


def _js_export_names_regex(source: str) -> Set[str]:
    names: Set[str] = set()
    for m in _JS_EXPORT_RE.finditer(source):
        names.add(m.group(1))
    for m in _JS_EXPORT_LIST_RE.finditer(source):
        if m.group(1):
            for part in m.group(1).split(","):
                part = part.strip()
                if not part:
                    continue
                bits = re.split(r"\s+as\s+", part, flags=re.I)
                local = bits[0].strip()
                if local and local != "default" and re.match(r"^[A-Za-z_]", local):
                    names.add(local)
        if m.group(2):
            names.add(m.group(2))
    return names


def _extract_js_rationales(source: str, symbols: List[ExtractedSymbol]) -> List[ExtractedSymbol]:
    out: List[ExtractedSymbol] = []
    for m in _JS_RATIONALE_RE.finditer(source):
        line = source[: m.start()].count("\n") + 1
        if m.group(1):
            tag = m.group(1).upper()
            text = (m.group(2) or "").strip().rstrip("*/").strip()
            name = f"{tag}: {text}" if text else tag
        else:
            adr = (m.group(3) or "").strip()
            name = f"ADR: {adr}"
        enclosing = _enclosing_qualname(symbols, line)
        out.append(
            ExtractedSymbol(
                kind="rationale",
                name=name[:120],
                qualname=f"$rationale:{line}",
                line=line,
                end_line=line,
                bases=[enclosing] if enclosing else [],
            )
        )
    return out


def _extract_js_regex(path: str, source: str) -> FileExtraction:
    lang = "typescript" if path.endswith((".ts", ".tsx")) else "javascript"
    out = FileExtraction(path=path, language=lang, imports=_js_import_specs(source))
    exported_names = _js_export_names_regex(source)
    seen: set[str] = set()
    for m in _JS_EXPORT_RE.finditer(source):
        name = m.group(1)
        if name in seen:
            continue
        seen.add(name)
        line = source[: m.start()].count("\n") + 1
        kind = "class" if "class" in m.group(0) else "function"
        out.symbols.append(
            ExtractedSymbol(
                kind=kind,
                name=name,
                qualname=name,
                line=line,
                end_line=line,
                exported=True,
            )
        )
    for m in _JS_FUNCTION_RE.finditer(source):
        name = m.group(1)
        if name in seen:
            continue
        seen.add(name)
        line = source[: m.start()].count("\n") + 1
        out.symbols.append(
            ExtractedSymbol(
                kind="function",
                name=name,
                qualname=name,
                line=line,
                end_line=line,
                exported=name in exported_names,
            )
        )
    for m in _JS_CLASS_RE.finditer(source):
        name = m.group(1)
        if name in seen:
            continue
        seen.add(name)
        line = source[: m.start()].count("\n") + 1
        out.symbols.append(
            ExtractedSymbol(
                kind="class",
                name=name,
                qualname=name,
                line=line,
                end_line=line,
                exported=name in exported_names,
            )
        )
    for m in _JS_CALL_RE.finditer(source):
        if m.group(1) and m.group(2):
            out.calls.append(
                ExtractedCall(
                    name=m.group(2),
                    line=source[: m.start()].count("\n") + 1,
                    receiver=m.group(1),
                    qualname_hint=f"{m.group(1)}.{m.group(2)}",
                )
            )
        elif m.group(3):
            name = m.group(3)
            if name in _JS_CALL_SKIP:
                continue
            out.calls.append(
                ExtractedCall(
                    name=name,
                    line=source[: m.start()].count("\n") + 1,
                    qualname_hint=name,
                )
            )
    # Mark export-list names
    for sym in out.symbols:
        if sym.name in exported_names:
            sym.exported = True
    out.all_exports = sorted(exported_names)
    out.symbols.extend(_extract_js_rationales(source, out.symbols))
    return out


# ---------------------------------------------------------------------------
# JS / TS tree-sitter
# ---------------------------------------------------------------------------


def _js_heritage(node, source: bytes) -> tuple[List[str], List[str]]:
    bases: List[str] = []
    implements: List[str] = []
    heritage = _first_child(node, {"class_heritage"})
    if heritage is None:
        return bases, implements
    for clause in heritage.children:
        if clause.type == "extends_clause":
            for c in clause.children:
                if c.type in {"identifier", "type_identifier", "member_expression"}:
                    bases.append(_node_text(c, source))
        elif clause.type == "implements_clause":
            for c in clause.children:
                if c.type in {"identifier", "type_identifier", "generic_type"}:
                    text = _node_text(c, source)
                    # strip type args: Foo<T> → Foo
                    implements.append(text.split("<", 1)[0].strip())
    return bases, implements


def _collect_js_exports(root, source: bytes) -> tuple[Set[str], List[str], List[ExtractedImport]]:
    """Return (exported local names, import specs, import details / reexports)."""
    exported: Set[str] = set()
    specs: List[str] = []
    details: List[ExtractedImport] = []
    reexports: List[str] = []

    for node in root.children:
        t = node.type
        if t == "import_statement":
            detail = ExtractedImport(module="")
            for c in _walk(node):
                if c.type == "string":
                    spec = _node_text(c, source).strip("'\"")
                    if spec:
                        specs.append(spec)
                        detail.module = spec
                elif c.type == "import_specifier":
                    ids = [x for x in c.children if x.type == "identifier"]
                    if ids:
                        local = _node_text(ids[-1], source)
                        remote = _node_text(ids[0], source)
                        detail.names.append(remote)
                        detail.alias_map[local] = remote
                elif c.type == "identifier" and c.parent is not None and c.parent.type == "import_clause":
                    name = _node_text(c, source)
                    detail.names.append(name)
                    detail.alias_map[name] = "default"
            if detail.module or detail.names:
                details.append(detail)
        elif t == "export_statement":
            # export { X } / export { X } from '...' / export default ...
            has_from = any(c.type == "string" for c in node.children)
            from_spec = ""
            stmt_reexports: List[str] = []  # names re-exported by THIS statement
            for c in node.children:
                if c.type == "string":
                    from_spec = _node_text(c, source).strip("'\"")
                    if from_spec:
                        specs.append(from_spec)
            for c in node.children:
                if c.type == "export_clause":
                    for spec in c.children:
                        if spec.type != "export_specifier":
                            continue
                        ids = [x for x in spec.children if x.type == "identifier"]
                        if not ids:
                            continue
                        local = _node_text(ids[0], source)
                        exported.add(local)
                        if has_from:
                            reexports.append(local)
                            stmt_reexports.append(local)
                elif c.type in {
                    "function_declaration",
                    "class_declaration",
                    "interface_declaration",
                    "type_alias_declaration",
                    "lexical_declaration",
                    "variable_declaration",
                }:
                    name = _first_named_text(
                        c, source, {"identifier", "type_identifier", "property_identifier"}
                    )
                    if c.type == "lexical_declaration":
                        for vd in c.children:
                            if vd.type == "variable_declarator":
                                name = _first_named_text(vd, source, {"identifier"}) or name
                    if name:
                        exported.add(name)
                elif c.type == "identifier" and any(x.type == "default" for x in node.children):
                    exported.add(_node_text(c, source))
            if has_from and from_spec:
                # Only names from THIS statement (the old accumulated-list slice
                # cross-contaminated re-export names across statements).
                details.append(
                    ExtractedImport(module=from_spec, names=stmt_reexports)
                )

    # Fix reexport details: rebuild from export-from statements cleanly
    details = [d for d in details if d.module or d.names]
    return exported, specs, details


def _extract_js_calls(root, source: bytes) -> List[ExtractedCall]:
    calls: List[ExtractedCall] = []
    for node in _walk(root):
        if node.type != "call_expression":
            continue
        func = node.child_by_field_name("function") if hasattr(node, "child_by_field_name") else None
        if func is None and node.children:
            func = node.children[0]
        if func is None:
            continue
        line = int(node.start_point[0]) + 1
        if func.type == "identifier":
            name = _node_text(func, source)
            if name in _JS_CALL_SKIP:
                continue
            calls.append(ExtractedCall(name=name, line=line, qualname_hint=name))
        elif func.type == "member_expression":
            prop = _first_child(func, {"property_identifier", "identifier"})
            if prop is None:
                continue
            name = _node_text(prop, source)
            obj = func.children[0] if func.children else None
            receiver = ""
            if obj is not None and obj.type in {"identifier", "this", "super"}:
                receiver = _node_text(obj, source)
            elif obj is not None:
                receiver = _node_text(obj, source).split(".")[-1]
            hint = f"{receiver}.{name}" if receiver else name
            calls.append(
                ExtractedCall(name=name, line=line, receiver=receiver, qualname_hint=hint)
            )
    return calls


def _extract_js_tree_sitter(path: str, source: str) -> Optional[FileExtraction]:
    from devcouncil.indexing.ts_imports import language_for_suffix

    suffix = Path(path).suffix.lower()
    lang_key = language_for_suffix(suffix)
    if not lang_key:
        return None
    tree, raw = _parse_lang(lang_key, source)
    if tree is None and lang_key == "typescript":
        tree, raw = _parse_lang("tsx", source)
    if tree is None:
        return None

    lang = "typescript" if suffix in {".ts", ".tsx"} else "javascript"
    out = FileExtraction(path=path, language=lang)
    root = tree.root_node
    exported, specs, details = _collect_js_exports(root, raw)
    # Also pick up require/import() via regex
    for spec in _js_import_specs(source):
        if spec not in specs:
            specs.append(spec)
    out.imports = specs
    out.import_details = details
    out.all_exports = sorted(exported)
    out.reexports = []

    # Re-scan export-from for reexports list
    for node in root.children:
        if node.type != "export_statement":
            continue
        if not any(c.type == "string" for c in node.children):
            continue
        for c in node.children:
            if c.type != "export_clause":
                continue
            for spec in c.children:
                if spec.type != "export_specifier":
                    continue
                ids = [x for x in spec.children if x.type == "identifier"]
                if ids:
                    out.reexports.append(_node_text(ids[0], raw))

    seen_qual: Set[str] = set()

    def add_sym(
        kind: str,
        name: str,
        qualname: str,
        node,
        *,
        bases: Optional[List[str]] = None,
        implements: Optional[List[str]] = None,
        force_exported: Optional[bool] = None,
    ) -> None:
        if not name or qualname in seen_qual:
            return
        seen_qual.add(qualname)
        start, end = _line_span(node)
        if force_exported is not None:
            exp = force_exported
        else:
            exp = name in exported
        out.symbols.append(
            ExtractedSymbol(
                kind=kind,
                name=name,
                qualname=qualname,
                line=start,
                end_line=end,
                bases=bases or [],
                implements=implements or [],
                exported=exp,
            )
        )

    def walk_decl(node, under_export: bool = False) -> None:
        t = node.type
        if t == "export_statement":
            for c in node.children:
                walk_decl(c, under_export=True)
            return
        if t == "function_declaration":
            name = _first_named_text(node, raw, {"identifier"})
            add_sym("function", name, name, node, force_exported=True if under_export else None)
            return
        if t == "class_declaration":
            name = _first_named_text(node, raw, {"type_identifier", "identifier"})
            bases, implements = _js_heritage(node, raw)
            add_sym(
                "class",
                name,
                name,
                node,
                bases=bases,
                implements=implements,
                force_exported=True if under_export else None,
            )
            body = _first_child(node, {"class_body"})
            if body is not None and name:
                for child in body.children:
                    if child.type != "method_definition":
                        continue
                    # skip constructors as methods with name constructor still recorded
                    mname = _first_named_text(
                        child, raw, {"property_identifier", "identifier"}
                    )
                    if not mname:
                        continue
                    add_sym("method", mname, f"{name}.{mname}", child, force_exported=False)
            return
        if t == "interface_declaration":
            name = _first_named_text(node, raw, {"type_identifier", "identifier"})
            bases, _impl = _js_heritage(node, raw)
            # interfaces use extends_type_clause sometimes
            for c in node.children:
                if c.type in {"extends_type_clause", "extends_clause"}:
                    for gc in c.children:
                        if gc.type in {"identifier", "type_identifier"}:
                            bases.append(_node_text(gc, raw))
            add_sym(
                "interface",
                name,
                name,
                node,
                bases=bases,
                force_exported=True if under_export else None,
            )
            return
        if t == "type_alias_declaration":
            name = _first_named_text(node, raw, {"type_identifier", "identifier"})
            add_sym(
                "type",
                name,
                name,
                node,
                force_exported=True if under_export else None,
            )
            return
        if t in {"lexical_declaration", "variable_declaration"}:
            for c in node.children:
                if c.type != "variable_declarator":
                    continue
                name = _first_named_text(c, raw, {"identifier"})
                if not name:
                    continue
                # Only count if value looks like a function/arrow, or is exported
                val = None
                for gc in c.children:
                    if gc.type not in {"identifier", "=", "type_annotation"}:
                        val = gc
                        break
                is_fn = val is not None and val.type in {
                    "arrow_function",
                    "function",
                    "function_expression",
                }
                if is_fn or under_export or name in exported:
                    add_sym(
                        "function",
                        name,
                        name,
                        c,
                        force_exported=True if under_export or name in exported else False,
                    )
            return

    for child in root.children:
        walk_decl(child)

    # Mark any remaining exported names that were only listed via export { X }
    by_name = {s.name: s for s in out.symbols if s.kind != "rationale"}
    for name in exported:
        if name in by_name:
            by_name[name].exported = True

    out.calls = _extract_js_calls(root, raw)
    out.symbols.extend(_extract_js_rationales(source, out.symbols))
    return out


# ---------------------------------------------------------------------------
# Go
# ---------------------------------------------------------------------------


def _go_receiver_type(recv_list, source: bytes) -> str:
    """Extract type name from method receiver parameter_list."""
    if recv_list is None:
        return ""
    for c in recv_list.children:
        if c.type != "parameter_declaration":
            continue
        for gc in c.children:
            if gc.type == "type_identifier":
                return _node_text(gc, source)
            if gc.type == "pointer_type":
                tid = _first_child(gc, {"type_identifier"})
                if tid is not None:
                    return _node_text(tid, source)
                return _node_text(gc, source).lstrip("*").strip()
    return ""


def _extract_go_tree_sitter(path: str, source: str) -> Optional[FileExtraction]:
    tree, raw = _parse_lang("go", source)
    if tree is None:
        return None
    out = FileExtraction(path=path, language="go")
    try:
        from devcouncil.indexing.ts_imports import extract_go_import_specs

        specs = extract_go_import_specs(source)
        if specs is not None:
            out.imports = specs
        else:
            out.imports = _go_import_specs_regex(source)
    except Exception:
        out.imports = _go_import_specs_regex(source)

    for node in _walk(tree.root_node):
        if node.type == "function_declaration":
            name = _first_named_text(node, raw, {"identifier"})
            if not name:
                continue
            start, end = _line_span(node)
            out.symbols.append(
                ExtractedSymbol(
                    kind="function",
                    name=name,
                    qualname=name,
                    line=start,
                    end_line=end,
                    exported=name[:1].isupper(),
                )
            )
        elif node.type == "method_declaration":
            name = _first_named_text(node, raw, {"field_identifier"})
            if not name:
                continue
            # First parameter_list is the receiver
            recv_type = ""
            for c in node.children:
                if c.type == "parameter_list":
                    recv_type = _go_receiver_type(c, raw)
                    break
            qual = f"{recv_type}.{name}" if recv_type else name
            start, end = _line_span(node)
            out.symbols.append(
                ExtractedSymbol(
                    kind="method",
                    name=name,
                    qualname=qual,
                    line=start,
                    end_line=end,
                    exported=name[:1].isupper(),
                )
            )
        elif node.type == "type_spec":
            name = _first_named_text(node, raw, {"type_identifier", "identifier"})
            if not name:
                continue
            kind = "type"
            for c in node.children:
                if c.type == "struct_type":
                    kind = "struct"
                elif c.type == "interface_type":
                    kind = "interface"
            start, end = _line_span(node)
            out.symbols.append(
                ExtractedSymbol(
                    kind=kind,
                    name=name,
                    qualname=name,
                    line=start,
                    end_line=end,
                    exported=name[:1].isupper(),
                )
            )
        elif node.type == "call_expression":
            func = node.children[0] if node.children else None
            if func is None:
                continue
            line = int(node.start_point[0]) + 1
            if func.type == "identifier":
                name = _node_text(func, raw)
                if name in _GO_CALL_SKIP:
                    continue
                out.calls.append(ExtractedCall(name=name, line=line, qualname_hint=name))
            elif func.type == "selector_expression":
                field = _first_child(func, {"field_identifier"})
                if field is None:
                    continue
                name = _node_text(field, raw)
                recv_node = func.children[0] if func.children else None
                receiver = _node_text(recv_node, raw) if recv_node else ""
                out.calls.append(
                    ExtractedCall(
                        name=name,
                        line=line,
                        receiver=receiver,
                        qualname_hint=f"{receiver}.{name}" if receiver else name,
                    )
                )
    out.symbols.extend(extract_rationales(source, out.symbols))  # # style rare in Go
    return out


def _extract_go_regex(path: str, source: str) -> FileExtraction:
    out = FileExtraction(path=path, language="go", imports=_go_import_specs_regex(source))
    try:
        from devcouncil.indexing.ts_imports import extract_go_import_specs

        ts_specs = extract_go_import_specs(source)
        if ts_specs is not None:
            out.imports = ts_specs
    except Exception:
        pass
    for m in _GO_FUNC_RE.finditer(source):
        name = m.group("name")
        recv = (m.group("recv") or "").strip()
        line = source[: m.start()].count("\n") + 1
        if recv:
            rt = _GO_RECV_TYPE_RE.search(recv)
            rtype = rt.group(1) if rt else ""
            qual = f"{rtype}.{name}" if rtype else name
            out.symbols.append(
                ExtractedSymbol(
                    kind="method",
                    name=name,
                    qualname=qual,
                    line=line,
                    end_line=line,
                    exported=name[:1].isupper() if name else False,
                )
            )
        else:
            out.symbols.append(
                ExtractedSymbol(
                    kind="function",
                    name=name,
                    qualname=name,
                    line=line,
                    end_line=line,
                    exported=name[:1].isupper() if name else False,
                )
            )
    for m in _GO_CALL_RE.finditer(source):
        if m.group(1) and m.group(2):
            out.calls.append(
                ExtractedCall(
                    name=m.group(2),
                    line=source[: m.start()].count("\n") + 1,
                    receiver=m.group(1),
                    qualname_hint=f"{m.group(1)}.{m.group(2)}",
                )
            )
        elif m.group(3):
            name = m.group(3)
            if name in _GO_CALL_SKIP:
                continue
            out.calls.append(
                ExtractedCall(
                    name=name,
                    line=source[: m.start()].count("\n") + 1,
                    qualname_hint=name,
                )
            )
    return out


# ---------------------------------------------------------------------------
# Rust
# ---------------------------------------------------------------------------


def _extract_rust_rationales(source: str, symbols: List[ExtractedSymbol]) -> List[ExtractedSymbol]:
    out: List[ExtractedSymbol] = []
    for m in _RUST_RATIONALE_RE.finditer(source):
        line = source[: m.start()].count("\n") + 1
        if m.group(1):
            tag = m.group(1).upper()
            text = (m.group(2) or "").strip()
            name = f"{tag}: {text}" if text else tag
        else:
            name = f"ADR: {(m.group(3) or '').strip()}"
        enclosing = _enclosing_qualname(symbols, line)
        out.append(
            ExtractedSymbol(
                kind="rationale",
                name=name[:120],
                qualname=f"$rationale:{line}",
                line=line,
                end_line=line,
                bases=[enclosing] if enclosing else [],
            )
        )
    return out


def _rust_is_pub(node) -> bool:
    return any(c.type == "visibility_modifier" for c in node.children)


def _extract_rust_tree_sitter(path: str, source: str) -> Optional[FileExtraction]:
    tree, raw = _parse_lang("rust", source)
    if tree is None:
        return None
    out = FileExtraction(path=path, language="rust")
    try:
        from devcouncil.indexing.ts_imports import extract_rust_import_refs

        refs = extract_rust_import_refs(source)
        if refs is not None:
            for ref in refs:
                if ref.get("kind") == "mod":
                    name = ref.get("name") or ""
                    if name:
                        out.imports.append(f"mod:{name}")
                elif ref.get("kind") == "use":
                    segs = ref.get("segments") or []
                    if segs:
                        out.imports.append("::".join(segs))
                        out.import_details.append(
                            ExtractedImport(
                                module="::".join(segs[:-1]) if len(segs) > 1 else "::".join(segs),
                                names=[segs[-1]] if segs else [],
                                alias_map={segs[-1]: "::".join(segs)} if segs else {},
                            )
                        )
    except Exception:
        pass

    for node in tree.root_node.children:
        t = node.type
        if t == "function_item":
            name = _first_named_text(node, raw, {"identifier"})
            if not name:
                continue
            start, end = _line_span(node)
            out.symbols.append(
                ExtractedSymbol(
                    kind="function",
                    name=name,
                    qualname=name,
                    line=start,
                    end_line=end,
                    exported=_rust_is_pub(node),
                )
            )
        elif t in {"struct_item", "enum_item", "trait_item", "type_item"}:
            kind = {
                "struct_item": "struct",
                "enum_item": "enum",
                "trait_item": "trait",
                "type_item": "type",
            }[t]
            name = _first_named_text(node, raw, {"type_identifier", "identifier"})
            if not name:
                continue
            start, end = _line_span(node)
            out.symbols.append(
                ExtractedSymbol(
                    kind=kind,
                    name=name,
                    qualname=name,
                    line=start,
                    end_line=end,
                    exported=_rust_is_pub(node),
                )
            )
            if t == "trait_item":
                body = _first_child(node, {"declaration_list"})
                if body is not None:
                    for child in body.children:
                        if child.type not in {"function_item", "function_signature_item"}:
                            continue
                        mname = _first_named_text(child, raw, {"identifier"})
                        if not mname:
                            continue
                        ms, me = _line_span(child)
                        out.symbols.append(
                            ExtractedSymbol(
                                kind="method",
                                name=mname,
                                qualname=f"{name}.{mname}",
                                line=ms,
                                end_line=me,
                                exported=_rust_is_pub(node),
                            )
                        )
        elif t == "impl_item":
            # impl Type { ... }  or  impl Trait for Type { ... }
            type_ids = [c for c in node.children if c.type == "type_identifier"]
            trait_name = ""
            type_name = ""
            if any(c.type == "for" for c in node.children):
                if len(type_ids) >= 2:
                    trait_name = _node_text(type_ids[0], raw)
                    type_name = _node_text(type_ids[1], raw)
                elif len(type_ids) == 1:
                    type_name = _node_text(type_ids[0], raw)
            elif type_ids:
                type_name = _node_text(type_ids[0], raw)
            if type_name and trait_name:
                # Record implements on a synthetic note via a type-level symbol update later
                # Ensure type exists; attach implements list by appending a marker symbol update
                existing = next((s for s in out.symbols if s.qualname == type_name), None)
                if existing is not None:
                    if trait_name not in existing.implements:
                        existing.implements.append(trait_name)
                else:
                    start, end = _line_span(node)
                    out.symbols.append(
                        ExtractedSymbol(
                            kind="struct",
                            name=type_name,
                            qualname=type_name,
                            line=start,
                            end_line=end,
                            implements=[trait_name],
                            exported=False,
                        )
                    )
            body = _first_child(node, {"declaration_list"})
            if body is not None and type_name:
                for child in body.children:
                    if child.type != "function_item":
                        continue
                    mname = _first_named_text(child, raw, {"identifier"})
                    if not mname:
                        continue
                    ms, me = _line_span(child)
                    out.symbols.append(
                        ExtractedSymbol(
                            kind="method",
                            name=mname,
                            qualname=f"{type_name}.{mname}",
                            line=ms,
                            end_line=me,
                            exported=_rust_is_pub(child),
                            bases=[trait_name] if trait_name else [],
                        )
                    )

    for node in _walk(tree.root_node):
        if node.type != "call_expression":
            continue
        func = node.children[0] if node.children else None
        if func is None:
            continue
        line = int(node.start_point[0]) + 1
        if func.type == "identifier":
            name = _node_text(func, raw)
            if name in _RUST_CALL_SKIP:
                continue
            out.calls.append(ExtractedCall(name=name, line=line, qualname_hint=name))
        elif func.type == "field_expression":
            field = _first_child(func, {"field_identifier", "identifier"})
            if field is None:
                continue
            name = _node_text(field, raw)
            recv = func.children[0] if func.children else None
            receiver = _node_text(recv, raw) if recv is not None else ""
            out.calls.append(
                ExtractedCall(
                    name=name,
                    line=line,
                    receiver=receiver,
                    qualname_hint=f"{receiver}.{name}" if receiver else name,
                )
            )

    out.symbols.extend(_extract_rust_rationales(source, out.symbols))
    return out


def _extract_rust_regex(path: str, source: str) -> FileExtraction:
    out = FileExtraction(path=path, language="rust")
    for m in _RUST_USE_RE.finditer(source):
        path_s = m.group(1).strip()
        out.imports.append(path_s.replace(" ", ""))
    for m in _RUST_MOD_RE.finditer(source):
        out.imports.append(f"mod:{m.group(1)}")
    for kind, rx in (
        ("struct", _RUST_STRUCT_RE),
        ("enum", _RUST_ENUM_RE),
        ("trait", _RUST_TRAIT_RE),
        ("function", _RUST_FN_RE),
    ):
        for m in rx.finditer(source):
            name = m.group(1)
            line = source[: m.start()].count("\n") + 1
            exported = "pub" in m.group(0)
            out.symbols.append(
                ExtractedSymbol(
                    kind=kind,
                    name=name,
                    qualname=name,
                    line=line,
                    end_line=line,
                    exported=exported,
                )
            )
    for m in _RUST_CALL_RE.finditer(source):
        if m.group(1) and m.group(2):
            out.calls.append(
                ExtractedCall(
                    name=m.group(2),
                    line=source[: m.start()].count("\n") + 1,
                    receiver=m.group(1),
                    qualname_hint=f"{m.group(1)}.{m.group(2)}",
                )
            )
        elif m.group(3):
            name = m.group(3)
            if name in _RUST_CALL_SKIP:
                continue
            out.calls.append(
                ExtractedCall(
                    name=name,
                    line=source[: m.start()].count("\n") + 1,
                    qualname_hint=name,
                )
            )
    out.symbols.extend(_extract_rust_rationales(source, out.symbols))
    return out


# ---------------------------------------------------------------------------
# Public API
# ---------------------------------------------------------------------------


def extract_ts_js(path: str, source: str) -> FileExtraction:
    """Extract from a TS/JS file (tree-sitter when available, else regex)."""
    ts = _extract_js_tree_sitter(path, source)
    if ts is not None:
        return ts
    return _extract_js_regex(path, source)


def extract_go(path: str, source: str) -> FileExtraction:
    """Extract from a Go file (tree-sitter preferred)."""
    ts = _extract_go_tree_sitter(path, source)
    if ts is not None:
        return ts
    return _extract_go_regex(path, source)


def extract_rust(path: str, source: str) -> FileExtraction:
    """Extract from a Rust file (tree-sitter preferred)."""
    ts = _extract_rust_tree_sitter(path, source)
    if ts is not None:
        return ts
    return _extract_rust_regex(path, source)


def extract_file(path: str, source: str) -> FileExtraction:
    """Dispatch extraction by file suffix."""
    lower = path.lower()
    if lower.endswith(".py"):
        from devcouncil.indexing.graph.extract_python import extract_python

        return extract_python(path, source)
    if any(lower.endswith(s) for s in _JS_SUFFIXES):
        return extract_ts_js(path, source)
    if lower.endswith(".go"):
        return extract_go(path, source)
    if lower.endswith(".rs"):
        return extract_rust(path, source)
    return FileExtraction(path=path, language="")
