"""Pydantic schema for the symbol-level code knowledge graph.

devcouncil: allow-unwired — package-private types; imported by sibling graph modules.
"""

from __future__ import annotations

from enum import Enum
from typing import Any, Dict, List, Optional

from pydantic import BaseModel, Field


SCHEMA_VERSION = 2


class NodeKind(str, Enum):
    FILE = "file"
    MODULE = "module"
    NAMESPACE = "namespace"
    PACKAGE = "package"
    FUNCTION = "function"
    CLASS = "class"
    METHOD = "method"
    INTERFACE = "interface"
    TYPE = "type"
    STRUCT = "struct"
    ENUM = "enum"
    TRAIT = "trait"
    PROPERTY = "property"
    VARIABLE = "variable"
    ROUTE = "route"
    EVENT = "event"
    STATE = "state"
    PROVIDER = "provider"
    COMPONENT = "component"
    DYNAMIC = "dynamic"
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
    # The evidence tier the kernel resolved this edge by (the store's
    # `ResolutionKind` spelling: SameFile, ImportScoped, ReceiverType,
    # UniqueGlobal, AmbiguousGlobal, Unresolved, Structural) and whether that
    # tier was resolved in-process, read back from the store, or reconstructed
    # from a generation that predates the persisted column. Empty on an edge
    # written by a kernel that did not emit them.
    resolution: str = ""
    resolution_source: str = ""
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
    #: Abandoned cycles: components of the call graph nothing outside reaches.
    #:
    #: ``code_graph.json`` has carried these since the component pass landed and
    #: this model had no field for them, so pydantic dropped every one at load
    #: and the highest-recall finding in the analysis was invisible to every
    #: Python consumer. The claim is narrow and deliberately not a BFS from entry
    #: roots: every symbol the component declares belongs to a group that nothing
    #: outside reaches, where "reaches" already excludes ambiguous edges and
    #: already exempts anything exported or wired.
    #:
    #: Untyped rows rather than a model, matching ``meta``: the kernel owns the
    #: shape, it is capped at the source, and a second declaration here is a
    #: second thing to keep in step.
    #: ``None``, not ``[]``, when the key is absent: a ``code_graph.json``
    #: written before the component pass existed did not run it, and an empty
    #: list would say it ran and found nothing. That is the same distinction
    #: ``AnalysisSummary.discovery_refused_files`` keeps for the same reason.
    dead_clusters: Optional[List[Dict[str, Any]]] = None
    #: Components found and not listed, because the kernel's cap cut them.
    dead_clusters_truncated: int = 0
    # Why ``dead_clusters`` is ``None`` when the kernel ran the pass and refused
    # it. Without a field here pydantic drops the key, and the refusal renders
    # as the generation simply predating the pass — which tells a reader to
    # rebuild, and the rebuild refuses again.
    dead_clusters_incomplete: Optional[str] = None
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
