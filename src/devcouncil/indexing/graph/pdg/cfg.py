"""Python AST control-flow graph builder (intra-procedural MVP)."""

from __future__ import annotations

import ast
from dataclasses import dataclass, field
from typing import Dict, List, Optional, Set, Tuple

from devcouncil.indexing.graph.pdg.schema import BasicBlock, CFGEdge


@dataclass
class CFGResult:
    blocks: List[BasicBlock] = field(default_factory=list)
    edges: List[CFGEdge] = field(default_factory=list)
    entry: str = ""
    exits: List[str] = field(default_factory=list)
    block_by_line: Dict[int, str] = field(default_factory=dict)


def _block_id(path: str, qualname: str, start_line: int, index: int) -> str:
    return f"{path}::{qualname}:{start_line}:bb{index}"


def _stmt_lines(stmts: List[ast.stmt]) -> Tuple[int, int]:
    lines: List[int] = []
    for stmt in stmts:
        if hasattr(stmt, "lineno"):
            lines.append(int(stmt.lineno))
        if hasattr(stmt, "end_lineno") and stmt.end_lineno:
            lines.append(int(stmt.end_lineno))
    if not lines:
        return 0, 0
    return min(lines), max(lines)


def _first_line(node: ast.AST, default: int = 0) -> int:
    return int(getattr(node, "lineno", default) or default)


def _is_leader(stmt: ast.stmt) -> bool:
    return isinstance(
        stmt,
        (ast.If, ast.While, ast.For, ast.AsyncFor, ast.Try, ast.Return, ast.Raise, ast.Break, ast.Continue),
    )


class _CFGBuilder:
    def __init__(self, path: str, qualname: str, source_lines: List[str]) -> None:
        self.path = path
        self.qualname = qualname
        self.source_lines = source_lines
        self.blocks: List[BasicBlock] = []
        self.edges: List[CFGEdge] = []
        self._index = 0
        self.block_by_line: Dict[int, str] = {}
        self.exits: Set[str] = set()

    def _new_block(self, stmts: List[ast.stmt]) -> str:
        start, end = _stmt_lines(stmts)
        if not start and self.blocks:
            start = self.blocks[-1].end_line or self.blocks[-1].start_line
            end = start
        bid = _block_id(self.path, self.qualname, start or 1, self._index)
        self._index += 1
        text_lines: List[str] = []
        line_nums: List[int] = []
        for stmt in stmts:
            ln = _first_line(stmt)
            if ln:
                line_nums.append(ln)
                if 0 < ln <= len(self.source_lines):
                    text_lines.append(self.source_lines[ln - 1].rstrip())
        block = BasicBlock(
            id=bid,
            start_line=start or 1,
            end_line=end or start or 1,
            text="\n".join(text_lines),
            lines=line_nums,
        )
        self.blocks.append(block)
        for ln in line_nums:
            self.block_by_line.setdefault(ln, bid)
        return bid

    def _connect(self, source: Optional[str], target: Optional[str], kind: str = "fallthrough") -> None:
        if source and target and source != target:
            self.edges.append(CFGEdge(source=source, target=target, kind=kind))

    def _build_stmts(self, stmts: List[ast.stmt], *, after: Optional[str] = None) -> Tuple[Optional[str], List[str]]:
        if not stmts:
            return after, []
        current_after = after
        pending_exits: List[str] = []
        i = 0
        while i < len(stmts):
            group: List[ast.stmt] = []
            while i < len(stmts) and not _is_leader(stmts[i]):
                group.append(stmts[i])
                i += 1
            block_id: Optional[str] = None
            if group:
                block_id = self._new_block(group)
                self._connect(current_after, block_id)
                current_after = block_id
            if i >= len(stmts):
                break
            stmt = stmts[i]
            i += 1
            if isinstance(stmt, (ast.Return, ast.Raise)):
                term = block_id or current_after or self._new_block([stmt])
                ln = _first_line(stmt)
                if ln:
                    self.block_by_line[ln] = term
                self.exits.add(term)
                current_after = None
            elif isinstance(stmt, ast.If):
                test_block = block_id or current_after or self._new_block([stmt])
                then_end, then_exits = self._build_stmts(stmt.body)
                else_end, else_exits = self._build_stmts(stmt.orelse) if stmt.orelse else (test_block, [])
                merge = self._new_block([])
                if then_end:
                    self._connect(then_end, merge)
                    self._connect(test_block, then_end, "true")
                else:
                    self._connect(test_block, merge, "true")
                if stmt.orelse:
                    if else_end:
                        self._connect(else_end, merge)
                        self._connect(test_block, else_end, "false")
                    else:
                        self._connect(test_block, merge, "false")
                else:
                    self._connect(test_block, merge, "false")
                pending_exits.extend(then_exits + else_exits)
                current_after = merge
            elif isinstance(stmt, (ast.While, ast.For, ast.AsyncFor)):
                loop_header = block_id or current_after or self._new_block([stmt])
                body_end, body_exits = self._build_stmts(stmt.body)
                if body_end:
                    self._connect(body_end, loop_header, "loop")
                after_loop = self._new_block([])
                self._connect(loop_header, after_loop, "false")
                pending_exits.extend(body_exits)
                current_after = after_loop
            elif isinstance(stmt, ast.Try):
                _ = block_id or current_after or self._new_block([stmt])
                try_end, try_exits = self._build_stmts(stmt.body)
                join = self._new_block([])
                if try_end:
                    self._connect(try_end, join)
                for handler in stmt.handlers:
                    h_end, h_exits = self._build_stmts(handler.body)
                    if h_end:
                        self._connect(h_end, join, "exception")
                    pending_exits.extend(h_exits)
                pending_exits.extend(try_exits)
                current_after = join
            else:
                group2 = [stmt]
                while i < len(stmts) and not _is_leader(stmts[i]):
                    group2.append(stmts[i])
                    i += 1
                bid2 = self._new_block(group2)
                self._connect(current_after, bid2)
                current_after = bid2
        return current_after, pending_exits

    def build(self, body: List[ast.stmt]) -> CFGResult:
        entry = self._new_block([])
        end, exits = self._build_stmts(body, after=entry)
        if end:
            self.exits.add(end)
        self.exits.update(exits)
        return CFGResult(
            blocks=self.blocks,
            edges=self.edges,
            entry=entry,
            exits=sorted(self.exits),
            block_by_line=self.block_by_line,
        )


def build_cfg_for_function(
    path: str,
    qualname: str,
    node: ast.AST,
    source_lines: List[str],
) -> CFGResult:
    body: List[ast.stmt] = []
    if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
        body = list(node.body)
    elif isinstance(node, ast.Module):
        body = list(node.body)
    return _CFGBuilder(path, qualname, source_lines).build(body)


def block_for_line(cfg: CFGResult, line: int) -> Optional[str]:
    return cfg.block_by_line.get(line)
