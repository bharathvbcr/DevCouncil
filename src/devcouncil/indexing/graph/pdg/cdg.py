"""Control-dependence graph from CFG branch structure (MVP)."""

from __future__ import annotations

import ast
from typing import List, Set

from devcouncil.indexing.graph.pdg.cfg import CFGResult
from devcouncil.indexing.graph.pdg.schema import BasicBlock, CDGEdge


def _is_guard_block(block: BasicBlock) -> bool:
    text = block.text.strip()
    return text.startswith("return") or text.startswith("raise")


def build_cdg(cfg: CFGResult, func_node: ast.AST) -> List[CDGEdge]:
    """Build CDG edges from branch terminators to dependent blocks."""
    del func_node  # reserved for richer branch conditions
    cdg: List[CDGEdge] = []
    seen: Set[tuple[str, str, str]] = set()
    branch_kinds = {"true", "false", "loop", "exception"}
    for edge in cfg.edges:
        if edge.kind not in branch_kinds:
            continue
        branch = "T" if edge.kind in {"true", "loop"} else "F"
        key = (edge.source, edge.target, branch)
        if key in seen:
            continue
        seen.add(key)
        dependent = next((b for b in cfg.blocks if b.id == edge.target), None)
        guard = bool(dependent and _is_guard_block(dependent))
        cdg.append(
            CDGEdge(
                controller=edge.source,
                dependent=edge.target,
                branch=branch,
                guard=guard,
            )
        )
    return cdg
