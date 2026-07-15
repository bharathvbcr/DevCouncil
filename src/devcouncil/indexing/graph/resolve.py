"""Import + call resolution with a confidence ladder.

Reuses RepoMapper's import resolution (Python/JS/Go) so edges stay consistent
with existing verify-gate behavior. ``resolve_import_edges`` resolves from
already-extracted import specs — no source re-reads for extraction.

devcouncil: allow-unwired — package-private; reached only via graph.build.
"""

from __future__ import annotations

import logging
import sys
from collections import defaultdict
from pathlib import Path
from typing import Any, Dict, List, Optional, Set, Tuple

from devcouncil.indexing.graph.extract_python import FileExtraction
from devcouncil.indexing.graph.schema import (
    Confidence,
    GraphEdge,
    GraphNode,
    NodeKind,
    file_node_id,
    symbol_node_id,
)

logger = logging.getLogger(__name__)

_JS_SUFFIXES = frozenset({".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs"})
_PY_SUFFIXES = frozenset({".py"})


def _lang_family(path: str) -> str:
    """Coarse language family for cross-language call-resolution guards."""
    suffix = Path(path).suffix.lower()
    if suffix in _JS_SUFFIXES:
        return "js"
    if suffix in _PY_SUFFIXES:
        return "py"
    if suffix == ".go":
        return "go"
    if suffix == ".rs":
        return "rs"
    return suffix or "other"


def _same_package_dir(path: str) -> str:
    """Parent directory used as same-package / sibling-module scope."""
    return str(Path(path).parent).replace("\\", "/")


def _method_name_hits(
    by_name: Dict[str, List[Tuple[str, str, str]]],
    name: str,
    *,
    caller_fam: str,
    path_filter: Optional[Set[str]] = None,
) -> List[Tuple[str, str, str]]:
    """Symbols whose qualname is ``*.{name}`` (methods), same language family."""
    suffix = f".{name}"
    out: List[Tuple[str, str, str]] = []
    for p, q, i in by_name.get(name, []):
        if _lang_family(p) != caller_fam:
            continue
        if "." not in q or not q.endswith(suffix):
            continue
        if path_filter is not None and p not in path_filter:
            continue
        out.append((p, q, i))
    return out


def module_suffix_index(py_files: List[str]) -> Dict[str, str]:
    """Map every dotted suffix of each module path to its file (pure).

    Ambiguous suffixes shared by two files are dropped. Packages (``__init__.py``)
    are indexed under their package dotted path.
    """
    index: Dict[str, str] = {}
    ambiguous: Set[str] = set()

    def _register(dotted: str, file: str) -> None:
        comps = [c for c in dotted.split(".") if c]
        for i in range(len(comps)):
            suffix = ".".join(comps[i:])
            if not suffix:
                continue
            if suffix in index and index[suffix] != file:
                ambiguous.add(suffix)
            else:
                index[suffix] = file

    for f in py_files:
        module_path = f[:-3] if f.endswith(".py") else f
        if module_path.endswith("/__init__"):
            _register(module_path[: -len("/__init__")].replace("/", "."), f)
        else:
            _register(module_path.replace("/", "."), f)
    for suffix in ambiguous:
        index.pop(suffix, None)
    return index


def resolve_python_module(module: str, index: Dict[str, str]) -> Optional[str]:
    """Resolve a Python import module string against ``module_suffix_index`` (pure)."""
    comps = [c for c in module.split(".") if c]
    if comps and comps[0] in getattr(sys, "stdlib_module_names", set()):
        return None
    while comps:
        candidate = ".".join(comps)
        if candidate in index:
            return index[candidate]
        comps = comps[:-1]
    return None


def ancestor_init_files(target: str, py_file_set: Set[str]) -> List[str]:
    """Ancestor package ``__init__.py`` files between ``target`` and repo root (pure)."""
    parts = target.replace("\\", "/").split("/")
    if not parts:
        return []
    if parts[-1] == "__init__.py":
        dir_parts = parts[:-2]
    else:
        dir_parts = parts[:-1]
    out: List[str] = []
    for i in range(len(dir_parts), 0, -1):
        init = "/".join(dir_parts[:i]) + "/__init__.py"
        if init in py_file_set and init != target:
            out.append(init)
    return out


def _is_js_path(path: str) -> bool:
    return Path(path).suffix.lower() in _JS_SUFFIXES


def resolve_import_edges(
    extractions: Dict[str, FileExtraction],
    files: List[str],
    *,
    root: Path,
    mapper: Optional[Any] = None,
) -> List[Tuple[str, str]]:
    """Resolve file→file import edges from extractions (no import re-extraction).

    Uses pure Python module→path helpers plus RepoMapper JS/Go/Rust resolvers
    against already-extracted ``imports`` / specs. Does not update the parse cache.
    """
    from devcouncil.indexing.repo_mapper import RepoMapper

    root = root.expanduser().resolve()
    mapper = mapper if mapper is not None else RepoMapper(root)
    file_set = set(files)
    edges: List[Tuple[str, str]] = []
    seen: Set[Tuple[str, str]] = set()

    # --- Python ---
    py_files = [f for f in files if f.endswith(".py")]
    if py_files:
        index = module_suffix_index(py_files)
        py_file_set = set(py_files)
        for rel in py_files:
            ext = extractions.get(rel)
            modules = list(ext.imports) if ext else []
            for module in modules:
                target = resolve_python_module(module, index)
                if not target:
                    continue
                for dest in [target, *ancestor_init_files(target, py_file_set)]:
                    if dest != rel and (rel, dest) not in seen:
                        seen.add((rel, dest))
                        edges.append((rel, dest))

    # --- JS/TS ---
    js_files = [f for f in files if _is_js_path(f)]
    if js_files:
        mapper._last_file_set = file_set
        for rel in js_files:
            ext = extractions.get(rel)
            specs = list(ext.imports) if ext else []
            for spec in specs:
                target = mapper._resolve_js_spec(rel, spec, file_set)
                if not target or target == rel:
                    continue
                to_add = [target]
                to_add.extend(mapper._follow_js_reexports(rel, target, file_set))
                for dest in to_add:
                    if dest != rel and (rel, dest) not in seen:
                        seen.add((rel, dest))
                        edges.append((rel, dest))

    # --- Go (specs from extractions + same-package co-membership) ---
    go_files = [f for f in files if f.endswith(".go")]
    if go_files:
        go_module = mapper._go_module_prefix(file_set)
        if go_module:
            pkg_files: Dict[str, List[str]] = defaultdict(list)
            for f in go_files:
                if f.endswith("_test.go"):
                    continue
                pkg_files[Path(f).parent.as_posix()].append(f)
            for members in pkg_files.values():
                if len(members) < 2:
                    continue
                for a in members:
                    for b in members:
                        if a != b and (a, b) not in seen:
                            seen.add((a, b))
                            edges.append((a, b))
            for rel in go_files:
                ext = extractions.get(rel)
                specs = list(ext.imports) if ext else []
                for spec in specs:
                    if spec != go_module and not spec.startswith(go_module + "/"):
                        continue
                    rel_pkg = spec[len(go_module) :].lstrip("/")
                    target_dir = rel_pkg if rel_pkg else "."
                    for target in sorted(pkg_files.get(target_dir, ())):
                        if target != rel and (rel, target) not in seen:
                            seen.add((rel, target))
                            edges.append((rel, target))

    # --- Rust: structured refs not in extractions; fall back to mapper ---
    try:
        for a, b in mapper._rust_import_edges(files, file_set):
            if (a, b) not in seen:
                seen.add((a, b))
                edges.append((a, b))
    except Exception:
        logger.debug("Rust import-edge resolution failed", exc_info=True)

    return edges


def _area_for(path: str) -> str:
    parts = path.replace("\\", "/").split("/")
    if not parts:
        return "root"
    if parts[0] in {"src", "lib", "app", "pkg"} and len(parts) >= 2:
        return "/".join(parts[:2]) if len(parts) > 2 else parts[0]
    return parts[0] if parts else "root"


def build_file_and_symbol_nodes(
    extractions: Dict[str, FileExtraction],
    *,
    area_fn=None,
) -> Tuple[List[GraphNode], Dict[str, str]]:
    """Build file + symbol nodes. Returns (nodes, qualname_index path::qual -> id)."""
    area_fn = area_fn or _area_for
    nodes: List[GraphNode] = []
    index: Dict[str, str] = {}  # path::qualname -> id
    name_index: Dict[str, List[str]] = defaultdict(list)  # bare name -> ids

    for path, ext in sorted(extractions.items()):
        fid = file_node_id(path)
        area = area_fn(path)
        nodes.append(
            GraphNode(
                id=fid,
                kind=NodeKind.FILE,
                path=path,
                name=Path(path).name,
                area=area,
                language=ext.language,
            )
        )
        for sym in ext.symbols:
            sid = symbol_node_id(path, sym.qualname)
            kind = {
                "function": NodeKind.FUNCTION,
                "class": NodeKind.CLASS,
                "method": NodeKind.METHOD,
                "interface": NodeKind.INTERFACE,
                "type": NodeKind.TYPE,
                "struct": NodeKind.STRUCT,
                "enum": NodeKind.ENUM,
                "trait": NodeKind.TRAIT,
                "property": NodeKind.PROPERTY,
                "variable": NodeKind.VARIABLE,
                "rationale": NodeKind.RATIONALE,
            }.get(sym.kind, NodeKind.FUNCTION)
            extras = {
                "qualname": sym.qualname,
                "bases": list(sym.bases),
                "implements": list(getattr(sym, "implements", []) or []),
                "decorators": list(sym.decorators),
            }
            if sym.kind == "rationale" and sym.bases:
                extras["enclosing"] = sym.bases[0]
            nodes.append(
                GraphNode(
                    id=sid,
                    kind=kind,
                    path=path,
                    name=sym.name,
                    line=sym.line,
                    end_line=sym.end_line,
                    area=area,
                    language=ext.language,
                    exported=bool(sym.exported),
                    extras=extras,
                )
            )
            index[f"{path}::{sym.qualname}"] = sid
            if sym.kind != "rationale":
                name_index[sym.name].append(sid)
                if "." in sym.qualname:
                    name_index[sym.qualname].append(sid)

    return nodes, dict(index)


def contains_and_defines_edges(
    extractions: Dict[str, FileExtraction],
) -> List[GraphEdge]:
    edges: List[GraphEdge] = []
    for path, ext in extractions.items():
        fid = file_node_id(path)
        for sym in ext.symbols:
            sid = symbol_node_id(path, sym.qualname)
            # Single structural edge per symbol ("contains"). The historical
            # duplicate "defines" edge doubled edge counts and skewed degree /
            # blast-radius metrics for zero information gain.
            edges.append(
                GraphEdge(
                    source=fid,
                    target=sid,
                    kind="contains",
                    confidence=Confidence.EXTRACTED,
                    reason="ast definition",
                )
            )
            # Class contains method
            if sym.kind == "method" and "." in sym.qualname:
                cls = sym.qualname.rsplit(".", 1)[0]
                cid = symbol_node_id(path, cls)
                edges.append(
                    GraphEdge(
                        source=cid,
                        target=sid,
                        kind="contains",
                        confidence=Confidence.EXTRACTED,
                        reason="method of class",
                    )
                )
            if sym.kind == "rationale":
                target = file_node_id(path)
                if sym.bases:
                    target = symbol_node_id(path, sym.bases[0])
                edges.append(
                    GraphEdge(
                        source=sid,
                        target=target,
                        kind="documents",
                        confidence=Confidence.EXTRACTED,
                        reason="rationale comment",
                    )
                )
    return edges


def inherit_edges(extractions: Dict[str, FileExtraction], symbol_index: Dict[str, str]) -> List[GraphEdge]:
    """Resolve bases to inherits, implements lists, and matching methods to overrides."""
    by_name: Dict[str, List[str]] = defaultdict(list)
    for key, sid in symbol_index.items():
        qual = key.split("::", 1)[-1]
        if qual.startswith("$rationale:"):
            continue
        by_name[qual.split(".")[-1]].append(sid)
        by_name[qual].append(sid)

    methods_by_class: Dict[str, Dict[str, str]] = defaultdict(dict)
    for key, sid in symbol_index.items():
        path, _, qual = key.partition("::")
        if "." in qual and not qual.startswith("$"):
            cls, _, meth = qual.partition(".")
            methods_by_class[f"{path}::{cls}"][meth] = sid

    def _resolve_base(path: str, base: str) -> tuple[Optional[str], Confidence, str, List[str]]:
        base_name = base.split(".")[-1].split("<", 1)[0]
        candidates = by_name.get(base_name, [])
        same = [c for c in candidates if c.startswith(f"{path}::")]
        if len(same) == 1:
            return same[0], Confidence.EXTRACTED, "same-file base", []
        if len(candidates) == 1:
            return candidates[0], Confidence.INFERRED, "unique global base", []
        if len(candidates) > 1:
            return (
                candidates[0],
                Confidence.AMBIGUOUS,
                f"ambiguous base ({len(candidates)} candidates)",
                list(candidates),
            )
        return None, Confidence.AMBIGUOUS, "unresolved", []

    edges: List[GraphEdge] = []
    type_kinds = {"class", "interface", "struct", "enum", "trait", "type"}
    for path, ext in extractions.items():
        for sym in ext.symbols:
            if sym.kind not in type_kinds:
                continue
            src_id = symbol_node_id(path, sym.qualname)
            for base in sym.bases:
                target, conf, reason, cands = _resolve_base(path, base)
                if not target:
                    continue
                edges.append(
                    GraphEdge(
                        source=src_id,
                        target=target,
                        kind="inherits",
                        confidence=conf,
                        reason=reason,
                        extras={"candidates": cands} if cands else {},
                    )
                )
            for iface in getattr(sym, "implements", []) or []:
                target, conf, reason, cands = _resolve_base(path, iface)
                if not target:
                    continue
                edges.append(
                    GraphEdge(
                        source=src_id,
                        target=target,
                        kind="implements",
                        confidence=conf,
                        reason=reason.replace("base", "interface"),
                        extras={"candidates": cands} if cands else {},
                    )
                )

        for sym in ext.symbols:
            if sym.kind != "method" or "." not in sym.qualname:
                continue
            cls, _, meth = sym.qualname.partition(".")
            src_id = symbol_node_id(path, sym.qualname)
            owner = next((s for s in ext.symbols if s.qualname == cls), None)
            parent_names: List[str] = []
            if owner is not None:
                parent_names.extend(owner.bases)
                parent_names.extend(getattr(owner, "implements", []) or [])
            parent_names.extend(sym.bases)
            seen_targets: Set[str] = set()
            for parent in parent_names:
                parent_name = parent.split(".")[-1].split("<", 1)[0]
                for pid in by_name.get(parent_name, []):
                    parent_path = pid.split("::", 1)[0]
                    parent_qual = pid.split("::", 1)[-1]
                    cand = methods_by_class.get(f"{parent_path}::{parent_qual}", {}).get(meth)
                    if cand and cand != src_id and cand not in seen_targets:
                        seen_targets.add(cand)
                        edges.append(
                            GraphEdge(
                                source=src_id,
                                target=cand,
                                kind="overrides",
                                confidence=Confidence.INFERRED,
                                reason="matching method on base",
                            )
                        )
    return edges


def decorator_edges(
    extractions: Dict[str, FileExtraction],
    symbol_index: Dict[str, str],
) -> List[GraphEdge]:
    """``decorator -> decorated symbol`` edges (GitNexus-style DECORATES).

    Resolved same-file first, then unique-global by bare name. Purely additive:
    liveness of decorator functions is handled via the bare-decorator calls
    emitted at extraction time.
    """
    by_name: Dict[str, List[str]] = defaultdict(list)
    for key, sid in symbol_index.items():
        qual = key.split("::", 1)[-1]
        if qual.startswith("$rationale:"):
            continue
        if "." not in qual:
            by_name[qual].append(sid)

    edges: List[GraphEdge] = []
    seen: Set[Tuple[str, str]] = set()
    for path, ext in extractions.items():
        for sym in ext.symbols:
            if not sym.decorators:
                continue
            target = symbol_node_id(path, sym.qualname)
            for dec in sym.decorators:
                bare = dec.split("(", 1)[0].split(".")[-1].strip()
                if not bare:
                    continue
                src = symbol_index.get(f"{path}::{bare}")
                conf = Confidence.EXTRACTED
                reason = "same-file decorator"
                if not src:
                    cands = by_name.get(bare, [])
                    if len(cands) != 1:
                        continue
                    src = cands[0]
                    conf = Confidence.INFERRED
                    reason = "unique global decorator"
                if src == target or (src, target) in seen:
                    continue
                seen.add((src, target))
                edges.append(
                    GraphEdge(
                        source=src,
                        target=target,
                        kind="decorates",
                        confidence=conf,
                        reason=reason,
                    )
                )
    return edges


def import_graph_edges(file_edges: List[Tuple[str, str]]) -> List[GraphEdge]:
    return [
        GraphEdge(
            source=file_node_id(a),
            target=file_node_id(b),
            kind="imports",
            confidence=Confidence.EXTRACTED,
            reason="import resolution",
        )
        for a, b in file_edges
        if a != b
    ]


def named_import_edges(
    extractions: Dict[str, FileExtraction],
    symbol_index: Dict[str, str],
    file_edges: List[Tuple[str, str]],
) -> List[GraphEdge]:
    """``from mod import Name`` → imports edge to the resolved symbol (keeps it live)."""
    imports_of: Dict[str, Set[str]] = defaultdict(set)
    for a, b in file_edges:
        imports_of[a].add(b)

    # path::bare_name -> symbol id (top-level only)
    by_path_name: Dict[str, str] = {}
    for key, sid in symbol_index.items():
        path, _, qual = key.partition("::")
        if "." not in qual:
            by_path_name[f"{path}::{qual}"] = sid

    edges: List[GraphEdge] = []
    seen: Set[Tuple[str, str]] = set()
    for path, ext in extractions.items():
        src = file_node_id(path)
        imported_files = imports_of.get(path, set())
        for detail in ext.import_details:
            for name in detail.names:
                bare = name.split(".")[-1]
                if not bare or bare == "*":
                    continue
                # Prefer a symbol in an imported file
                target: Optional[str] = None
                for fpath in imported_files:
                    cand = by_path_name.get(f"{fpath}::{bare}")
                    if cand:
                        target = cand
                        break
                if target is None:
                    # Fallback: unique global top-level with that name in imported files
                    matches = [
                        by_path_name[k]
                        for k in by_path_name
                        if k.endswith(f"::{bare}") and k.split("::", 1)[0] in imported_files
                    ]
                    if len(matches) == 1:
                        target = matches[0]
                if target and (src, target) not in seen:
                    seen.add((src, target))
                    edges.append(
                        GraphEdge(
                            source=src,
                            target=target,
                            kind="imports",
                            confidence=Confidence.EXTRACTED,
                            reason="named import",
                        )
                    )
    return edges


def resolve_calls(
    extractions: Dict[str, FileExtraction],
    symbol_index: Dict[str, str],
    file_edges: List[Tuple[str, str]],
    *,
    class_ids: Optional[Set[str]] = None,
) -> List[GraphEdge]:
    """Confidence ladder: same-file → import-resolved → unique-global → ambiguous.

    When ``class_ids`` is given, calls that target a class node are tagged
    ``extras["instantiates"] = True`` (constructor calls) so consumers can
    distinguish instantiation from plain function calls.
    """
    # name -> list of (path, qualname, id)
    by_name: Dict[str, List[Tuple[str, str, str]]] = defaultdict(list)
    for key, sid in symbol_index.items():
        path, _, qual = key.partition("::")
        by_name[qual.split(".")[-1]].append((path, qual, sid))
        if "." in qual:
            by_name[qual].append((path, qual, sid))

    # importer -> imported files
    imports_of: Dict[str, Set[str]] = defaultdict(set)
    for a, b in file_edges:
        imports_of[a].add(b)

    # Local alias maps from extractions (import as)
    alias_of: Dict[str, Dict[str, str]] = {}
    for path, ext in extractions.items():
        amap: Dict[str, str] = {}
        for detail in ext.import_details:
            amap.update(detail.alias_map)
        # Also map imported module last segment
        for mod in ext.imports:
            amap.setdefault(mod.split(".")[-1], mod)
        alias_of[path] = amap

    edges: List[GraphEdge] = []
    seen: Set[Tuple[str, str]] = set()

    for path, ext in extractions.items():
        # Caller context: prefer enclosing function/method by line
        enclosing: List[Tuple[int, int, str]] = []
        for sym in ext.symbols:
            enclosing.append((sym.line, sym.end_line, symbol_node_id(path, sym.qualname)))
        enclosing.sort()

        for call in ext.calls:
            caller_id = file_node_id(path)
            for start, end, sid in enclosing:
                if start <= call.line <= end:
                    caller_id = sid
                    # prefer innermost (last match)
            target_id: Optional[str] = None
            confidence = Confidence.AMBIGUOUS
            reason = "unresolved"
            ambig_candidates: List[str] = []

            # 0) Attribute call with inferable receiver class: Foo.bar / Foo().bar
            if target_id is None and call.receiver and call.name:
                recv = call.receiver.split(".")[-1]
                # Same-file Class.method
                same_method = symbol_index.get(f"{path}::{recv}.{call.name}")
                if same_method:
                    target_id = same_method
                    confidence = Confidence.EXTRACTED
                    reason = "attribute-call same-file class"
                else:
                    # Unique Class.method across imported / global (same language)
                    method_key = f"{recv}.{call.name}"
                    imported_files = imports_of.get(path, set())
                    caller_fam = _lang_family(path)
                    cands = [
                        (p, q, i)
                        for p, q, i in by_name.get(call.name, [])
                        if _lang_family(p) == caller_fam
                        and (
                            q == method_key
                            or (
                                q.endswith(f".{call.name}")
                                and q.rsplit(".", 1)[0] == recv
                            )
                        )
                    ]
                    if not cands:
                        cands = [
                            (p, q, i)
                            for p, q, i in by_name.get(method_key, [])
                            if _lang_family(p) == caller_fam
                        ]
                    in_imp = [(p, q, i) for p, q, i in cands if p in imported_files or p == path]
                    pool = in_imp or cands
                    if len(pool) == 1:
                        target_id = pool[0][2]
                        confidence = Confidence.INFERRED
                        reason = "attribute-call inferred class"
                    elif len(pool) > 1:
                        target_id = None
                        confidence = Confidence.AMBIGUOUS
                        reason = f"attribute-call ambiguous ({len(pool)} candidates)"
                        ambig_candidates = [i for _p, _q, i in pool]

            # 0b) Unique *.{name} method when receiver is a variable (not a type name).
            # Prefer same-file, then same-package / imported (same language family).
            if (
                target_id is None
                and not ambig_candidates
                and call.receiver
                and call.receiver not in ("self", "cls", "this")
                and call.name
            ):
                caller_fam = _lang_family(path)
                same_file = _method_name_hits(
                    by_name, call.name, caller_fam=caller_fam, path_filter={path}
                )
                if len(same_file) == 1:
                    target_id = same_file[0][2]
                    confidence = Confidence.INFERRED
                    reason = "unique same-file method"
                elif len(same_file) > 1:
                    confidence = Confidence.AMBIGUOUS
                    reason = f"ambiguous same-file method ({len(same_file)} candidates)"
                    ambig_candidates = [i for _p, _q, i in same_file]
                else:
                    pkg = _same_package_dir(path)
                    imported = imports_of.get(path, set())
                    scope = {
                        p
                        for p, q, _i in by_name.get(call.name, [])
                        if _lang_family(p) == caller_fam
                        and "." in q
                        and q.endswith(f".{call.name}")
                        and (_same_package_dir(p) == pkg or p in imported)
                    }
                    scoped = _method_name_hits(
                        by_name, call.name, caller_fam=caller_fam, path_filter=scope
                    )
                    if len(scoped) == 1:
                        target_id = scoped[0][2]
                        confidence = Confidence.INFERRED
                        reason = "unique package/imported method"
                    elif len(scoped) > 1:
                        confidence = Confidence.AMBIGUOUS
                        reason = (
                            f"ambiguous package/imported method ({len(scoped)} candidates)"
                        )
                        ambig_candidates = [i for _p, _q, i in scoped]

            # 1) same-file exact qualname / name (``this`` covers TS/JS methods)
            if call.receiver in ("self", "cls", "this") and call.name:
                # method on same class — find enclosing class
                for start, end, sid in enclosing:
                    if start <= call.line <= end and "." in sid:
                        cls = sid.split("::", 1)[1].rsplit(".", 1)[0]
                        cand = symbol_index.get(f"{path}::{cls}.{call.name}")
                        if cand:
                            target_id = cand
                            confidence = Confidence.EXTRACTED
                            reason = "same-file self/cls call"
                            break
            if target_id is None and call.qualname_hint:
                same = symbol_index.get(f"{path}::{call.qualname_hint}")
                if same:
                    target_id = same
                    confidence = Confidence.EXTRACTED
                    reason = "same-file qualname"
                elif not call.receiver or call.receiver in ("self", "cls", "this"):
                    # Bare-name fallback only when there is no module/class receiver —
                    # otherwise ``prompt_handlers.get_prompt`` wrongly binds to a
                    # same-file ``get_prompt``.
                    same = symbol_index.get(f"{path}::{call.name}")
                    if same:
                        target_id = same
                        confidence = Confidence.EXTRACTED
                        reason = "same-file name"

            # 2) import-resolved: receiver is an imported alias pointing at a module/file
            if target_id is None and call.receiver:
                amap = alias_of.get(path, {})
                imported_files = imports_of.get(path, set())
                # Alias → defining module file (``import prompts as prompt_handlers``)
                alias_target = amap.get(call.receiver, "")
                alias_files: List[str] = []
                if alias_target:
                    dotted = alias_target.replace(".", "/")
                    for f in imported_files:
                        norm = f[:-3] if f.endswith(".py") else f
                        if norm.replace("\\", "/").endswith(dotted) or norm.replace(
                            "/", "."
                        ).endswith(alias_target):
                            alias_files.append(f)
                        elif Path(f).stem == alias_target.split(".")[-1]:
                            alias_files.append(f)
                candidates = [
                    (p, q, i)
                    for p, q, i in by_name.get(call.name, [])
                    if p in (alias_files or imported_files)
                ]
                if alias_files:
                    narrowed = [
                        (p, q, i) for p, q, i in by_name.get(call.name, []) if p in alias_files
                    ]
                    if len(narrowed) == 1:
                        target_id = narrowed[0][2]
                        confidence = Confidence.EXTRACTED
                        reason = "import-alias module attr"
                    elif len(narrowed) > 1:
                        # Prefer top-level function over methods
                        top = [(p, q, i) for p, q, i in narrowed if "." not in q]
                        pool = top or narrowed
                        if len(pool) == 1:
                            target_id = pool[0][2]
                            confidence = Confidence.INFERRED
                            reason = "import-alias module attr"
                        else:
                            target_id = None
                            confidence = Confidence.AMBIGUOUS
                            reason = f"import-alias ambiguous ({len(pool)} candidates)"
                            ambig_candidates = [i for _p, _q, i in pool]
                elif len(candidates) == 1:
                    target_id = candidates[0][2]
                    confidence = Confidence.INFERRED
                    reason = "import-resolved"
                elif call.receiver in amap or call.receiver:
                    # Prefer imported file whose stem matches receiver
                    recv_files = [
                        f
                        for f in imported_files
                        if Path(f).stem == call.receiver
                        or f.replace("/", ".").endswith(call.receiver)
                    ]
                    narrowed = [
                        (p, q, i)
                        for p, q, i in by_name.get(call.name, [])
                        if p in recv_files or (not recv_files and p in imported_files)
                    ]
                    if len(narrowed) == 1:
                        target_id = narrowed[0][2]
                        confidence = Confidence.INFERRED
                        reason = "import-resolved receiver"

            # 3) unique global (same language family only — never JS→Python etc.)
            if target_id is None:
                # Skip stdlib-looking bare names
                if call.name in getattr(sys, "stdlib_module_names", set()):
                    continue
                caller_fam = _lang_family(path)
                cands = [
                    (p, q, i)
                    for p, q, i in by_name.get(call.name, [])
                    if _lang_family(p) == caller_fam
                ]
                # Prefer non-method top-level when bare call
                if not call.receiver:
                    top = [(p, q, i) for p, q, i in cands if "." not in q]
                    if len(top) == 1:
                        target_id = top[0][2]
                        confidence = Confidence.INFERRED
                        reason = "unique global"
                    elif len(cands) == 1:
                        target_id = cands[0][2]
                        confidence = Confidence.INFERRED
                        reason = "unique global"
                    elif len(cands) > 1:
                        target_id = None
                        confidence = Confidence.AMBIGUOUS
                        reason = f"ambiguous ({len(cands)} candidates)"
                        ambig_candidates = [i for _p, _q, i in cands]
                else:
                    if len(cands) == 1:
                        target_id = cands[0][2]
                        confidence = Confidence.INFERRED
                        reason = "unique global"
                    elif len(cands) > 1:
                        target_id = None
                        confidence = Confidence.AMBIGUOUS
                        reason = f"ambiguous ({len(cands)} candidates)"
                        ambig_candidates = [i for _p, _q, i in cands]

            # Ambiguous name → fan-out edges to every candidate so liveness does
            # not drop real callees when the resolver cannot pick a unique target
            # (e.g. ``result.to_dict()`` among many ``to_dict`` methods).
            if target_id is None and ambig_candidates:
                extras_base: Dict[str, Any] = {"candidates": list(ambig_candidates)}
                for cand_id in ambig_candidates:
                    if (caller_id, cand_id) in seen:
                        continue
                    seen.add((caller_id, cand_id))
                    extras = dict(extras_base)
                    if class_ids and cand_id in class_ids:
                        extras["instantiates"] = True
                    edges.append(
                        GraphEdge(
                            source=caller_id,
                            target=cand_id,
                            kind="calls",
                            confidence=Confidence.AMBIGUOUS,
                            reason=reason or f"ambiguous ({len(ambig_candidates)} candidates)",
                            extras=extras,
                        )
                    )
                continue

            if target_id and (caller_id, target_id) not in seen:
                seen.add((caller_id, target_id))
                extras = {}
                if confidence == Confidence.AMBIGUOUS and ambig_candidates:
                    extras["candidates"] = list(ambig_candidates)
                if class_ids and target_id in class_ids:
                    extras["instantiates"] = True
                edges.append(
                    GraphEdge(
                        source=caller_id,
                        target=target_id,
                        kind="calls",
                        confidence=confidence,
                        reason=reason,
                        extras=extras,
                    )
                )
    return edges
