"""Serializable PDG layer types."""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, cast, Dict, List, Literal, Optional

PDG_VERSION = 1

TaintCategory = Literal[
    "command-injection",
    "path-traversal",
    "sql-injection",
    "code-injection",
    "ssrf",
    "deserialization",
    "other",
]


@dataclass
class BasicBlock:
    id: str
    start_line: int
    end_line: int
    text: str = ""
    lines: List[int] = field(default_factory=list)

    def to_dict(self) -> dict[str, Any]:
        return {
            "id": self.id,
            "start_line": self.start_line,
            "end_line": self.end_line,
            "text": self.text,
            "lines": self.lines,
        }

    @classmethod
    def from_dict(cls, raw: dict[str, Any]) -> "BasicBlock":
        return cls(
            id=str(raw["id"]),
            start_line=int(raw["start_line"]),
            end_line=int(raw["end_line"]),
            text=str(raw.get("text") or ""),
            lines=[int(x) for x in raw.get("lines") or []],
        )


@dataclass
class CFGEdge:
    source: str
    target: str
    kind: str  # fallthrough | true | false | loop | exception | return

    def to_dict(self) -> dict[str, Any]:
        return {"source": self.source, "target": self.target, "kind": self.kind}

    @classmethod
    def from_dict(cls, raw: dict[str, Any]) -> "CFGEdge":
        return cls(
            source=str(raw["source"]),
            target=str(raw["target"]),
            kind=str(raw.get("kind") or "fallthrough"),
        )


@dataclass
class DataDepEdge:
    variable: str
    def_line: int
    use_line: int
    def_block: str
    use_block: str

    def to_dict(self) -> dict[str, Any]:
        return {
            "variable": self.variable,
            "def_line": self.def_line,
            "use_line": self.use_line,
            "def_block": self.def_block,
            "use_block": self.use_block,
        }

    @classmethod
    def from_dict(cls, raw: dict[str, Any]) -> "DataDepEdge":
        return cls(
            variable=str(raw["variable"]),
            def_line=int(raw["def_line"]),
            use_line=int(raw["use_line"]),
            def_block=str(raw["def_block"]),
            use_block=str(raw["use_block"]),
        )


@dataclass
class CDGEdge:
    controller: str
    dependent: str
    branch: str  # T | F | *
    guard: bool = False

    def to_dict(self) -> dict[str, Any]:
        return {
            "controller": self.controller,
            "dependent": self.dependent,
            "branch": self.branch,
            "guard": self.guard,
        }

    @classmethod
    def from_dict(cls, raw: dict[str, Any]) -> "CDGEdge":
        return cls(
            controller=str(raw["controller"]),
            dependent=str(raw["dependent"]),
            branch=str(raw.get("branch") or "*"),
            guard=bool(raw.get("guard")),
        )


@dataclass
class TaintFinding:
    path: str
    function: str
    category: TaintCategory
    source_line: int
    sink_line: int
    variable: str
    source_expr: str
    sink_expr: str
    confidence: str = "extracted"

    def to_dict(self) -> dict[str, Any]:
        return {
            "path": self.path,
            "function": self.function,
            "category": self.category,
            "source_line": self.source_line,
            "sink_line": self.sink_line,
            "variable": self.variable,
            "source_expr": self.source_expr,
            "sink_expr": self.sink_expr,
            "confidence": self.confidence,
        }

    @classmethod
    def from_dict(cls, raw: dict[str, Any]) -> "TaintFinding":
        return cls(
            path=str(raw["path"]),
            function=str(raw["function"]),
            category=cast(TaintCategory, raw.get("category") or "other"),
            source_line=int(raw["source_line"]),
            sink_line=int(raw["sink_line"]),
            variable=str(raw.get("variable") or ""),
            source_expr=str(raw.get("source_expr") or ""),
            sink_expr=str(raw.get("sink_expr") or ""),
            confidence=str(raw.get("confidence") or "extracted"),
        )


@dataclass
class FunctionPDG:
    path: str
    qualname: str
    start_line: int
    end_line: int
    blocks: List[BasicBlock] = field(default_factory=list)
    cfg_edges: List[CFGEdge] = field(default_factory=list)
    reaching_def: List[DataDepEdge] = field(default_factory=list)
    cdg: List[CDGEdge] = field(default_factory=list)
    taint: List[TaintFinding] = field(default_factory=list)

    def to_dict(self) -> dict[str, Any]:
        return {
            "path": self.path,
            "qualname": self.qualname,
            "start_line": self.start_line,
            "end_line": self.end_line,
            "blocks": [b.to_dict() for b in self.blocks],
            "cfg_edges": [e.to_dict() for e in self.cfg_edges],
            "reaching_def": [e.to_dict() for e in self.reaching_def],
            "cdg": [e.to_dict() for e in self.cdg],
            "taint": [t.to_dict() for t in self.taint],
        }

    @classmethod
    def from_dict(cls, raw: dict[str, Any]) -> "FunctionPDG":
        return cls(
            path=str(raw["path"]),
            qualname=str(raw["qualname"]),
            start_line=int(raw["start_line"]),
            end_line=int(raw["end_line"]),
            blocks=[BasicBlock.from_dict(b) for b in raw.get("blocks") or []],
            cfg_edges=[CFGEdge.from_dict(e) for e in raw.get("cfg_edges") or []],
            reaching_def=[DataDepEdge.from_dict(e) for e in raw.get("reaching_def") or []],
            cdg=[CDGEdge.from_dict(e) for e in raw.get("cdg") or []],
            taint=[TaintFinding.from_dict(t) for t in raw.get("taint") or []],
        )


@dataclass
class FilePDG:
    path: str
    language: str
    functions: List[FunctionPDG] = field(default_factory=list)

    def to_dict(self) -> dict[str, Any]:
        return {
            "path": self.path,
            "language": self.language,
            "functions": [f.to_dict() for f in self.functions],
        }

    @classmethod
    def from_dict(cls, raw: dict[str, Any]) -> "FilePDG":
        return cls(
            path=str(raw["path"]),
            language=str(raw.get("language") or ""),
            functions=[FunctionPDG.from_dict(f) for f in raw.get("functions") or []],
        )


@dataclass
class PDGLayer:
    version: int = PDG_VERSION
    files: Dict[str, FilePDG] = field(default_factory=dict)
    taint_findings: List[TaintFinding] = field(default_factory=list)

    def to_meta(self) -> dict[str, Any]:
        findings = [f.to_dict() for f in self.taint_findings]
        stats = {
            "file_count": len(self.files),
            "function_count": sum(len(f.functions) for f in self.files.values()),
            "cfg_edge_count": sum(
                len(fn.cfg_edges) for f in self.files.values() for fn in f.functions
            ),
            "reaching_def_count": sum(
                len(fn.reaching_def) for f in self.files.values() for fn in f.functions
            ),
            "cdg_edge_count": sum(len(fn.cdg) for f in self.files.values() for fn in f.functions),
            "taint_count": len(findings),
        }
        return {
            "version": self.version,
            "stats": stats,
            "files": sorted(self.files.keys()),
            "taint_findings": findings[:500],
        }

    def shard_payload(self, path: str) -> Optional[dict[str, Any]]:
        file_pdg = self.files.get(path)
        if file_pdg is None:
            return None
        return {"pdg": file_pdg.to_dict()}
