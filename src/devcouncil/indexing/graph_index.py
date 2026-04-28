from typing import List, Dict, Any, Set
from pydantic import BaseModel, Field
from pathlib import Path

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

    def build_initial_graph(self, files: List[str]):
        """
        Bootstrap the graph from file list.
        """
        for f in files:
            self.graph.nodes.append(GraphNode(
                id=f,
                type="file",
                metadata={"extension": Path(f).suffix}
            ))

    def add_relation(self, source: str, target: str, relation: str):
        self.graph.edges.append(GraphEdge(source=source, target=target, relation=relation))

    def get_context_for_file(self, file_path: str) -> Set[str]:
        """
        Retrieve related paths for a given file.
        """
        related = {file_path}
        for edge in self.graph.edges:
            if edge.source == file_path:
                related.add(edge.target)
            if edge.target == file_path:
                related.add(edge.source)
        return related
