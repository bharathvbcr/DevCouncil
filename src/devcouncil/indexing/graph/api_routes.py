"""API route mapping over the native code graph.

Deterministic, LLM-free: ROUTE nodes, ``routes_to`` / ``registers`` edges, plus
token scans for client fetch sites and handler/consumer response shapes.
"""

from __future__ import annotations

import ast
import re
from pathlib import Path
from typing import Any, Dict, Iterable, List, Optional, Set, Tuple

from devcouncil.indexing.graph.build import load_code_graph
from devcouncil.indexing.graph.schema import CodeGraph, GraphNode, NodeKind

_PARAM_RE = re.compile(
    r":\w+"  # FastAPI / Flask
    r"|\{[^}]+\}"  # Express / Spring
    r"|\[[^\]]+\]"  # Next.js dynamic segments
    r"|\$\{[^}]+\}"  # JS template-literal fetch URLs
)

_FETCH_LITERAL = re.compile(
    r"""fetch\s*\(\s*(['"`])(?P<url>[^'"`]+)\1""",
    re.IGNORECASE,
)
_FETCH_TEMPLATE = re.compile(
    r"""fetch\s*\(\s*`(?P<url>[^`]+)`""",
    re.IGNORECASE,
)
_AXIOS_LITERAL = re.compile(
    r"""(?:axios|axios\.default)\.(?P<verb>get|post|put|patch|delete)\s*\(\s*(['"`])(?P<url>[^'"`]+)\2""",
    re.IGNORECASE,
)
_REQUESTS_LITERAL = re.compile(
    r"""requests\.(?P<verb>get|post|put|patch|delete)\s*\(\s*(['"`])(?P<url>[^'"`]+)\2""",
    re.IGNORECASE,
)
_HTTPX_LITERAL = re.compile(
    r"""httpx\.(?P<verb>get|post|put|patch|delete)\s*\(\s*(['"`])(?P<url>[^'"`]+)\2""",
    re.IGNORECASE,
)
_RESP_VAR = re.compile(
    r"""(?:const|let|var)\s+(?P<var>\w+)\s*=\s*(?:await\s+)?(?:fetch|axios|requests|httpx)""",
    re.IGNORECASE,
)
_JSON_VAR = re.compile(
    r"""(?:const|let|var)\s+(?P<var>\w+)\s*=\s*(?:await\s+)?(?P<src>\w+)\.json\s*\(""",
    re.IGNORECASE,
)
_DESTRUCTURE = re.compile(
    r"""const\s*\{\s*(?P<keys>[^}]+)\s*\}\s*=\s*(?:await\s+)?""",
    re.IGNORECASE,
)
_TS_RETURN_DICT = re.compile(r"""return\s*\{([^}]+)\}""", re.MULTILINE | re.DOTALL)
_TS_KEY = re.compile(r"""['"]?(\w+)['"]?\s*:""")

_SCAN_WINDOW = 40

_ROUTE_HANDLER_KINDS = frozenset(
    {
        NodeKind.FUNCTION,
        NodeKind.METHOD,
        NodeKind.CLASS,
    }
)


def _load(root: Path, graph: Optional[CodeGraph] = None) -> Optional[CodeGraph]:
    return graph if graph is not None else load_code_graph(root)


def normalize_route_path(path: str) -> str:
    """Normalize ``:param``, ``{id}``, ``[id]``, ``${id}`` segments to ``*``."""
    norm = path.replace("\\", "/").strip()
    if not norm.startswith("/"):
        norm = "/" + norm.lstrip("/")
    return _PARAM_RE.sub("*", norm)


def _segments(path: str) -> List[str]:
    return [s for s in normalize_route_path(path).split("/") if s]


def paths_match(route_path: str, fetch_path: str) -> bool:
    """Segment-wise match after param normalization."""
    a, b = _segments(route_path), _segments(fetch_path)
    if len(a) != len(b):
        return False
    for left, right in zip(a, b):
        if left == "*" or right == "*":
            continue
        if left != right:
            return False
    return True


def _verbs_compatible(route_verb: str, fetch_verb: str) -> bool:
    rv = (route_verb or "ANY").upper()
    fv = (fetch_verb or "GET").upper()
    if rv in {"ANY", "FILE", "ROUTE"}:
        return True
    if fv == "GET" and rv in {"GET", "HEAD"}:
        return True
    return rv == fv


def _route_nodes(graph: CodeGraph) -> List[GraphNode]:
    return [n for n in graph.nodes if n.kind == NodeKind.ROUTE]


def _index_edges(graph: CodeGraph) -> Tuple[Dict[str, List[str]], Dict[str, List[str]]]:
    """``routes_to``: route -> handlers; ``registers``: route -> owners."""
    routes_to: Dict[str, List[str]] = {}
    registers: Dict[str, List[str]] = {}
    for e in graph.edges:
        if e.kind == "routes_to":
            routes_to.setdefault(e.source, []).append(e.target)
        elif e.kind == "registers":
            registers.setdefault(e.target, []).append(e.source)
    return routes_to, registers


def _node_map(graph: CodeGraph) -> Dict[str, GraphNode]:
    return graph.node_by_id()


def _read_lines(root: Path, rel: str) -> List[str]:
    try:
        return (root / rel).read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return []


def _scan_fetch_sites(root: Path, graph: CodeGraph) -> List[Dict[str, Any]]:
    seen_paths: Set[str] = set()
    for n in graph.nodes:
        if n.path:
            seen_paths.add(n.path.replace("\\", "/"))
    sites: List[Dict[str, Any]] = []
    for rel in sorted(seen_paths):
        lines = _read_lines(root, rel)
        if not lines:
            continue
        for line_no, line in enumerate(lines, start=1):
            for match in _FETCH_LITERAL.finditer(line):
                sites.append(_site(rel, line_no, match.group("url"), "GET", line, lines))
            for match in _FETCH_TEMPLATE.finditer(line):
                sites.append(_site(rel, line_no, match.group("url"), "GET", line, lines))
            for pat in (_AXIOS_LITERAL, _REQUESTS_LITERAL, _HTTPX_LITERAL):
                for match in pat.finditer(line):
                    sites.append(
                        _site(
                            rel,
                            line_no,
                            match.group("url"),
                            match.group("verb").upper(),
                            line,
                            lines,
                        )
                    )
    return sites


def _site(
    path: str,
    line: int,
    url: str,
    verb: str,
    line_text: str,
    lines: List[str],
) -> Dict[str, Any]:
    window = lines[line - 1 : line - 1 + _SCAN_WINDOW]
    response_var = ""
    for wline in window:
        m = _RESP_VAR.search(wline)
        if m:
            response_var = m.group("var")
            break
    data_var = response_var
    for wline in window:
        m = _JSON_VAR.search(wline)
        if m and (not response_var or m.group("src") == response_var):
            data_var = m.group("var")
            break
    accessed = _consumer_keys(window, data_var or response_var)
    return {
        "path": path,
        "line": line,
        "url": url.strip(),
        "verb": verb,
        "response_var": data_var or response_var,
        "accessed_keys": sorted(accessed),
    }


def _consumer_keys(window: Iterable[str], response_var: str) -> Set[str]:
    keys: Set[str] = set()
    text = "\n".join(window)
    if response_var:
        dot = re.compile(rf"\b{re.escape(response_var)}\.(\w+)")
        bracket = re.compile(
            rf"\b{re.escape(response_var)}\s*\[\s*['\"](\w+)['\"]\s*\]"
        )
        for m in dot.finditer(text):
            keys.add(m.group(1))
        for m in bracket.finditer(text):
            keys.add(m.group(1))
    for m in _DESTRUCTURE.finditer(text):
        for part in m.group("keys").split(","):
            name = part.strip().split(":")[0].strip()
            if name and re.match(r"^\w+$", name):
                keys.add(name)
    return keys


def _handler_node(handler_id: str, nodes: Dict[str, GraphNode]) -> Optional[GraphNode]:
    node = nodes.get(handler_id)
    if node is not None:
        return node
    if "::" not in handler_id:
        return nodes.get(f"{handler_id}::{handler_id.split('/')[-1]}")
    return None


def _following_handler(route: GraphNode, graph: CodeGraph) -> Optional[str]:
    """Resolve decorator-style handler: next callable after the route line."""
    line = route.line or 0
    path = route.path
    candidates = [
        node
        for node in graph.nodes
        if node.path == path
        and node.kind in _ROUTE_HANDLER_KINDS
        and line < node.line <= line + 8
    ]
    if not candidates:
        return None
    return min(candidates, key=lambda node: node.line).id


def _resolve_route_handlers(
    route: GraphNode,
    handler_ids: List[str],
    nodes: Dict[str, GraphNode],
    graph: CodeGraph,
) -> List[str]:
    """Fix routes_to edges that point at the ROUTE node instead of the handler."""
    resolved: List[str] = []
    seen: Set[str] = set()
    for handler_id in handler_ids:
        node = _handler_node(handler_id, nodes)
        needs_fallback = (
            node is None
            or node.kind == NodeKind.ROUTE
            or node.id == route.id
            or node.kind not in _ROUTE_HANDLER_KINDS
            or (route.line and node.line <= route.line)
        )
        if needs_fallback:
            fallback = _following_handler(route, graph)
            if fallback and fallback not in seen:
                resolved.append(fallback)
                seen.add(fallback)
            continue
        if handler_id not in seen:
            resolved.append(handler_id)
            seen.add(handler_id)
    if not resolved:
        fallback = _following_handler(route, graph)
        if fallback:
            resolved.append(fallback)
    return resolved


def _python_return_keys(root: Path, node: GraphNode) -> Set[str]:
    lines = _read_lines(root, node.path)
    if not lines:
        return set()
    try:
        tree = ast.parse("\n".join(lines))
    except SyntaxError:
        return set()
    keys: Set[str] = set()
    for fn in ast.walk(tree):
        if not isinstance(fn, (ast.FunctionDef, ast.AsyncFunctionDef)):
            continue
        if fn.name != node.name and node.name not in {fn.name}:
            if node.line and not (fn.lineno <= node.line <= (fn.end_lineno or fn.lineno)):
                continue
        for sub in ast.walk(fn):
            if isinstance(sub, ast.Return) and isinstance(sub.value, ast.Dict):
                for k in sub.value.keys:
                    if isinstance(k, ast.Constant) and isinstance(k.value, str):
                        keys.add(k.value)
    return keys


def _ts_return_keys(root: Path, node: GraphNode) -> Set[str]:
    lines = _read_lines(root, node.path)
    if not lines:
        return set()
    start = max(0, (node.line or 1) - 1)
    end = min(len(lines), start + 120)
    chunk = "\n".join(lines[start:end])
    keys: Set[str] = set()
    for m in _TS_RETURN_DICT.finditer(chunk):
        for km in _TS_KEY.finditer(m.group(1)):
            keys.add(km.group(1))
    return keys


def handler_return_keys(root: Path, handler: GraphNode) -> Set[str]:
    suffix = Path(handler.path).suffix.lower()
    if suffix == ".py":
        return _python_return_keys(root, handler)
    return _ts_return_keys(root, handler)


def _summarize_handler(handler_id: str, nodes: Dict[str, GraphNode]) -> Optional[Dict[str, Any]]:
    node = _handler_node(handler_id, nodes)
    if node is None:
        return {"id": handler_id}
    kind = node.kind.value if hasattr(node.kind, "value") else str(node.kind)
    return {
        "id": node.id,
        "path": node.path,
        "name": node.name,
        "line": node.line,
        "kind": kind,
    }


def route_map(root: Path, graph: Optional[CodeGraph] = None) -> Dict[str, Any]:
    """Map HTTP routes to handlers, registrations, and client fetch consumers."""
    g = _load(root, graph)
    if g is None:
        return {"error": "no code graph; run `dev map` first", "routes": []}

    nodes = _node_map(g)
    routes_to, registers = _index_edges(g)
    fetch_sites = _scan_fetch_sites(root, g)
    routes_out: List[Dict[str, Any]] = []

    for route in sorted(_route_nodes(g), key=lambda n: (n.extras.get("route", ""), n.extras.get("verb", ""), n.id)):
        route_path = str(route.extras.get("route") or "")
        verb = str(route.extras.get("verb") or "ANY")
        handler_ids = _resolve_route_handlers(route, routes_to.get(route.id, []), nodes, g)
        handlers = [_summarize_handler(hid, nodes) for hid in handler_ids]
        handler_keys: Set[str] = set()
        for hid in handler_ids:
            hnode = _handler_node(hid, nodes)
            if hnode is not None:
                handler_keys.update(handler_return_keys(root, hnode))

        consumers: List[Dict[str, Any]] = []
        for site in fetch_sites:
            if not paths_match(route_path, site["url"]):
                continue
            if not _verbs_compatible(verb, site["verb"]):
                continue
            consumers.append(dict(site))

        reg_sources = []
        for owner_id in registers.get(route.id, []):
            owner = nodes.get(owner_id)
            reg_sources.append(
                {
                    "id": owner_id,
                    "path": owner.path if owner else "",
                    "name": owner.name if owner else owner_id,
                }
            )

        routes_out.append(
            {
                "id": route.id,
                "path": route_path,
                "normalized_path": normalize_route_path(route_path),
                "verb": verb,
                "framework": route.extras.get("framework", ""),
                "file": route.path,
                "line": route.line,
                "handlers": [h for h in handlers if h],
                "registrations": reg_sources,
                "handler_keys": sorted(handler_keys),
                "consumers": consumers,
            }
        )

    return {"routes": routes_out, "count": len(routes_out)}


def _route_matches_filter(route: Dict[str, Any], route_filter: str) -> bool:
    q = route_filter.strip()
    if not q:
        return True
    norm_q = normalize_route_path(q)
    if q in (route.get("path") or "") or q in (route.get("id") or ""):
        return True
    if norm_q == route.get("normalized_path"):
        return True
    return paths_match(route.get("path") or "", q)


def shape_check(
    root: Path,
    graph: Optional[CodeGraph] = None,
    route_filter: Optional[str] = None,
) -> Dict[str, Any]:
    """Compare handler return dict keys vs consumer accessed keys."""
    mapped = route_map(root, graph)
    if mapped.get("error"):
        return mapped

    checks: List[Dict[str, Any]] = []
    for route in mapped.get("routes") or []:
        if route_filter and not _route_matches_filter(route, route_filter):
            continue
        handler_keys = set(route.get("handler_keys") or [])
        consumer_keys: Set[str] = set()
        for consumer in route.get("consumers") or []:
            consumer_keys.update(consumer.get("accessed_keys") or [])

        missing_in_handler = sorted(consumer_keys - handler_keys)
        unused_by_consumers = sorted(handler_keys - consumer_keys)
        mismatch = bool(missing_in_handler) if consumer_keys else False

        checks.append(
            {
                "route": route.get("path"),
                "verb": route.get("verb"),
                "route_id": route.get("id"),
                "handler_keys": sorted(handler_keys),
                "consumer_keys": sorted(consumer_keys),
                "missing_in_handler": missing_in_handler,
                "unused_by_consumers": unused_by_consumers,
                "mismatch": mismatch,
                "consumer_count": len(route.get("consumers") or []),
            }
        )

    mismatches = [c for c in checks if c["mismatch"]]
    return {
        "checks": checks,
        "mismatch_count": len(mismatches),
        "mismatches": mismatches,
    }


def _risk_level(
    *,
    consumers: List[Dict[str, Any]],
    mismatches: List[Dict[str, Any]],
) -> str:
    if not consumers:
        return "none"
    if mismatches and len(consumers) >= 2:
        return "high"
    if mismatches:
        return "medium"
    if len(consumers) >= 2:
        return "low"
    return "low"


def api_impact(
    root: Path,
    route_or_path: str,
    graph: Optional[CodeGraph] = None,
) -> Dict[str, Any]:
    """Consumers, middleware registrations, shape mismatches, and risk for one route."""
    mapped = route_map(root, graph)
    if mapped.get("error"):
        return {"error": mapped["error"], "route": route_or_path}

    matched = [
        r for r in (mapped.get("routes") or []) if _route_matches_filter(r, route_or_path)
    ]
    if not matched:
        return {
            "route": route_or_path,
            "found": False,
            "consumers": [],
            "middleware": [],
            "shape_mismatches": [],
            "risk": "none",
        }

    # Prefer exact path match; otherwise first segment match.
    route = matched[0]
    if len(matched) > 1:
        exact = [r for r in matched if r.get("path") == route_or_path or r.get("id") == route_or_path]
        if exact:
            route = exact[0]

    shape = shape_check(root, graph, route_filter=route.get("path") or route_or_path)
    route_shape = next(
        (c for c in shape.get("checks") or [] if c.get("route_id") == route.get("id")),
        None,
    )
    mismatches = shape.get("mismatches") or []
    if route_shape and route_shape.get("mismatch"):
        mismatches = [route_shape]

    consumers = route.get("consumers") or []
    middleware = route.get("registrations") or []
    risk = _risk_level(consumers=consumers, mismatches=mismatches)

    return {
        "route": route.get("path"),
        "route_id": route.get("id"),
        "verb": route.get("verb"),
        "found": True,
        "handlers": route.get("handlers") or [],
        "consumers": consumers,
        "middleware": middleware,
        "handler_keys": route.get("handler_keys") or [],
        "shape_mismatches": mismatches,
        "risk": risk,
    }
