from devcouncil.indexing.viz import sample_demo_graph, write_graph_demo


def test_sample_demo_graph_html(tmp_path):
    html_path = write_graph_demo(tmp_path, open_browser=False)
    assert html_path.name == "demo.html"
    assert html_path.exists()
    content = html_path.read_text(encoding="utf-8")
    assert "ForceGraph" in content
    assert list(html_path.parent.glob("*.svg")) == []


def test_sample_demo_graph_has_nodes_and_links():
    graph = sample_demo_graph()
    assert graph["nodes"]
    assert graph["links"]
