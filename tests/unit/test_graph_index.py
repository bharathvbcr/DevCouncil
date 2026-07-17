from devcouncil.indexing.graph_index import GraphIndex


def test_build_initial_graph_creates_file_nodes_with_extensions(tmp_path):
    index = GraphIndex(tmp_path)

    index.build_initial_graph(["src/app.py", "README", "web/App.tsx"])

    assert [node.id for node in index.graph.nodes] == ["src/app.py", "README", "web/App.tsx"]
    assert [node.type for node in index.graph.nodes] == ["file", "file", "file"]
    assert [node.metadata["extension"] for node in index.graph.nodes] == [".py", "", ".tsx"]


def test_add_relation_records_graph_edge(tmp_path):
    index = GraphIndex(tmp_path)

    index.add_relation("src/app.py", "src/routes.py", "imports")

    assert len(index.graph.edges) == 1
    edge = index.graph.edges[0]
    assert edge.source == "src/app.py"
    assert edge.target == "src/routes.py"
    assert edge.relation == "imports"


def test_get_context_for_file_includes_outgoing_and_incoming_neighbors(tmp_path):
    index = GraphIndex(tmp_path)
    index.add_relation("src/app.py", "src/routes.py", "imports")
    index.add_relation("tests/test_app.py", "src/app.py", "validates")
    index.add_relation("src/unrelated.py", "src/other.py", "imports")

    context = index.get_context_for_file("src/app.py")

    assert context == {"src/app.py", "src/routes.py", "tests/test_app.py"}


def test_get_context_for_file_returns_file_itself_without_edges(tmp_path):
    index = GraphIndex(tmp_path)

    assert index.get_context_for_file("src/lonely.py") == {"src/lonely.py"}
