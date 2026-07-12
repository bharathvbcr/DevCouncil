"""Python AST extraction: symbols, calls, imports, bases, __all__, rationales.

devcouncil: allow-unwired — package-private; reached only via graph.build.
"""

from __future__ import annotations

import ast
import re
from dataclasses import dataclass, field
from typing import List, Optional


@dataclass
class ExtractedSymbol:
    kind: str  # function | class | method | interface | type | struct | enum | trait | rationale
    name: str
    qualname: str
    line: int
    end_line: int
    bases: List[str] = field(default_factory=list)
    implements: List[str] = field(default_factory=list)
    decorators: List[str] = field(default_factory=list)
    exported: bool = False  # in __all__ or top-level public


@dataclass
class ExtractedCall:
    name: str
    line: int
    receiver: str = ""  # "self", module alias, or ""
    qualname_hint: str = ""  # receiver.name when known


@dataclass
class ExtractedImport:
    module: str
    names: List[str] = field(default_factory=list)  # imported names (empty for bare import)
    alias_map: dict = field(default_factory=dict)  # local name -> module.attr or module


@dataclass
class FileExtraction:
    path: str
    language: str
    imports: List[str] = field(default_factory=list)  # module strings (cache-compatible)
    import_details: List[ExtractedImport] = field(default_factory=list)
    symbols: List[ExtractedSymbol] = field(default_factory=list)
    calls: List[ExtractedCall] = field(default_factory=list)
    all_exports: List[str] = field(default_factory=list)
    reexports: List[str] = field(default_factory=list)  # names re-exported from imports
    # Non-call name references (Name loads / attribute accesses). Used by
    # liveness to avoid false-dead flags on callbacks, registry-dispatched
    # handlers, and @property-style attribute access.
    references: List[str] = field(default_factory=list)


_RATIONALE_RE = re.compile(
    r"^\s*#\s*(NOTE|WHY)\s*:\s*(.+)$"
    r"|^\s*#\s*.*\b(ADR[- ]?\d+)\b.*$",
    re.M | re.IGNORECASE,
)


def _decorator_name(node: ast.AST) -> str:
    if isinstance(node, ast.Name):
        return str(node.id)
    if isinstance(node, ast.Attribute):
        base = _decorator_name(node.value)
        return f"{base}.{node.attr}" if base else node.attr
    if isinstance(node, ast.Call):
        return _decorator_name(node.func)
    return ""


def _call_parts(node: ast.AST) -> tuple[str, str]:
    """Return (receiver, name) for a call func expression."""
    if isinstance(node, ast.Name):
        return "", node.id
    if isinstance(node, ast.Attribute):
        if isinstance(node.value, ast.Name):
            return node.value.id, node.attr
        if isinstance(node.value, ast.Call):
            # Foo().bar() / pkg.Foo().bar() → treat constructor as receiver class
            func = node.value.func
            if isinstance(func, ast.Name):
                return func.id, node.attr
            if isinstance(func, ast.Attribute):
                return _decorator_name(func), node.attr
            return "", node.attr
        if isinstance(node.value, ast.Attribute):
            # a.b.c() → receiver a.b, name c — keep outermost attr as receiver hint
            return _decorator_name(node.value), node.attr
        return "", node.attr
    return "", ""


def _enclosing_qualname(symbols: List[ExtractedSymbol], line: int) -> str:
    """Innermost non-rationale symbol whose span covers ``line``."""
    best = ""
    best_span = None
    for sym in symbols:
        if sym.kind == "rationale":
            continue
        if sym.line <= line <= sym.end_line:
            span = sym.end_line - sym.line
            if best_span is None or span <= best_span:
                best = sym.qualname
                best_span = span
    return best


def extract_rationales(source: str, symbols: List[ExtractedSymbol]) -> List[ExtractedSymbol]:
    """Capture ``# NOTE:`` / ``# WHY:`` / ADR-reference comments as rationale nodes."""
    out: List[ExtractedSymbol] = []
    for m in _RATIONALE_RE.finditer(source):
        line = source[: m.start()].count("\n") + 1
        if m.group(1):
            tag = m.group(1).upper()
            text = (m.group(2) or "").strip()
            name = f"{tag}: {text}" if text else tag
        else:
            adr = (m.group(3) or "").strip()
            name = f"ADR: {adr}" if adr else m.group(0).strip().lstrip("#").strip()
        enclosing = _enclosing_qualname(symbols, line)
        out.append(
            ExtractedSymbol(
                kind="rationale",
                name=name[:120],
                qualname=f"$rationale:{line}",
                line=line,
                end_line=line,
                bases=[enclosing] if enclosing else [],
                exported=False,
            )
        )
    return out


def extract_python(path: str, source: str) -> FileExtraction:
    """Extract symbols, calls, and import module strings from Python source.

    Never raises on syntax errors — returns whatever was recoverable (empty).
    """
    out = FileExtraction(path=path, language="python")
    try:
        tree = ast.parse(source)
    except (SyntaxError, ValueError):
        out.symbols.extend(extract_rationales(source, []))
        return out

    pkg_parts = path[:-3].replace("/", ".").split(".") if path.endswith(".py") else []

    # __all__
    for node in tree.body:
        if isinstance(node, ast.Assign):
            for t in node.targets:
                if isinstance(t, ast.Name) and t.id == "__all__":
                    if isinstance(node.value, (ast.List, ast.Tuple)):
                        for elt in node.value.elts:
                            if isinstance(elt, ast.Constant) and isinstance(elt.value, str):
                                out.all_exports.append(elt.value)

    # Imports
    modules: List[str] = []
    for inode in ast.walk(tree):
        if isinstance(inode, ast.Import):
            detail = ExtractedImport(module="")
            for alias in inode.names:
                modules.append(alias.name)
                local = alias.asname or alias.name.split(".")[0]
                detail.alias_map[local] = alias.name
                detail.names.append(alias.name)
            out.import_details.append(detail)
        elif isinstance(inode, ast.ImportFrom):
            if inode.level:
                base = pkg_parts[: -inode.level] if inode.level <= len(pkg_parts) else []
                base_mod = ".".join(base + ([inode.module] if inode.module else []))
            else:
                base_mod = inode.module or ""
            if base_mod:
                modules.append(base_mod)
            detail = ExtractedImport(module=base_mod)
            for alias in inode.names:
                if alias.name and alias.name != "*":
                    modules.append(f"{base_mod}.{alias.name}" if base_mod else alias.name)
                    local = alias.asname or alias.name
                    detail.names.append(alias.name)
                    detail.alias_map[local] = (
                        f"{base_mod}.{alias.name}" if base_mod else alias.name
                    )
            out.import_details.append(detail)

    # Re-exports: barrel ``__init__.py`` or names also listed in ``__all__``.
    # Do NOT record every module-level import as a re-export of the importer.
    out.reexports = []
    is_init = path.replace("\\", "/").endswith("__init__.py")
    all_set = set(out.all_exports)
    for stmt in tree.body:
        if isinstance(stmt, ast.ImportFrom):
            for alias in stmt.names:
                if alias.name and alias.name != "*":
                    local = alias.asname or alias.name
                    if is_init or local in all_set:
                        out.reexports.append(local)

    out.imports = modules

    # Top-level symbols + methods
    export_set = set(out.all_exports) if out.all_exports else None

    def _add_symbol(
        kind: str,
        name: str,
        qualname: str,
        node: ast.AST,
        bases: Optional[List[str]] = None,
        implements: Optional[List[str]] = None,
    ) -> None:
        start = getattr(node, "lineno", 1) or 1
        end = getattr(node, "end_lineno", start) or start
        decs = [_decorator_name(d) for d in getattr(node, "decorator_list", []) or []]
        exported = False
        if export_set is not None:
            exported = name in export_set
        else:
            exported = not name.startswith("_")
        out.symbols.append(
            ExtractedSymbol(
                kind=kind,
                name=name,
                qualname=qualname,
                line=start,
                end_line=end,
                bases=bases or [],
                implements=implements or [],
                decorators=decs,
                exported=exported,
            )
        )

    for stmt in tree.body:
        if isinstance(stmt, (ast.FunctionDef, ast.AsyncFunctionDef)):
            _add_symbol("function", stmt.name, stmt.name, stmt)
        elif isinstance(stmt, ast.ClassDef):
            bases = []
            for b in stmt.bases:
                bases.append(_decorator_name(b) or "")
            _add_symbol("class", stmt.name, stmt.name, stmt, bases=[b for b in bases if b])
            for child in stmt.body:
                if isinstance(child, (ast.FunctionDef, ast.AsyncFunctionDef)):
                    _add_symbol(
                        "method",
                        child.name,
                        f"{stmt.name}.{child.name}",
                        child,
                    )

    # Calls + non-call references
    call_func_ids = set()
    refs: set[str] = set()
    for cnode in ast.walk(tree):
        if isinstance(cnode, ast.Call):
            call_func_ids.add(id(cnode.func))
            receiver, name = _call_parts(cnode.func)
            if not name:
                continue
            hint = f"{receiver}.{name}" if receiver else name
            out.calls.append(
                ExtractedCall(
                    name=name,
                    line=getattr(cnode, "lineno", 0) or 0,
                    receiver=receiver,
                    qualname_hint=hint,
                )
            )
    for rnode in ast.walk(tree):
        if id(rnode) in call_func_ids:
            continue  # already modeled as a call
        if isinstance(rnode, ast.Name) and isinstance(rnode.ctx, ast.Load):
            refs.add(rnode.id)
        elif isinstance(rnode, ast.Attribute) and isinstance(rnode.ctx, ast.Load):
            refs.add(rnode.attr)
    out.references = sorted(refs)

    # Bare decorators (``@foo`` / ``@mod.foo``) are invocations too — model them
    # as calls so decorator functions get inbound edges and stay live.
    for dnode in ast.walk(tree):
        for dec in getattr(dnode, "decorator_list", []) or []:
            if isinstance(dec, ast.Call):
                continue  # walked as a Call above
            receiver, name = _call_parts(dec) if isinstance(dec, ast.Attribute) else ("", "")
            if isinstance(dec, ast.Name):
                name = dec.id
            if not name:
                continue
            out.calls.append(
                ExtractedCall(
                    name=name,
                    line=getattr(dnode, "lineno", 0) or 0,
                    receiver=receiver,
                    qualname_hint=f"{receiver}.{name}" if receiver else name,
                )
            )

    out.symbols.extend(extract_rationales(source, out.symbols))
    return out
