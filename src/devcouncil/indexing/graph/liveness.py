"""Graph-based file + symbol liveness / dead-code detection.

devcouncil: allow-unwired — package-private; reached only via graph.build / CLI.
"""

from __future__ import annotations

import logging
import re
from collections import defaultdict
from dataclasses import asdict
from pathlib import Path
from typing import Dict, List, Optional, Set, Tuple, cast

from devcouncil.indexing.graph.extract_python import (
    ExtractedCall,
    ExtractedImport,
    ExtractedSymbol,
    FileExtraction,
)
from devcouncil.indexing.graph.schema import (
    Confidence,
    DeadCodeEntry,
    GraphEdge,
    GraphNode,
    NodeKind,
)

logger = logging.getLogger(__name__)

_CONFIDENCE_RANK = {
    Confidence.AMBIGUOUS: 0,
    Confidence.INFERRED: 1,
    Confidence.EXTRACTED: 2,
    "ambiguous": 0,
    "inferred": 1,
    "extracted": 2,
}


def file_liveness(
    root: Path,
    files: List[str],
    file_edges: List[Tuple[str, str]],
    *,
    cap: Optional[int] = None,
) -> Tuple[List[str], List[str], List[str], bool]:
    """Return (entry_roots, unwired, unreachable, unreachable_unreliable).

    ``cap`` defaults to uncapped (``None`` / ``<= 0``). Caps belong on
    ``repo_map.json`` serialization, not the in-memory / ``code_graph.json`` lists.

    When production entry roots are empty, unreachable BFS would flood every
    non-root file. Fail soft: ``unreachable=[]`` and
    ``unreachable_unreliable=True`` (still compute unwired).
    """
    from devcouncil.indexing.wiring import (
        build_dynamic_import_index,
        entry_roots,
        has_allow_unwired,
        is_liveness_code_file,
        is_test_path,
        reference_cleared,
        structural_exemptions,
    )

    limit = None if (cap is None or cap <= 0) else cap
    roots = entry_roots(root, files)
    prod_roots = entry_roots(root, files, production_only=True)
    root_set = set(roots)
    prod_root_set = set(prod_roots)
    dyn_index = build_dynamic_import_index(root, files)
    unreachable_unreliable = not bool(prod_roots)

    inbound: Dict[str, Set[str]] = defaultdict(set)
    outbound: Dict[str, Set[str]] = defaultdict(set)
    for importer, imported in file_edges:
        inbound[imported].add(importer)
        outbound[importer].add(imported)

    unwired: List[str] = []
    for f in sorted(files):
        if not is_liveness_code_file(f):
            continue
        if f in root_set or structural_exemptions(f):
            continue
        if has_allow_unwired(root, f):
            continue
        non_test_importers = {i for i in inbound.get(f, ()) if not is_test_path(i)}
        if non_test_importers:
            continue
        if reference_cleared(
            root, f, skip_files=set(), git_files=files, dynamic_index=dyn_index
        ):
            continue
        unwired.append(f)
        if limit is not None and len(unwired) >= limit:
            break

    unreachable: List[str] = []
    if not unreachable_unreliable:
        reachable: Set[str] = set()
        queue = list(prod_roots)
        seen_q: Set[str] = set(queue)
        while queue:
            cur = queue.pop()
            reachable.add(cur)
            for nxt in outbound.get(cur, ()):
                if nxt not in seen_q:
                    seen_q.add(nxt)
                    queue.append(nxt)

        for f in sorted(files):
            if not is_liveness_code_file(f):
                continue
            if f in prod_root_set or f in root_set or structural_exemptions(f):
                continue
            if f in reachable:
                continue
            unreachable.append(f)
            if limit is not None and len(unreachable) >= limit:
                break

    return prod_roots, unwired, unreachable, unreachable_unreliable


def _token_scan_dead(
    root: Path,
    files: List[str],
    *,
    cap: int = 0,
    lsp_refs: bool = False,
    mapper: Optional[object] = None,
) -> Tuple[List[str], List[str], Set[str]]:
    """Legacy token-scan dead symbols via RepoMapper (format path:line name).

    Returns (dead_list, symbol_index, dead_keys as path::name).
    """
    from devcouncil.indexing.repo_mapper import RepoMapper

    m = cast(RepoMapper, mapper) if mapper is not None else RepoMapper(root)
    dead, index = m._dead_symbol_candidates(
        files, cap=cap if cap > 0 else 0, with_index=True, lsp_refs=lsp_refs
    )
    keys: Set[str] = set()
    for entry in dead:
        # "path:line name"
        loc, _, name = entry.partition(" ")
        path, _, _line = loc.rpartition(":")
        if path and name:
            keys.add(f"{path}::{name}")
    return dead, index, keys


def _reference_index(
    extractions: Dict[str, FileExtraction],
) -> Dict[str, Set[str]]:
    """name -> set of files that reference it outside a call site.

    Covers callbacks passed by name (``register(handler)``), registry/dict
    dispatch, and attribute access on ``@property``-style members — including
    references from tests (so property-only APIs are not inferred-dead).
    """
    index: Dict[str, Set[str]] = defaultdict(set)
    for path, ext in extractions.items():
        for name in getattr(ext, "references", None) or ():
            index[name].add(path)
    return index


def build_liveness_shard(root: Path, extraction: FileExtraction) -> dict[str, object]:
    """Compact persisted reference shard used by one-file liveness updates."""
    from devcouncil.indexing.wiring import strip_js_comments, strip_py_comments, strip_string_literals

    try:
        source = (root / extraction.path).read_text(encoding="utf-8", errors="replace")
    except OSError:
        source = ""
    cleaned = (
        strip_py_comments(source)
        if Path(extraction.path).suffix.lower() == ".py"
        else strip_js_comments(source)
    )
    cleaned = strip_string_literals(cleaned)
    token_lines: dict[str, list[int]] = defaultdict(list)
    for lineno, line in enumerate(cleaned.splitlines(), 1):
        for token in re.findall(r"\b[A-Za-z_][A-Za-z0-9_]*\b", line):
            if len(token) >= 2:
                token_lines[token].append(lineno)
    return {
        "extraction": asdict(extraction),
        "token_lines": dict(token_lines),
        "allow_unwired": "devcouncil: allow-unwired" in source,
    }


def extraction_from_liveness_shard(shard: dict[str, object]) -> FileExtraction:
    extraction = shard.get("extraction")
    raw = dict(extraction) if isinstance(extraction, dict) else {}
    return FileExtraction(
        path=str(raw.get("path") or ""),
        language=str(raw.get("language") or ""),
        imports=list(raw.get("imports") or []),
        import_details=[ExtractedImport(**row) for row in raw.get("import_details") or []],
        symbols=[ExtractedSymbol(**row) for row in raw.get("symbols") or []],
        calls=[ExtractedCall(**row) for row in raw.get("calls") or []],
        all_exports=list(raw.get("all_exports") or []),
        reexports=list(raw.get("reexports") or []),
        references=list(raw.get("references") or []),
    )


def token_dead_from_shards(
    nodes: List[GraphNode],
    shards: dict[str, dict[str, object]],
) -> Tuple[List[str], List[str], Set[str]]:
    """Compute token agreement from persisted per-file token occurrence shards."""
    token_files: Dict[str, Set[str]] = defaultdict(set)
    token_lines: Dict[str, Dict[str, Set[int]]] = defaultdict(lambda: defaultdict(set))
    for path, shard in shards.items():
        raw_token_lines = shard.get("token_lines")
        rows = raw_token_lines if isinstance(raw_token_lines, dict) else {}
        for name, lines in rows.items():
            token_files[str(name)].add(path)
            if isinstance(lines, list):
                token_lines[str(name)][path].update(int(line) for line in lines)
    definitions = [
        node for node in nodes
        if node.kind in {NodeKind.FUNCTION, NodeKind.CLASS}
        and "." not in str(node.extras.get("qualname") or node.name)
    ]
    dead: List[str] = []
    keys: Set[str] = set()
    for node in definitions:
        refs = token_files.get(node.name, set())
        same_lines = token_lines.get(node.name, {}).get(node.path, set())
        if refs - {node.path} or any(
            line < node.line or line > node.end_line for line in same_lines
        ):
            continue
        dead.append(f"{node.path}:{node.line} {node.name}")
        keys.add(f"{node.path}::{node.name}")
    return dead, sorted(f"{node.path}::{node.name}" for node in definitions), keys


def file_liveness_from_shards(
    files: List[str],
    file_edges: List[Tuple[str, str]],
    shards: dict[str, dict[str, object]],
    *,
    entry_roots: List[str],
) -> Tuple[List[str], List[str], List[str], bool]:
    """Recompute file reachability from persisted adjacency without source scans."""
    from devcouncil.indexing.wiring import is_liveness_code_file, is_test_path, structural_exemptions

    roots = [path for path in entry_roots if path in set(files)]
    unreliable = not bool(roots)
    root_set = set(roots)
    inbound: Dict[str, Set[str]] = defaultdict(set)
    outbound: Dict[str, Set[str]] = defaultdict(set)
    for importer, imported in file_edges:
        inbound[imported].add(importer)
        outbound[importer].add(imported)
    unwired = [
        path for path in sorted(files)
        if is_liveness_code_file(path)
        and path not in root_set
        and not structural_exemptions(path)
        and not bool(shards.get(path, {}).get("allow_unwired"))
        and not {item for item in inbound.get(path, ()) if not is_test_path(item)}
    ]
    reachable: Set[str] = set()
    queue = list(roots)
    while queue:
        path = queue.pop()
        if path in reachable:
            continue
        reachable.add(path)
        queue.extend(outbound.get(path, ()))
    unreachable = [] if unreliable else [
        path for path in sorted(files)
        if is_liveness_code_file(path)
        and path not in root_set
        and not structural_exemptions(path)
        and path not in reachable
    ]
    return roots, unwired, unreachable, unreliable


def _resolve_reexport_targets(
    extractions: Dict[str, FileExtraction],
    file_edges: List[Tuple[str, str]],
) -> Set[Tuple[str, str]]:
    """Return ``(defining_path, name)`` pairs protected by barrel / ``__all__`` re-exports."""
    imports_of: Dict[str, Set[str]] = defaultdict(set)
    for a, b in file_edges:
        imports_of[a].add(b)

    # Top-level symbol names per file
    names_in: Dict[str, Set[str]] = defaultdict(set)
    for path, ext in extractions.items():
        for sym in ext.symbols:
            if "." not in sym.qualname and sym.kind != "rationale":
                names_in[path].add(sym.name)

    protected: Set[Tuple[str, str]] = set()
    for path, ext in extractions.items():
        for name in ext.all_exports:
            protected.add((path, name))

        reexport_names = set(ext.reexports)
        if not reexport_names:
            continue
        imported_files = imports_of.get(path, set())
        for detail in ext.import_details:
            for name in detail.names:
                bare = name.split(".")[-1]
                if not bare or bare == "*":
                    continue
                # Match either the imported name or local alias if present in reexports
                locals_for = [
                    loc for loc, remote in detail.alias_map.items() if remote.endswith(bare) or remote == name
                ]
                if bare not in reexport_names and not (set(locals_for) & reexport_names):
                    # Also accept when reexport list uses the imported bare name
                    if name not in reexport_names:
                        continue
                for fpath in imported_files:
                    if bare in names_in.get(fpath, ()):
                        protected.add((fpath, bare))
                        break
                else:
                    # Fallback: unique defining file among imports
                    hits = [f for f in imported_files if bare in names_in.get(f, ())]
                    if len(hits) == 1:
                        protected.add((hits[0], bare))
    return protected


# Framework targets are live only through a reachable registration owner.
_FRAMEWORK_LIVENESS_EDGE_KINDS = frozenset({
    "registers",
    "routes_to",
    "listens",
    "provides",
    "reflects_to",
})


def _live_seeds(
    root: Path,
    files: List[str],
    nodes: List[GraphNode],
    edges: List[GraphEdge],
    extractions: Dict[str, FileExtraction],
    protected: Set[Tuple[str, str]],
    file_edges: Optional[List[Tuple[str, str]]] = None,
) -> Set[str]:
    """Entry / export / wiring / test-referenced symbols seed the live fixed-point."""
    from devcouncil.indexing.wiring import (
        entry_point_symbols,
        is_test_path,
        is_wiring_decorated,
    )

    live: Set[str] = set()
    entry_syms = entry_point_symbols(root, files)
    node_by_id = {n.id: n for n in nodes}

    imports_of: Dict[str, Set[str]] = defaultdict(set)
    if file_edges:
        for a, b in file_edges:
            imports_of[a].add(b)

    for node in nodes:
        if node.kind not in {
            NodeKind.FUNCTION,
            NodeKind.CLASS,
            NodeKind.METHOD,
            NodeKind.INTERFACE,
            NodeKind.TYPE,
            NodeKind.STRUCT,
            NodeKind.ENUM,
            NodeKind.TRAIT,
        }:
            continue
        key = f"{node.path}::{node.name}"
        qual = (node.extras or {}).get("qualname") or node.name
        if key in entry_syms or f"{node.path}::{qual}" in entry_syms:
            live.add(node.id)
            continue
        if (node.path, node.name) in protected:
            live.add(node.id)
            continue
        # JS/TS explicit export surface (not Python's default-public exported flag)
        lang = (node.language or "").lower()
        if node.exported and lang in {"javascript", "typescript", "js", "ts", "tsx", "jsx"}:
            live.add(node.id)
            continue
        # Python: only __all__/reexport protection above — do not seed all public names
        if path_is_init_all_member(extractions, node):
            live.add(node.id)
            continue
        decs = list((node.extras or {}).get("decorators") or [])
        if is_wiring_decorated(decs):
            live.add(node.id)
            continue
    # Test-referenced: inbound call/named-import from a test file seeds live
    for e in edges:
        if e.kind not in {"calls", "imports"}:
            continue
        src_path = e.source.split("::", 1)[0]
        if not is_test_path(src_path):
            continue
        if e.target in node_by_id:
            live.add(e.target)

    # Same-file + imported-module name references (``pool.submit(self._run_one)``,
    # ``card.blocks_completion`` property access, registry dispatch). Includes
    # private methods so callback callees stay in the live fixed-point.
    from devcouncil.indexing.graph.schema import symbol_node_id

    by_name: Dict[str, List[GraphNode]] = defaultdict(list)
    for node in nodes:
        if node.kind in {
            NodeKind.FUNCTION,
            NodeKind.CLASS,
            NodeKind.METHOD,
            NodeKind.INTERFACE,
            NodeKind.TYPE,
            NodeKind.STRUCT,
            NodeKind.ENUM,
            NodeKind.TRAIT,
        }:
            by_name[node.name].append(node)

    for path, ext in extractions.items():
        ref_names = set(getattr(ext, "references", None) or ())
        if not ref_names:
            continue
        imported = set(imports_of.get(path, ())) | {path}
        for name in ref_names:
            hits = [n for n in by_name.get(name, ()) if n.path in imported]
            if len(hits) == 1:
                live.add(hits[0].id)
                continue
            if len(hits) > 1:
                same = [n for n in hits if n.path == path]
                if len(same) == 1:
                    live.add(same[0].id)
                    continue
            # Same-file symbol by qualname even when name is globally ambiguous
            for sym in ext.symbols:
                if sym.name == name:
                    live.add(symbol_node_id(path, sym.qualname))

    return live


def path_is_init_all_member(
    extractions: Dict[str, FileExtraction],
    node: GraphNode,
) -> bool:
    """True when ``node.name`` is listed in the defining file's ``__all__``."""
    ext = extractions.get(node.path)
    if ext is None:
        return False
    return node.name in set(ext.all_exports)


def symbol_reachability_dead(
    root: Path,
    files: List[str],
    nodes: List[GraphNode],
    edges: List[GraphEdge],
    extractions: Dict[str, FileExtraction],
    entry_roots: List[str],
    *,
    token_dead_keys: Optional[Set[str]] = None,
    file_edges: Optional[List[Tuple[str, str]]] = None,
    unreachable: Optional[List[str]] = None,
    dynamic_index: Optional[dict[str, Set[str]]] = None,
) -> List[DeadCodeEntry]:
    """Symbol-level dead code with fixed-point live propagation and confidence tiers.

    A symbol is live only if it is a seed (entry / exported surface / wiring /
    test-referenced / getattr) or has an inbound call/named-import from a live
    source. Cascade members whose only callers are dead get
    ``reason="only callers are dead"`` at ``inferred`` confidence.
    """
    from devcouncil.indexing.wiring import (
        GETATTR_INDEX_PREFIX,
        build_dynamic_import_index,
        has_allow_unwired,
        is_dunder_symbol,
        is_private_symbol,
        is_test_path,
        is_wiring_decorated,
        structural_exemptions,
    )

    # Reconstruct file edges from graph imports when not provided
    if file_edges is None:
        file_edges = [
            (e.source, e.target)
            for e in edges
            if e.kind == "imports" and "::" not in e.source and "::" not in e.target
        ]

    protected = _resolve_reexport_targets(extractions, file_edges)
    ref_index = _reference_index(extractions)
    dyn_index = (
        build_dynamic_import_index(root, files)
        if dynamic_index is None
        else dynamic_index
    )
    getattr_names = {
        k[len(GETATTR_INDEX_PREFIX) :]
        for k in dyn_index
        if k.startswith(GETATTR_INDEX_PREFIX)
    }

    imported_by: Dict[str, Set[str]] = defaultdict(set)
    for a, b in file_edges:
        imported_by[b].add(a)

    # Inbound call / named-import sources per symbol (liveness-relevant edges)
    inbound_live_edges: Dict[str, List[GraphEdge]] = defaultdict(list)
    overrides_of: Dict[str, List[str]] = defaultdict(list)  # child -> parent method ids
    for e in edges:
        if e.kind in {"calls", "imports"} | _FRAMEWORK_LIVENESS_EDGE_KINDS:
            if e.kind in _FRAMEWORK_LIVENESS_EDGE_KINDS:
                confidence = (
                    e.confidence.value
                    if hasattr(e.confidence, "value")
                    else str(e.confidence)
                )
                if confidence == Confidence.AMBIGUOUS.value:
                    continue
            # Ignore file→file imports for symbol liveness; named imports target symbols
            if e.kind == "imports" and "::" not in e.target:
                continue
            # Ignore contains/defines-style noise: already filtered by kind
            inbound_live_edges[e.target].append(e)
        elif e.kind == "overrides":
            overrides_of[e.source].append(e.target)

    live = _live_seeds(
        root, files, nodes, edges, extractions, protected, file_edges=file_edges
    )

    # getattr(x, "name") in non-test files seeds the named symbol live
    _seed_kinds = {
        NodeKind.FUNCTION,
        NodeKind.CLASS,
        NodeKind.METHOD,
        NodeKind.INTERFACE,
        NodeKind.TYPE,
        NodeKind.STRUCT,
        NodeKind.ENUM,
        NodeKind.TRAIT,
    }
    for node in nodes:
        if node.kind in _seed_kinds and node.name in getattr_names:
            live.add(node.id)

    # Fixed-point: live if inbound from live source, or overrides a live base method.
    unreachable_set = {p.replace("\\", "/") for p in (unreachable or [])}
    entry_root_set = {p.replace("\\", "/") for p in entry_roots}

    def _file_source_is_live(path: str) -> bool:
        """Module-level calls execute on import/run when the file is reachable."""
        norm = path.replace("\\", "/")
        if norm in entry_root_set or structural_exemptions(norm):
            return True
        return norm not in unreachable_set

    def _source_is_live(src: str) -> bool:
        if src in live:
            return True
        if "::" not in src:
            if _file_source_is_live(src):
                return True
            return any(n.startswith(f"{src}::") for n in live)
        return False

    changed = True
    while changed:
        changed = False
        for target, srcs in inbound_live_edges.items():
            if target in live:
                continue
            if any(_source_is_live(e.source) for e in srcs):
                live.add(target)
                changed = True
        for child, parents in overrides_of.items():
            if child in live:
                continue
            if any(p in live for p in parents):
                live.add(child)
                changed = True

    token_dead_keys = token_dead_keys or set()
    dead: List[DeadCodeEntry] = []

    for node in nodes:
        if node.kind not in {NodeKind.FUNCTION, NodeKind.CLASS, NodeKind.METHOD}:
            continue
        if is_test_path(node.path):
            continue
        if structural_exemptions(node.path):
            continue
        if is_private_symbol(node.name) or is_dunder_symbol(node.name):
            continue
        if has_allow_unwired(root, node.path):
            continue
        if node.id in live:
            continue
        decs = list((node.extras or {}).get("decorators") or [])
        if is_wiring_decorated(decs):
            continue
        if (node.path, node.name) in protected:
            continue

        key = f"{node.path}::{node.name}"
        qual_key = f"{node.path}::{(node.extras or {}).get('qualname') or node.name}"
        in_token = key in token_dead_keys or qual_key in token_dead_keys
        # Non-call references scoped to defining file + its importers (not global
        # bare-name collisions in unrelated modules).
        ref_files = ref_index.get(node.name, set())
        scoped_refs = {node.path} | imported_by.get(node.path, set())
        referenced_by_name = bool(ref_files & scoped_refs)

        # Relevant inbound call/named-import edges (exclude self-recursive).
        # Module-level file→symbol calls count when the source file is live.
        relevant = [
            e
            for e in inbound_live_edges.get(node.id, [])
            if e.source != node.id
        ]
        has_any_caller = bool(relevant)
        only_dead_callers = has_any_caller and all(
            not _source_is_live(e.source) for e in relevant
        )

        if node.kind == NodeKind.METHOD:
            if is_dunder_symbol(node.name):
                continue
            if referenced_by_name:
                dead.append(
                    DeadCodeEntry(
                        id=node.id,
                        path=node.path,
                        line=node.line,
                        kind="method",
                        confidence=Confidence.AMBIGUOUS,
                        reason="referenced by name only (possible callback/property)",
                    )
                )
                continue
            if only_dead_callers:
                dead.append(
                    DeadCodeEntry(
                        id=node.id,
                        path=node.path,
                        line=node.line,
                        kind="method",
                        confidence=Confidence.INFERRED,
                        reason="only callers are dead",
                    )
                )
                continue
            dead.append(
                DeadCodeEntry(
                    id=node.id,
                    path=node.path,
                    line=node.line,
                    kind="method",
                    confidence=Confidence.INFERRED,
                    reason="no inbound call edges (method)",
                )
            )
            continue

        # Top-level function/class
        if only_dead_callers:
            dead.append(
                DeadCodeEntry(
                    id=node.id,
                    path=node.path,
                    line=node.line,
                    kind=node.kind.value,
                    confidence=Confidence.INFERRED,
                    reason="only callers are dead",
                )
            )
            continue

        graph_dead = not has_any_caller
        if not graph_dead:
            # Has live-or-entry callers — should have been marked live; skip
            continue

        if graph_dead and in_token:
            dead.append(
                DeadCodeEntry(
                    id=node.id,
                    path=node.path,
                    line=node.line,
                    kind=node.kind.value,
                    confidence=(
                        Confidence.AMBIGUOUS if referenced_by_name else Confidence.EXTRACTED
                    ),
                    reason=(
                        "referenced by name only (possible callback)"
                        if referenced_by_name
                        else "no inbound calls and token-scan agrees"
                    ),
                )
            )
        elif graph_dead and not in_token:
            dead.append(
                DeadCodeEntry(
                    id=node.id,
                    path=node.path,
                    line=node.line,
                    kind=node.kind.value,
                    confidence=Confidence.AMBIGUOUS,
                    reason="graph-dead but token-scan cleared (possible name collision)",
                )
            )

    return dead


def legacy_dead_strings(
    dead_code: List[DeadCodeEntry],
    token_dead: List[str],
    *,
    cap: Optional[int] = None,
) -> List[str]:
    """repo_map.json format: intersection of extracted-confidence dead + token list.

    Preserves ``path:line name`` strings for verify-gate / ratchet compatibility.
    Methods (inferred) are excluded from the legacy list.
    ``cap`` defaults to uncapped; apply caps only when serializing ``repo_map.json``.
    """
    extracted_paths_names = {
        (d.path, d.id.split("::")[-1].split(".")[-1])
        for d in dead_code
        if d.confidence == Confidence.EXTRACTED
    }
    out: List[str] = []
    for entry in token_dead:
        loc, _, name = entry.partition(" ")
        path, _, _line = loc.rpartition(":")
        if (path, name) in extracted_paths_names:
            out.append(entry)
        elif not extracted_paths_names:
            # Graph had nothing — fall back to full token list (keep tests green
            # when graph extraction is empty / disabled).
            out.append(entry)
    # If intersection emptied everything but token had hits and graph had
    # extracted entries that didn't match format — keep token list for safety.
    if not out and token_dead and not any(
        d.confidence == Confidence.EXTRACTED for d in dead_code
    ):
        out = list(token_dead)

    if cap is not None and cap > 0:
        return out[:cap]
    return out


def confidence_at_least(conf: object, minimum: str) -> bool:
    """True when ``conf`` ranks at or above ``minimum`` (extracted > inferred > ambiguous)."""
    want = _CONFIDENCE_RANK.get(minimum, 0)
    if hasattr(conf, "value"):
        conf = conf.value
    have = _CONFIDENCE_RANK.get(str(conf), 0)
    return have >= want
