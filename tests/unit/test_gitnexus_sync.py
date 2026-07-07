import json

from devcouncil.indexing.graph_index import GraphIndex, GraphNode, GraphEdge
from devcouncil.integrations.gitnexus import GitNexusIntegration


def test_sync_graph_writes_artifact_json(tmp_path):
    graph = GraphIndex(tmp_path)
    graph.graph.nodes.append(GraphNode(id="src/foo.py", type="file", metadata={"ext": ".py"}))
    graph.graph.edges.append(GraphEdge(source="src/foo.py", target="src/bar.py", relation="imports"))

    GitNexusIntegration(tmp_path).sync_graph(graph)

    out_path = tmp_path / ".devcouncil" / "nexus" / "artifact_graph.json"
    assert out_path.exists()
    payload = json.loads(out_path.read_text(encoding="utf-8"))
    assert payload["node_count"] == 1
    assert payload["edge_count"] == 1
    assert payload["nodes"][0]["id"] == "src/foo.py"
    assert payload["edges"][0]["relation"] == "imports"
    assert "exported_at" in payload
