"""Pydantic schema for the symbol-level code knowledge graph.

devcouncil: allow-unwired — package-private types; imported by sibling graph modules.
"""

from __future__ import annotations

from enum import Enum
from typing import Any, Dict, List

from pydantic import BaseModel, Field


SCHEMA_VERSION = 2


class NodeKind(str, Enum):
    FILE = "file"
    FUNCTION = "function"
    CLASS = "class"
    METHOD = "method"
    INTERFACE = "interface"
    TYPE = "type"
    STRUCT = "struct"
    ENUM = "enum"
    TRAIT = "trait"
    RATIONALE = "rationale"


class Confidence(str, Enum):
    EXTRACTED = "extracted"
    INFERRED = "inferred"
    AMBIGUOUS = "ambiguous"


class GraphNode(BaseModel):
    id: str
    kind: NodeKind
    path: str = ""
    name: str = ""
    line: int = 0
    end_line: int = 0
    area: str = ""
    language: str = ""
    exported: bool = False
    community: str = ""
    extras: Dict[str, Any] = Field(default_factory=dict)


class GraphEdge(BaseModel):
    source: str
    target: str
    kind: str  # contains | imports | calls | inherits | implements | overrides | decorates | documents
    confidence: Confidence = Confidence.EXTRACTED
    reason: str = ""
    extras: Dict[str, Any] = Field(default_factory=dict)


class DeadCodeEntry(BaseModel):
    id: str
    path: str
    line: int = 0
    kind: str = ""
    confidence: Confidence = Confidence.INFERRED
    reason: str = ""


class CodeGraph(BaseModel):
    schema_version: int = SCHEMA_VERSION
    nodes: List[GraphNode] = Field(default_factory=list)
    edges: List[GraphEdge] = Field(default_factory=list)
    dead_code: List[DeadCodeEntry] = Field(default_factory=list)
    entry_roots: List[str] = Field(default_factory=list)
    unwired_candidates: List[str] = Field(default_factory=list)
    unreachable_files: List[str] = Field(default_factory=list)
    generated_head: str = ""
    indexed_hash: str = ""
    content_fingerprint: str = ""
    meta: Dict[str, Any] = Field(default_factory=dict)

    def node_by_id(self) -> Dict[str, GraphNode]:
        return {n.id: n for n in self.nodes}


def file_node_id(path: str) -> str:
    return path.replace("\\", "/")


def symbol_node_id(path: str, qualname: str) -> str:
    """Deterministic id: ``src/x.py::Class.method``."""
    norm = path.replace("\\", "/")
    return f"{norm}::{qualname}"
