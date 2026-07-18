"""Unit coverage for opt-in local graph embeddings (hash fallback)."""

from __future__ import annotations

from types import SimpleNamespace

from devcouncil.indexing.graph import embeddings as emb
from devcouncil.indexing.graph.schema import CodeGraph, GraphNode, NodeKind


def test_tokenize_and_hash_vector_are_deterministic():
    tokens = emb._tokenize("Foo_Bar baz!")
    assert tokens == ["foo_bar", "baz"]
    a = emb._hash_vector(tokens)
    b = emb._hash_vector(tokens)
    assert a == b
    assert abs(sum(v * v for v in a) - 1.0) < 1e-6


def test_cosine_identical_vectors():
    vec = emb._hash_vector(["alpha", "beta"])
    assert emb._cosine(vec, vec) == 1.0


def test_embeddings_disabled_by_default(tmp_path, monkeypatch):
    monkeypatch.setattr(
        emb,
        "load_config",
        lambda root: (_ for _ in ()).throw(RuntimeError("no config")),
        raising=False,
    )
    # Patch at the import site used inside embeddings_enabled.
    import devcouncil.app.config as config_mod

    monkeypatch.setattr(
        config_mod,
        "load_config",
        lambda root: (_ for _ in ()).throw(RuntimeError("no config")),
    )
    assert emb.embeddings_enabled(tmp_path) is False
    assert emb.build_embeddings(tmp_path) == 0
    result = emb.semantic_search(tmp_path, "foo")
    assert result["ok"] is False
    assert result["matches"] == []


def test_build_and_search_when_enabled(tmp_path, monkeypatch):
    monkeypatch.setattr(emb, "embeddings_enabled", lambda root: True)

    graph = CodeGraph(
        nodes=[
            GraphNode(id="a.py::foo", kind=NodeKind.FUNCTION, path="a.py", name="foo"),
            GraphNode(id="b.py::bar", kind=NodeKind.FUNCTION, path="b.py", name="bar"),
        ],
        edges=[],
    )

    class FakeEngine:
        def __init__(self, root):
            pass

        def _graph(self):
            return graph

    monkeypatch.setattr(
        "devcouncil.codeintel.query.engine.CodeIntelQueryEngine",
        FakeEngine,
    )
    # Avoid real config for model name.
    import devcouncil.app.config as config_mod

    monkeypatch.setattr(
        config_mod,
        "load_config",
        lambda root: SimpleNamespace(
            indexing=SimpleNamespace(embeddings=SimpleNamespace(model_name="hash-v1"))
        ),
    )
    # Embeddings are scoped to the committed codeintel generation; without one
    # (no store in tmp_path) build_embeddings correctly refuses to build.
    monkeypatch.setattr(emb, "_current_generation", lambda root: 1)

    count = emb.build_embeddings(tmp_path)
    assert count == 2
    assert emb._index_db(tmp_path).is_file()

    result = emb.semantic_search(tmp_path, "foo function", limit=1)
    assert result["ok"] is True
    assert result["backend"] == "hash-v1"
    assert len(result["matches"]) == 1
    assert result["matches"][0]["label"] in {"foo", "bar"}


def test_build_embeddings_returns_zero_without_graph(tmp_path, monkeypatch):
    monkeypatch.setattr(emb, "embeddings_enabled", lambda root: True)

    class Missing:
        def __init__(self, root):
            pass

        def _graph(self):
            raise FileNotFoundError("no graph")

    monkeypatch.setattr(
        "devcouncil.codeintel.query.engine.CodeIntelQueryEngine",
        Missing,
    )
    assert emb.build_embeddings(tmp_path) == 0


def test_semantic_search_skips_bad_json(tmp_path, monkeypatch):
    monkeypatch.setattr(emb, "embeddings_enabled", lambda root: True)
    emb.ensure_embeddings_schema(tmp_path)
    import sqlite3

    with sqlite3.connect(emb._index_db(tmp_path)) as conn:
        conn.execute(
            f"INSERT INTO {emb._EMBEDDINGS_TABLE} (node_id, path, label, model, vector_json) "
            "VALUES ('bad', 'x.py', 'bad', 'hash-v1', 'not-json')"
        )
        conn.commit()
    result = emb.semantic_search(tmp_path, "anything")
    assert result["ok"] is True
    assert result["matches"] == []


def test_embeddings_enabled_reads_config(tmp_path, monkeypatch):
    import devcouncil.app.config as config_mod

    monkeypatch.setattr(
        config_mod,
        "load_config",
        lambda root: SimpleNamespace(
            indexing=SimpleNamespace(embeddings=SimpleNamespace(enabled=True, model_name="hash-v1"))
        ),
    )
    assert emb.embeddings_enabled(tmp_path) is True


def test_semantic_search_disabled_reason(tmp_path, monkeypatch):
    monkeypatch.setattr(emb, "embeddings_enabled", lambda root: False)
    result = emb.semantic_search(tmp_path, "query")
    assert result == {"ok": False, "reason": "embeddings disabled", "matches": []}


def test_build_and_search_survive_locked_store(tmp_path, monkeypatch):
    """A busy/locked index.sqlite degrades to 0 embeddings / FTS fallback."""
    import sqlite3

    from devcouncil.indexing.graph import embeddings as emb

    monkeypatch.setattr(emb, "embeddings_enabled", lambda root: True)
    monkeypatch.setattr(emb, "ensure_embeddings_schema", lambda root: None)

    class _Graph:
        nodes = [type("N", (), {"id": "a.py::f", "path": "a.py", "name": "f", "kind": "function"})()]

    class _Engine:
        def __init__(self, root):
            pass

        def _graph(self):
            return _Graph()

    monkeypatch.setattr(
        "devcouncil.codeintel.query.engine.CodeIntelQueryEngine", _Engine
    )

    def locked(*args, **kwargs):
        raise sqlite3.OperationalError("database is locked")

    monkeypatch.setattr(emb.sqlite3, "connect", locked)

    assert emb.build_embeddings(tmp_path) == 0
    result = emb.semantic_search(tmp_path, "query")
    assert result["ok"] is False
    assert result["matches"] == []
