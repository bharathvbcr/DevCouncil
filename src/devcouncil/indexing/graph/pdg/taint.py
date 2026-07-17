"""Heuristic intra-procedural taint analysis."""

from __future__ import annotations

import ast
from dataclasses import dataclass
from typing import Dict, Iterable, List, Set, Tuple

from devcouncil.indexing.graph.pdg.schema import DataDepEdge, TaintCategory, TaintFinding


@dataclass(frozen=True)
class _Site:
    line: int
    variable: str
    expr: str
    category: TaintCategory


_SOURCE_PATTERNS: Dict[TaintCategory, Tuple[str, ...]] = {
    "command-injection": ("input", "sys.argv", "os.environ.get", "request.args", "request.form"),
    "path-traversal": ("input", "sys.argv", "os.environ.get"),
    "sql-injection": ("input", "sys.argv", "request.args"),
    "code-injection": ("input", "sys.argv"),
    "ssrf": ("input", "sys.argv", "request.url"),
    "deserialization": ("json.loads", "yaml.load", "pickle.loads"),
    "other": (),
}

_SINK_PATTERNS: Dict[TaintCategory, Tuple[str, ...]] = {
    "command-injection": ("os.system", "subprocess.call", "subprocess.run", "subprocess.Popen", "os.popen"),
    "path-traversal": ("open", "Path", "shutil.copy", "shutil.move"),
    "sql-injection": (".execute", ".raw", "executemany"),
    "code-injection": ("eval", "exec", "compile"),
    "ssrf": ("urlopen", "requests.get", "requests.post", "httpx.get", "httpx.post"),
    "deserialization": (),
    "other": (),
}


def _call_name(node: ast.Call) -> str:
    func = node.func
    if isinstance(func, ast.Name):
        return func.id
    if isinstance(func, ast.Attribute):
        parts: List[str] = []
        cur: ast.AST = func
        while isinstance(cur, ast.Attribute):
            parts.append(cur.attr)
            cur = cur.value
        if isinstance(cur, ast.Name):
            parts.append(cur.id)
        return ".".join(reversed(parts))
    return ""


class _SiteVisitor(ast.NodeVisitor):
    def __init__(self) -> None:
        self.sites: List[_Site] = []

    def visit_Call(self, node: ast.Call) -> None:
        name = _call_name(node)
        line = int(node.lineno)
        for category, patterns in _SINK_PATTERNS.items():
            if any(p in name for p in patterns):
                var = ""
                if node.args and isinstance(node.args[0], ast.Name):
                    var = node.args[0].id
                self.sites.append(_Site(line=line, variable=var, expr=name, category=category))
        for category, patterns in _SOURCE_PATTERNS.items():
            if any(name.endswith(p) or name == p for p in patterns):
                var = ""
                if node.args and isinstance(node.args[0], ast.Name):
                    var = node.args[0].id
                self.sites.append(_Site(line=line, variable=var or name, expr=name, category=category))
        self.generic_visit(node)

    def visit_Name(self, node: ast.Name) -> None:
        if isinstance(node.ctx, ast.Load) and node.id == "input":
            self.sites.append(_Site(line=int(node.lineno), variable="input", expr="input", category="command-injection"))
        self.generic_visit(node)


def _param_sources(func_node: ast.AST) -> List[_Site]:
    sites: List[_Site] = []
    if not isinstance(func_node, (ast.FunctionDef, ast.AsyncFunctionDef)):
        return sites
    for arg in func_node.args.args:
        sites.append(_Site(line=int(arg.lineno), variable=arg.arg, expr=arg.arg, category="other"))
    return sites


def _reaching_map(edges: Iterable[DataDepEdge]) -> Dict[Tuple[int, str], Set[Tuple[int, str]]]:
    m: Dict[Tuple[int, str], Set[Tuple[int, str]]] = {}
    for edge in edges:
        m.setdefault((edge.use_line, edge.variable), set()).add((edge.def_line, edge.variable))
    return m


def analyze_taint(
    path: str,
    qualname: str,
    func_node: ast.AST,
    reaching: List[DataDepEdge],
) -> List[TaintFinding]:
    visitor = _SiteVisitor()
    visitor.visit(func_node)
    sites = _param_sources(func_node) + visitor.sites
    sources = [s for s in sites if s.category != "other" or s.expr in {p for pats in _SOURCE_PATTERNS.values() for p in pats}]
    sinks = [s for s in sites if any(s.expr.startswith(p) or p in s.expr for pats in _SINK_PATTERNS.values() for p in pats if pats)]
    reach = _reaching_map(reaching)
    findings: List[TaintFinding] = []
    seen: Set[tuple[int, int, str]] = set()
    for src in sources:
        for sink in sinks:
            if sink.line < src.line:
                continue
            if src.category != "other" and sink.category != "other" and src.category != sink.category:
                continue
            tainted = {src.variable}
            if sink.variable and sink.variable not in tainted:
                # walk reaching defs from sink arg
                queue = [(sink.line, sink.variable)]
                ok = False
                visited: Set[Tuple[int, str]] = set()
                while queue:
                    use_line, var = queue.pop()
                    if (use_line, var) in visited:
                        continue
                    visited.add((use_line, var))
                    if var == src.variable or (src.line, src.variable) in reach.get((use_line, var), set()):
                        ok = True
                        break
                    for def_line, def_var in reach.get((use_line, var), set()):
                        queue.append((def_line, def_var))
                if not ok and sink.variable != src.variable:
                    continue
            key = (src.line, sink.line, sink.category)
            if key in seen:
                continue
            seen.add(key)
            findings.append(
                TaintFinding(
                    path=path,
                    function=qualname,
                    category=sink.category if sink.category != "other" else src.category,
                    source_line=src.line,
                    sink_line=sink.line,
                    variable=sink.variable or src.variable,
                    source_expr=src.expr,
                    sink_expr=sink.expr,
                )
            )
    return findings
