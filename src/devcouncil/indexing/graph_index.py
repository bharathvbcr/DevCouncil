from typing import List, Dict, Any
from pydantic import BaseModel, Field
from pathlib import Path

# NOTE (Phase 4 legacy audit): artifact knowledge graph (requirements/tasks/files
# for GitNexus sync), not the code symbol graph in ``indexing/graph/``. Keep as a
# thin compat shim — do not extend for dead-code / call-graph analysis.

class GraphNode(BaseModel):
    id: str
    type: str # "file", "symbol", "requirement", "task"
    metadata: Dict[str, Any] = Field(default_factory=dict)

class GraphEdge(BaseModel):
    source: str
    target: str
    relation: str # "imports", "implements", "validates", "contains"

class KnowledgeGraph(BaseModel):
    nodes: List[GraphNode] = []
    edges: List[GraphEdge] = []

class GraphIndex:
    def __init__(self, project_root: Path):
        self.project_root = project_root
        self.graph = KnowledgeGraph()
