"""The Python `index.sqlite` graph store is retired; the kernel's artifact answers.

`load_code_graph` and `write_code_graph` were the store's only production reader
and writer, and both reached zero call sites by Lane M3. What was left was a
store nothing filled and surfaces that still asked it questions -- `dev map
cypher` among them, which answered "No committed graph generation." on every
repository because no code path had persisted a generation since the Python
engine was retired.

Every test here is a *consumer* test: it asserts the surface answers from the
kernel's `code_graph.json`, with no `index.sqlite` on disk at any point.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from tests.unit.graph_fixtures import kernel_graph


@pytest.fixture()
def kernel_corpus(tmp_path: Path) -> Path:
    """A small tree the kernel has indexed, with no Python store anywhere."""
    (tmp_path / "pkg").mkdir()
    (tmp_path / "pkg" / "__init__.py").write_text("", encoding="utf-8")
    (tmp_path / "pkg" / "core.py").write_text(
        "def helper():\n    return 1\n\n\ndef caller():\n    return helper()\n",
        encoding="utf-8",
    )
    (tmp_path / "main.py").write_text(
        "from pkg.core import caller\n\n\ndef main():\n    return caller()\n",
        encoding="utf-8",
    )
    kernel_graph(tmp_path)
    return tmp_path


def _index_sqlite(root: Path) -> Path:
    return root / ".devcouncil" / "codeintel" / "index.sqlite"


def test_cypher_answers_from_the_kernels_artifact(kernel_corpus: Path) -> None:
    """`dev map cypher` used to need a Python store generation nothing wrote.

    Pre-change this returns ``{"ok": False, "error": "No committed graph
    generation."}``: `run_cypher` asked `CodeIntelService`, whose store is empty
    because `write_code_graph` -- its last production writer -- has no callers.
    """
    from devcouncil.indexing.graph.cypher import run_cypher

    result = run_cypher(kernel_corpus, "MATCH (a)-[r:CALLS]->(b) RETURN a,b LIMIT 25")

    assert result["ok"] is True, result
    assert result["count"] >= 1, result
    assert not _index_sqlite(kernel_corpus).exists(), (
        "answering a cypher query created the retired Python store"
    )


def test_cypher_over_nodes_answers_from_the_kernels_artifact(kernel_corpus: Path) -> None:
    from devcouncil.indexing.graph.cypher import run_cypher

    result = run_cypher(
        kernel_corpus, "MATCH (a) WHERE contains(a.name, 'helper') RETURN a LIMIT 25"
    )

    assert result["ok"] is True, result
    assert result["count"] >= 1, result
    assert not _index_sqlite(kernel_corpus).exists()
