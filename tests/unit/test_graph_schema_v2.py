"""Phase 2 — schema v2 kinds, exported field, implements/overrides, ambiguous candidates, rationale."""

from __future__ import annotations

from pathlib import Path

from devcouncil.indexing.graph.build import assemble_graph, extract_all
from devcouncil.indexing.graph.extract_python import ExtractedSymbol, FileExtraction, extract_python
from devcouncil.indexing.graph.resolve import build_file_and_symbol_nodes, inherit_edges, resolve_calls
from devcouncil.indexing.graph.schema import SCHEMA_VERSION, Confidence, NodeKind


def test_schema_version_and_kinds():
    assert SCHEMA_VERSION == 2
    assert not hasattr(NodeKind, "AREA")
    for name in ("INTERFACE", "TYPE", "STRUCT", "ENUM", "TRAIT", "RATIONALE"):
        assert hasattr(NodeKind, name)


def test_python_rationale_nodes_and_documents_edge(tmp_path: Path):
    pkg = tmp_path / "pkg"
    pkg.mkdir()
    (pkg / "__init__.py").write_text("", encoding="utf-8")
    (pkg / "m.py").write_text(
        "# WHY: entry helper\n"
        "def helper():\n"
        "    # NOTE: core path\n"
        "    return 1\n"
        "def entry():\n"
        "    return helper()\n",
        encoding="utf-8",
    )
    files = ["pkg/__init__.py", "pkg/m.py"]
    extractions = extract_all(tmp_path, files)
    graph = assemble_graph(tmp_path, files, extractions, liveness=False)
    assert graph.schema_version == 2
    rats = [n for n in graph.nodes if n.kind == NodeKind.RATIONALE]
    assert rats
    docs = [e for e in graph.edges if e.kind == "documents"]
    assert docs
    helper = next(n for n in graph.nodes if n.id.endswith("::helper"))
    assert helper.exported is True


def test_implements_and_overrides_edges():
    extractions = {
        "a.py": FileExtraction(
            path="a.py",
            language="python",
            symbols=[
                ExtractedSymbol(
                    kind="class", name="Base", qualname="Base", line=1, end_line=4, exported=True
                ),
                ExtractedSymbol(
                    kind="method", name="run", qualname="Base.run", line=2, end_line=3
                ),
                ExtractedSymbol(
                    kind="class",
                    name="Child",
                    qualname="Child",
                    line=5,
                    end_line=10,
                    bases=["Base"],
                    implements=["IRunnable"],
                    exported=True,
                ),
                ExtractedSymbol(
                    kind="method", name="run", qualname="Child.run", line=6, end_line=8
                ),
                ExtractedSymbol(
                    kind="interface",
                    name="IRunnable",
                    qualname="IRunnable",
                    line=12,
                    end_line=12,
                    exported=True,
                ),
            ],
        )
    }
    nodes, index = build_file_and_symbol_nodes(extractions)
    assert any(n.kind == NodeKind.INTERFACE for n in nodes)
    assert any(n.exported and n.id.endswith("::Child") for n in nodes)
    edges = inherit_edges(extractions, index)
    kinds = {(e.kind, e.source.split("::")[-1], e.target.split("::")[-1]) for e in edges}
    assert ("inherits", "Child", "Base") in kinds
    assert ("implements", "Child", "IRunnable") in kinds
    assert ("overrides", "Child.run", "Base.run") in kinds


def test_ambiguous_call_lists_candidates(tmp_path: Path):
    pkg = tmp_path / "pkg"
    pkg.mkdir()
    (pkg / "__init__.py").write_text("", encoding="utf-8")
    (pkg / "a.py").write_text("def shared():\n    return 1\n", encoding="utf-8")
    (pkg / "b.py").write_text("def shared():\n    return 2\n", encoding="utf-8")
    (pkg / "c.py").write_text("def caller():\n    return shared()\n", encoding="utf-8")
    files = ["pkg/__init__.py", "pkg/a.py", "pkg/b.py", "pkg/c.py"]
    extractions = extract_all(tmp_path, files)
    # Resolve calls directly so we do not need a git repo
    _nodes, index = build_file_and_symbol_nodes(extractions)
    # No import edges between a/b/c — bare shared() is globally ambiguous
    edges = resolve_calls(extractions, index, [])
    ambig = [e for e in edges if e.kind == "calls" and e.confidence == Confidence.AMBIGUOUS]
    assert ambig
    assert any(len(e.extras.get("candidates") or []) >= 2 for e in ambig)


def test_extract_python_rationale():
    ext = extract_python(
        "m.py",
        "# WHY: top\n"
        "def f():\n"
        "    # NOTE: inside\n"
        "    # See ADR-42 for details\n"
        "    return 1\n",
    )
    rats = [s for s in ext.symbols if s.kind == "rationale"]
    assert len(rats) >= 2
    assert any("WHY" in s.name for s in rats)
    assert any("NOTE" in s.name for s in rats)
    assert any("ADR" in s.name for s in rats)
    inner = next(s for s in rats if "NOTE" in s.name)
    assert inner.bases == ["f"]
