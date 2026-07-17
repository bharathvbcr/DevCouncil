"""Reaching-definitions analysis on a CFG."""

from __future__ import annotations

import ast
from dataclasses import dataclass, field
from typing import Dict, Iterable, List, Set, Tuple

from devcouncil.indexing.graph.pdg.cfg import CFGResult
from devcouncil.indexing.graph.pdg.schema import DataDepEdge


@dataclass
class BlockDefUse:
    gen: Set[Tuple[str, int]] = field(default_factory=set)
    kill: Set[str] = field(default_factory=set)
    use: Set[Tuple[str, int]] = field(default_factory=set)


def _store_target(node: ast.AST) -> str | None:
    if isinstance(node, ast.Name):
        return node.id
    if isinstance(node, ast.Attribute):
        return node.attr
    return None


class _DefUseVisitor(ast.NodeVisitor):
    def __init__(self, block_lines: Set[int]) -> None:
        self.block_lines = block_lines
        self.gen: Set[Tuple[str, int]] = set()
        self.kill: Set[str] = set()
        self.use: Set[Tuple[str, int]] = set()

    def _in_block(self, node: ast.AST) -> bool:
        ln = getattr(node, "lineno", None)
        return ln is not None and int(ln) in self.block_lines

    def visit_Name(self, node: ast.Name) -> None:
        if not self._in_block(node):
            return
        if isinstance(node.ctx, ast.Load):
            self.use.add((node.id, int(node.lineno)))
        elif isinstance(node.ctx, ast.Store):
            self.gen.add((node.id, int(node.lineno)))
            self.kill.add(node.id)

    def visit_Attribute(self, node: ast.Attribute) -> None:
        if isinstance(node.ctx, ast.Load) and self._in_block(node):
            self.use.add((node.attr, int(node.lineno)))
        self.generic_visit(node)

    def visit_arg(self, node: ast.arg) -> None:
        if node.lineno in self.block_lines:
            self.gen.add((node.arg, int(node.lineno)))


def block_def_use(node: ast.AST, block_lines: Iterable[int]) -> BlockDefUse:
    lines = set(block_lines)
    visitor = _DefUseVisitor(lines)
    if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
        for arg in node.args.args + node.args.kwonlyargs:
            if arg.lineno in lines:
                visitor.gen.add((arg.arg, int(arg.lineno)))
        for d in node.args.defaults + node.args.kw_defaults:
            if d is not None:
                visitor.visit(d)
        visitor.visit(node)
    else:
        visitor.visit(node)
    return BlockDefUse(gen=visitor.gen, kill=visitor.kill, use=visitor.use)


def compute_reaching_defs(
    cfg: CFGResult,
    func_node: ast.AST,
) -> List[DataDepEdge]:
    """Iterative reaching-definitions; emit def->use edges."""
    block_map = {b.id: set(b.lines) for b in cfg.blocks}
    in_sets: Dict[str, Set[Tuple[str, int]]] = {b.id: set() for b in cfg.blocks}
    out_sets: Dict[str, Set[Tuple[str, int]]] = {b.id: set() for b in cfg.blocks}
    du: Dict[str, BlockDefUse] = {}
    for block in cfg.blocks:
        du[block.id] = block_def_use(func_node, block_map[block.id])

    preds: Dict[str, Set[str]] = {b.id: set() for b in cfg.blocks}
    for edge in cfg.edges:
        preds.setdefault(edge.target, set()).add(edge.source)

    changed = True
    while changed:
        changed = False
        for block in cfg.blocks:
            bid = block.id
            new_in: Set[Tuple[str, int]] = set()
            for pred in preds.get(bid, set()):
                new_in |= out_sets.get(pred, set())
            if new_in != in_sets[bid]:
                in_sets[bid] = new_in
                changed = True
            gen = du[bid].gen
            kill = du[bid].kill
            new_out = (new_in - {(v, ln) for v, ln in new_in if v in kill}) | gen
            if new_out != out_sets[bid]:
                out_sets[bid] = new_out
                changed = True

    edges: List[DataDepEdge] = []
    for block in cfg.blocks:
        bid = block.id
        reaching = in_sets[bid] | du[bid].gen
        for var, use_line in du[bid].use:
            for def_var, def_line in reaching:
                if def_var == var and def_line <= use_line:
                    def_block = cfg.block_by_line.get(def_line, bid)
                    use_block = cfg.block_by_line.get(use_line, bid)
                    edges.append(
                        DataDepEdge(
                            variable=var,
                            def_line=def_line,
                            use_line=use_line,
                            def_block=def_block,
                            use_block=use_block,
                        )
                    )
    return edges
