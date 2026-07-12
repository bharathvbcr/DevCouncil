"""Code-graph OKF + attributed GraphML export."""

from __future__ import annotations

import subprocess

from devcouncil.indexing.graph.build import build_code_graph, write_code_graph
from devcouncil.indexing.graph.export import (
    build_code_graph_okf,
    export_graphml,
    file_doc_rel,
    write_code_graph_okf,
)
from devcouncil.knowledge.okf import read_bundle, validate_bundle


def _git(root, *args):
    subprocess.run(["git", *args], cwd=root, check=True, capture_output=True, text=True)


def _commit(root):
    _git(root, "init")
    _git(root, "-c", "user.email=t@t", "-c", "user.name=t", "add", "-A")
    _git(root, "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", "init")


def _write(tmp_path, files):
    for rel, content in files.items():
        p = tmp_path / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(content, encoding="utf-8")


def _tiny_graph(tmp_path):
    _write(
        tmp_path,
        {
            "pyproject.toml": (
                '[project]\nname = "t"\nversion = "0"\n'
                '[project.scripts]\ncli = "pkg.main:main"\n'
            ),
            "pkg/__init__.py": "",
            "pkg/main.py": "from pkg import util\n\ndef main():\n    return util.run()\n",
            "pkg/util.py": "def run():\n    return 1\n",
        },
    )
    _commit(tmp_path)
    return build_code_graph(tmp_path, liveness=False)


def test_file_doc_rel_convention():
    assert file_doc_rel("src/foo.py") == "files/src/foo.py.md"
    assert file_doc_rel("./pkg/a.py") == "files/pkg/a.py.md"


def test_export_graphml_has_attributes(tmp_path):
    graph = _tiny_graph(tmp_path)
    xml = export_graphml(graph)
    assert 'attr.name="kind"' in xml
    assert 'attr.name="confidence"' in xml
    assert 'attr.name="community"' in xml
    assert 'attr.name="dead"' in xml
    assert "<data key=\"kind\">" in xml
    assert "<data key=\"ekind\">" in xml


def test_code_graph_okf_bundle_valid(tmp_path):
    graph = _tiny_graph(tmp_path)
    write_code_graph(tmp_path, graph)
    out = tmp_path / "okf-out"
    written_dir, paths = write_code_graph_okf(tmp_path, out, graph=graph, project_name="demo")
    assert written_dir == out
    assert paths
    bundle = read_bundle(out)
    by_path = bundle.by_path()
    assert "index.md" in by_path
    assert by_path["index.md"].type == "Code Graph"
    # File pages exist
    assert file_doc_rel("pkg/main.py") in by_path
    assert by_path[file_doc_rel("pkg/main.py")].type == "Code File"
    # Frontmatter invariants
    assert validate_bundle(bundle) == []
    # Import links use shared file_doc_rel convention
    main_body = by_path[file_doc_rel("pkg/main.py")].body
    assert file_doc_rel("pkg/util.py") in main_body or "pkg/util.py" in main_body


def test_build_code_graph_okf_has_subsystem_pages(tmp_path):
    graph = _tiny_graph(tmp_path)
    bundle = build_code_graph_okf(graph, project_name="demo", timestamp="2026-01-01T00:00:00Z")
    types = {d.type for d in bundle.documents}
    assert "Code Subsystem" in types
    assert "Code File" in types
    for doc in bundle.documents:
        assert doc.type
        assert doc.timestamp == "2026-01-01T00:00:00Z"
