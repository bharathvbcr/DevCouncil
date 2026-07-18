"""Legacy artifact knowledge graph — deprecated compat shim.

devcouncil: allow-unwired — deprecated compat shim; real graph is indexing/graph/.

This module holds a minimal Pydantic model for requirements/tasks/files used by
older integration experiments. It is **not** the symbol-level code graph in
``indexing/graph/`` (built by ``dev map``).

Do not extend this module for dead-code analysis, call-graph queries, or new
features — use ``indexing/graph/`` instead. Kept only so imports remain stable.
"""

from typing import Any, Dict, List

from pydantic import BaseModel, Field
from pathlib import Path


class GraphNode(BaseModel):
    id: str
    type: str  # "file", "symbol", "requirement", "task"
    metadata: Dict[str, Any] = Field(default_factory=dict)


class GraphEdge(BaseModel):
    source: str
    target: str
    relation: str  # "imports", "implements", "validates", "contains"


class KnowledgeGraph(BaseModel):
    nodes: List[GraphNode] = []
    edges: List[GraphEdge] = []


class GraphIndex:
    def __init__(self, project_root: Path):
        self.project_root = project_root
        self.graph = KnowledgeGraph()
