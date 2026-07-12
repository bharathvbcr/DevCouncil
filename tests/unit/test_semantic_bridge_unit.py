"""Unit coverage for the semantic-layer adapter facade.

Exercises ``devcouncil.llm.semantic_bridge`` without the optional faiss/sentence-
transformers runtime by injecting mock embedder/cache/router/compressor objects and
patching ``_ensure_initialized``. Also covers the pure helpers, graceful-degradation
branches, and the ``check_semantic_layer`` doctor rows.
"""

from __future__ import annotations

import asyncio
from types import SimpleNamespace
from unittest.mock import MagicMock, patch

import pytest

from devcouncil.app.config import SemanticLayerConfig
from devcouncil.llm.provider import LLMResponse
from devcouncil.llm import semantic_bridge as sb
from devcouncil.llm.semantic_bridge import (
    SemanticLayerAdapter,
    check_semantic_layer,
    load_semantic_adapter,
    prompt_text_from_messages,
    reset_semantic_adapters_for_tests,
)


@pytest.fixture(autouse=True)
def _reset():
    reset_semantic_adapters_for_tests()
    yield
    reset_semantic_adapters_for_tests()


def _enabled_settings(**cache_over):
    cache = {"enabled": True, "exploration_rate": 0.0, "namespace": "test", **cache_over}
    return SemanticLayerConfig(
        enabled=True,
        cache=cache,
        router={"enabled": True},
        compressor={"enabled": True, "min_chars": 10},
    )


def _initialized(adapter):
    """Force an adapter into a ready state with mock components."""
    adapter._init_failed = False
    adapter._initialized = True


# ---- pure helpers -------------------------------------------------------------

def test_prompt_text_from_messages_joins_nonempty_roles():
    msgs = [
        {"role": "system", "content": "  sys  "},
        {"role": "user", "content": "hello"},
        {"role": "assistant", "content": ""},
        {"content": "no role"},
    ]
    assert prompt_text_from_messages(msgs) == "system:sys\nuser:hello\nuser:no role"


def test_prompt_text_from_messages_empty():
    assert prompt_text_from_messages([]) == ""


# ---- adapter properties -------------------------------------------------------

def test_adapter_properties_reflect_settings(tmp_path):
    adapter = SemanticLayerAdapter(_enabled_settings(), tmp_path)
    assert adapter.active is True
    assert adapter.cache_enabled is True
    assert adapter.router_enabled is True
    assert adapter.compressor_enabled is True
    assert adapter.cache_dir() == tmp_path / ".devcouncil" / "cache" / "semantic"
    assert adapter.cache_base_path().name == "test"


def test_adapter_inactive_when_init_failed(tmp_path):
    adapter = SemanticLayerAdapter(_enabled_settings(), tmp_path)
    adapter._init_failed = True
    assert adapter.active is False
    assert adapter.cache_enabled is False
    assert adapter.router_enabled is False
    assert adapter.compressor_enabled is False


# ---- _ensure_initialized degradation -----------------------------------------

def test_ensure_initialized_disabled(tmp_path):
    adapter = SemanticLayerAdapter(SemanticLayerConfig(enabled=False), tmp_path)
    assert adapter._ensure_initialized() is False
    assert adapter._init_failed is True
    # Second call short-circuits on the cached failure.
    assert adapter._ensure_initialized() is False


@patch("devcouncil.llm.semantic_bridge.semantic_deps_available", return_value=False)
def test_ensure_initialized_missing_deps(_deps, tmp_path):
    adapter = SemanticLayerAdapter(SemanticLayerConfig(enabled=True), tmp_path)
    assert adapter._ensure_initialized() is False
    assert adapter._init_failed is True


# ---- lookup / store cache -----------------------------------------------------

def test_lookup_cache_disabled_returns_none(tmp_path):
    adapter = SemanticLayerAdapter(SemanticLayerConfig(enabled=False), tmp_path)
    assert adapter.lookup_cache([{"role": "user", "content": "x"}], model="m", role="r") is None


def test_lookup_cache_hit(tmp_path):
    adapter = SemanticLayerAdapter(_enabled_settings(), tmp_path)
    _initialized(adapter)
    adapter._embedder = MagicMock()
    adapter._embedder.embed_one.return_value = [0.1, 0.2]
    adapter._router = None
    adapter._cache = MagicMock()
    adapter._cache.lookup.return_value = SimpleNamespace(
        hit=True, response='{"answer": "hi"}', similarity=0.99, entry_id="e1"
    )
    with patch.object(adapter, "_ensure_initialized", return_value=True):
        out = adapter.lookup_cache([{"role": "user", "content": "hello"}], model="m", role="critic")
    assert out is not None
    assert out.content == '{"answer": "hi"}'
    assert out.raw_response["semantic_cache"] is True


def test_lookup_cache_miss(tmp_path):
    adapter = SemanticLayerAdapter(_enabled_settings(), tmp_path)
    _initialized(adapter)
    adapter._embedder = MagicMock()
    adapter._embedder.embed_one.return_value = [0.1]
    adapter._router = None
    adapter._cache = MagicMock()
    adapter._cache.lookup.return_value = SimpleNamespace(hit=False, response=None, similarity=0.0, entry_id=None)
    with patch.object(adapter, "_ensure_initialized", return_value=True):
        assert adapter.lookup_cache([{"role": "user", "content": "hi"}], model="m", role="r") is None


def test_lookup_cache_blank_prompt_returns_none(tmp_path):
    adapter = SemanticLayerAdapter(_enabled_settings(), tmp_path)
    with patch.object(adapter, "_ensure_initialized", return_value=True):
        adapter._embedder = MagicMock()
        adapter._cache = MagicMock()
        assert adapter.lookup_cache([{"role": "user", "content": "   "}], model="m", role="r") is None


def test_lookup_cache_swallows_embedder_error(tmp_path):
    adapter = SemanticLayerAdapter(_enabled_settings(), tmp_path)
    with patch.object(adapter, "_ensure_initialized", return_value=True):
        adapter._embedder = MagicMock()
        adapter._embedder.embed_one.side_effect = RuntimeError("boom")
        adapter._cache = MagicMock()
        assert adapter.lookup_cache([{"role": "user", "content": "hi"}], model="m", role="r") is None


def test_store_cache_stores(tmp_path):
    adapter = SemanticLayerAdapter(_enabled_settings(), tmp_path)
    adapter._embedder = MagicMock()
    adapter._embedder.embed_one.return_value = [0.5]
    adapter._router = None
    adapter._cache = MagicMock()
    resp = LLMResponse(content='{"answer": "y"}', model="m", usage={}, raw_response={})
    with patch.object(adapter, "_ensure_initialized", return_value=True):
        adapter.store_cache([{"role": "user", "content": "hi"}], resp, model="m", role="r")
    adapter._cache.put.assert_called_once()


def test_store_cache_skips_empty_response(tmp_path):
    adapter = SemanticLayerAdapter(_enabled_settings(), tmp_path)
    adapter._cache = MagicMock()
    adapter._embedder = MagicMock()
    resp = LLMResponse(content="   ", model="m", usage={}, raw_response={})
    with patch.object(adapter, "_ensure_initialized", return_value=True):
        adapter.store_cache([{"role": "user", "content": "hi"}], resp, model="m", role="r")
    adapter._cache.put.assert_not_called()


# ---- intent / route -----------------------------------------------------------

def test_intent_without_router(tmp_path):
    adapter = SemanticLayerAdapter(_enabled_settings(), tmp_path)
    adapter._router = None
    assert adapter._intent_for_prompt("p", [0.1], model="gpt", role="critic") == "critic:gpt"


def test_intent_with_router(tmp_path):
    adapter = SemanticLayerAdapter(_enabled_settings(), tmp_path)
    adapter._router = MagicMock()
    adapter._router.route.return_value = SimpleNamespace(intent="code")
    assert adapter._intent_for_prompt("p", [0.1], model="gpt", role="critic") == "critic:gpt:code"


def test_route_with_vector(tmp_path):
    adapter = SemanticLayerAdapter(_enabled_settings(), tmp_path)
    adapter._router = MagicMock()
    adapter._router.route.return_value = SimpleNamespace(
        complexity_score=0.9, tier=SimpleNamespace(value="large"), model_name="big-model"
    )
    assert adapter._route_with_vector("p", [0.1], configured_model="cfg") == "big-model"


# ---- async wrappers -----------------------------------------------------------

def test_lookup_cache_async(tmp_path):
    adapter = SemanticLayerAdapter(_enabled_settings(), tmp_path)
    adapter._embedder = MagicMock()
    adapter._embedder.embed_one.return_value = [0.1]
    adapter._router = None
    adapter._cache = MagicMock()
    adapter._cache.lookup.return_value = SimpleNamespace(hit=False, response=None, similarity=0, entry_id=None)
    with patch.object(adapter, "_ensure_initialized", return_value=True):
        out = asyncio.run(adapter.lookup_cache_async([{"role": "user", "content": "hi"}], model="m", role="r"))
    assert out is None


def test_maybe_route_model_async_non_ollama_returns_configured(tmp_path):
    adapter = SemanticLayerAdapter(_enabled_settings(), tmp_path)
    adapter._router = MagicMock()
    adapter._embedder = MagicMock()
    with patch.object(adapter, "_ensure_initialized", return_value=True):
        out = asyncio.run(
            adapter.maybe_route_model_async(
                [{"role": "user", "content": "hi"}], configured_model="cfg", role_provider="openrouter"
            )
        )
    assert out == "cfg"


def test_maybe_route_model_async_ollama_routes(tmp_path):
    adapter = SemanticLayerAdapter(_enabled_settings(), tmp_path)
    adapter._embedder = MagicMock()
    adapter._embedder.embed_one.return_value = [0.1]
    adapter._router = MagicMock()
    adapter._router.route.return_value = SimpleNamespace(
        complexity_score=0.2, tier=SimpleNamespace(value="small"), model_name="small-model"
    )
    with patch.object(adapter, "_ensure_initialized", return_value=True):
        out = asyncio.run(
            adapter.maybe_route_model_async(
                [{"role": "user", "content": "hi"}], configured_model="cfg", role_provider="ollama"
            )
        )
    assert out == "small-model"


def test_maybe_route_model_async_disabled_returns_configured(tmp_path):
    adapter = SemanticLayerAdapter(SemanticLayerConfig(enabled=True, router={"enabled": False}), tmp_path)
    out = asyncio.run(
        adapter.maybe_route_model_async(
            [{"role": "user", "content": "hi"}], configured_model="cfg", role_provider="ollama"
        )
    )
    assert out == "cfg"


# ---- compression --------------------------------------------------------------

def test_compression_inputs_below_min_chars_returns_none(tmp_path):
    adapter = SemanticLayerAdapter(_enabled_settings(), tmp_path)
    assert adapter._compression_inputs([{"role": "user", "content": "short"}]) is None


def test_compression_inputs_with_context_docs(tmp_path):
    settings = SemanticLayerConfig(enabled=True, compressor={"enabled": True, "min_chars": 10})
    adapter = SemanticLayerAdapter(settings, tmp_path)
    messages = [
        {"role": "system", "content": "context document one is fairly long"},
        {"role": "user", "content": "the actual question here"},
    ]
    result = adapter._compression_inputs(messages)
    assert result is not None
    query, docs = result
    assert query == "the actual question here"
    assert docs == ["context document one is fairly long"]


def test_compression_inputs_single_message_split(tmp_path):
    settings = SemanticLayerConfig(enabled=True, compressor={"enabled": True, "min_chars": 10})
    adapter = SemanticLayerAdapter(settings, tmp_path)
    blob = "x" * 100
    result = adapter._compression_inputs([{"role": "user", "content": blob}])
    assert result is not None
    query, docs = result
    assert docs and query


def test_maybe_compress_messages_async_applies(tmp_path):
    settings = SemanticLayerConfig(enabled=True, compressor={"enabled": True, "min_chars": 10})
    adapter = SemanticLayerAdapter(settings, tmp_path)
    adapter._embedder = MagicMock()
    adapter._embedder.embed_one.return_value = [0.1]
    adapter._compressor = MagicMock()
    adapter._compressor.compress.return_value = SimpleNamespace(
        text="compressed context", chunks_total=5, chunks_used=2, estimated_tokens=42
    )
    messages = [
        {"role": "system", "content": "some big context document here"},
        {"role": "user", "content": "answer this question please"},
    ]
    with patch.object(adapter, "_ensure_initialized", return_value=True):
        out = asyncio.run(adapter.maybe_compress_messages_async(messages))
    joined = " ".join(m["content"] for m in out)
    assert "compressed context" in joined


def test_maybe_compress_messages_async_disabled_passthrough(tmp_path):
    settings = SemanticLayerConfig(enabled=True, compressor={"enabled": False})
    adapter = SemanticLayerAdapter(settings, tmp_path)
    messages = [{"role": "user", "content": "hi"}]
    out = asyncio.run(adapter.maybe_compress_messages_async(messages))
    assert out == messages


# ---- persistence --------------------------------------------------------------

def test_persist_cache_disabled_noop(tmp_path):
    adapter = SemanticLayerAdapter(SemanticLayerConfig(enabled=False), tmp_path)
    adapter._cache = MagicMock()
    adapter.persist_cache()
    adapter._cache.persist.assert_not_called()


def test_persist_cache_writes(tmp_path):
    adapter = SemanticLayerAdapter(_enabled_settings(), tmp_path)
    adapter._cache = MagicMock()
    adapter._cache._entries = []
    with patch.object(adapter, "_ensure_initialized", return_value=True):
        # cache_enabled reads settings, already enabled
        adapter.persist_cache()
    adapter._cache.persist.assert_called_once()


def test_load_persisted_cache_missing_files_noop(tmp_path):
    adapter = SemanticLayerAdapter(_enabled_settings(), tmp_path)
    adapter._cache = MagicMock()
    adapter._load_persisted_cache()
    adapter._cache.load.assert_not_called()


def test_load_persisted_cache_reads_when_present(tmp_path):
    adapter = SemanticLayerAdapter(_enabled_settings(), tmp_path)
    base = adapter.cache_base_path()
    base.parent.mkdir(parents=True, exist_ok=True)
    (base.parent / f"{base.name}.faiss").write_text("x", encoding="utf-8")
    (base.parent / f"{base.name}.json").write_text("{}", encoding="utf-8")
    adapter._cache = MagicMock()
    adapter._cache._entries = [1, 2]
    adapter._load_persisted_cache()
    adapter._cache.load.assert_called_once()


# ---- warm_up ------------------------------------------------------------------

def test_warm_up_disabled(tmp_path):
    adapter = SemanticLayerAdapter(SemanticLayerConfig(enabled=False), tmp_path)
    assert adapter.warm_up() is False


def test_warm_up_success(tmp_path):
    adapter = SemanticLayerAdapter(_enabled_settings(), tmp_path)
    adapter._embedder = MagicMock()
    with patch.object(adapter, "_ensure_initialized", return_value=True):
        assert adapter.warm_up() is True
    adapter._embedder.embed_one.assert_called_once_with("warmup")


def test_warm_up_embedder_error_returns_false(tmp_path):
    adapter = SemanticLayerAdapter(_enabled_settings(), tmp_path)
    adapter._embedder = MagicMock()
    adapter._embedder.embed_one.side_effect = RuntimeError("no model")
    with patch.object(adapter, "_ensure_initialized", return_value=True):
        assert adapter.warm_up() is False


# ---- module-level helpers -----------------------------------------------------

def test_persist_all_adapters_swallows_errors(tmp_path):
    adapter = SemanticLayerAdapter(_enabled_settings(), tmp_path)
    adapter.persist_cache = MagicMock(side_effect=RuntimeError("fail"))
    sb._ADAPTER_BY_ROOT[tmp_path] = adapter
    # Must not raise.
    sb._persist_all_adapters()


def test_register_shutdown_once_idempotent():
    sb._SHUTDOWN_REGISTERED = False
    sb._register_shutdown_once()
    assert sb._SHUTDOWN_REGISTERED is True
    sb._register_shutdown_once()  # no-op second call


def test_load_semantic_adapter_no_config_returns_none(tmp_path):
    # No .devcouncil/config.yaml -> load_config raises FileNotFoundError -> None.
    assert load_semantic_adapter(tmp_path) is None


@patch("devcouncil.llm.semantic_bridge.semantic_deps_available", return_value=True)
@patch("devcouncil.llm.semantic_bridge.semantic_embedding_deps_available", return_value=False)
def test_check_semantic_layer_enabled_missing_embedder(_emb, _deps, tmp_path):
    settings = SemanticLayerConfig(enabled=True)
    cfg = SimpleNamespace(semantic_layer=settings)
    with patch.object(SemanticLayerAdapter, "warm_up", return_value=False):
        rows = check_semantic_layer(tmp_path, config=cfg)
    labels = [r[0] for r in rows]
    assert "Semantic layer" in labels
    assert "Semantic embedder" in labels
    assert "Semantic warm-up" in labels


# ---- dependency probes --------------------------------------------------------

def test_semantic_deps_available_true():
    assert sb.semantic_deps_available() is True  # faiss + numpy in dev deps


def test_semantic_deps_available_false_when_import_fails():
    import sys as _sys

    with patch.dict(_sys.modules, {"faiss": None}):
        assert sb.semantic_deps_available() is False


def test_semantic_embedding_deps_available_branches():
    import sys as _sys

    with patch.dict(_sys.modules, {"sentence_transformers": None}):
        assert sb.semantic_embedding_deps_available() is False


# ---- _ensure_initialized real init path ---------------------------------------

@patch("devcouncil.llm.semantic_bridge.semantic_deps_available", return_value=True)
def test_ensure_initialized_success(_deps, tmp_path):
    adapter = SemanticLayerAdapter(_enabled_settings(), tmp_path)
    fake_embedder = MagicMock()
    with patch("semantic_layer.embeddings.EmbeddingService.get_instance", return_value=fake_embedder), \
         patch("semantic_layer.cache.SemanticCache", return_value=MagicMock(_entries=[])), \
         patch("semantic_layer.router.SemanticRouter", return_value=MagicMock()), \
         patch("semantic_layer.compressor.SemanticCompressor", return_value=MagicMock()):
        assert adapter._ensure_initialized() is True
    assert adapter._embedder is fake_embedder
    assert adapter._init_failed is False


@patch("devcouncil.llm.semantic_bridge.semantic_deps_available", return_value=True)
def test_ensure_initialized_exception_disables(_deps, tmp_path):
    adapter = SemanticLayerAdapter(_enabled_settings(), tmp_path)
    with patch("semantic_layer.embeddings.EmbeddingService.get_instance", side_effect=RuntimeError("bad")):
        assert adapter._ensure_initialized() is False
    assert adapter._init_failed is True


# ---- _load_persisted_cache / persist edge cases -------------------------------

def test_load_persisted_cache_none_cache(tmp_path):
    adapter = SemanticLayerAdapter(_enabled_settings(), tmp_path)
    adapter._cache = None
    adapter._load_persisted_cache()  # returns early, no error


def test_load_persisted_cache_swallows_load_error(tmp_path):
    adapter = SemanticLayerAdapter(_enabled_settings(), tmp_path)
    base = adapter.cache_base_path()
    base.parent.mkdir(parents=True, exist_ok=True)
    (base.parent / f"{base.name}.faiss").write_text("x", encoding="utf-8")
    (base.parent / f"{base.name}.json").write_text("{}", encoding="utf-8")
    adapter._cache = MagicMock()
    adapter._cache.load.side_effect = RuntimeError("corrupt")
    adapter._load_persisted_cache()  # exception swallowed


def test_persist_cache_swallows_error(tmp_path):
    adapter = SemanticLayerAdapter(_enabled_settings(), tmp_path)
    adapter._cache = MagicMock()
    adapter._cache.persist.side_effect = RuntimeError("io")
    with patch.object(adapter, "_ensure_initialized", return_value=True):
        adapter.persist_cache()  # swallowed


# ---- warm_up not initialized --------------------------------------------------

def test_warm_up_not_initialized(tmp_path):
    adapter = SemanticLayerAdapter(_enabled_settings(), tmp_path)
    with patch.object(adapter, "_ensure_initialized", return_value=False):
        assert adapter.warm_up() is False


# ---- async lookup / store edge cases ------------------------------------------

def test_lookup_cache_async_blank_prompt(tmp_path):
    adapter = SemanticLayerAdapter(_enabled_settings(), tmp_path)
    adapter._embedder = MagicMock()
    adapter._cache = MagicMock()
    with patch.object(adapter, "_ensure_initialized", return_value=True):
        out = asyncio.run(adapter.lookup_cache_async([{"role": "user", "content": " "}], model="m", role="r"))
    assert out is None


def test_lookup_cache_async_swallows_error(tmp_path):
    adapter = SemanticLayerAdapter(_enabled_settings(), tmp_path)
    adapter._embedder = MagicMock()
    adapter._cache = MagicMock()
    with patch.object(adapter, "_ensure_initialized", return_value=True), \
         patch.object(adapter, "_embed_one_async", side_effect=RuntimeError("boom")):
        out = asyncio.run(adapter.lookup_cache_async([{"role": "user", "content": "hi"}], model="m", role="r"))
    assert out is None


def test_store_cache_disabled_noop(tmp_path):
    adapter = SemanticLayerAdapter(SemanticLayerConfig(enabled=False), tmp_path)
    adapter._cache = MagicMock()
    adapter.store_cache([{"role": "user", "content": "hi"}], LLMResponse(content="x", model="m", usage={}, raw_response={}), model="m", role="r")
    adapter._cache.put.assert_not_called()


def test_store_cache_swallows_error(tmp_path):
    adapter = SemanticLayerAdapter(_enabled_settings(), tmp_path)
    adapter._embedder = MagicMock()
    adapter._embedder.embed_one.side_effect = RuntimeError("boom")
    adapter._router = None
    adapter._cache = MagicMock()
    resp = LLMResponse(content="body", model="m", usage={}, raw_response={})
    with patch.object(adapter, "_ensure_initialized", return_value=True):
        adapter.store_cache([{"role": "user", "content": "hi"}], resp, model="m", role="r")
    adapter._cache.put.assert_not_called()


def test_store_cache_async_stores_and_skips(tmp_path):
    adapter = SemanticLayerAdapter(_enabled_settings(), tmp_path)
    adapter._embedder = MagicMock()
    adapter._embedder.embed_one.return_value = [0.1]
    adapter._router = None
    adapter._cache = MagicMock()
    resp = LLMResponse(content='{"a": 1}', model="m", usage={}, raw_response={})
    with patch.object(adapter, "_ensure_initialized", return_value=True):
        asyncio.run(adapter.store_cache_async([{"role": "user", "content": "hi"}], resp, model="m", role="r"))
    adapter._cache.put.assert_called_once()
    # Blank response is skipped.
    adapter._cache.reset_mock()
    empty = LLMResponse(content="  ", model="m", usage={}, raw_response={})
    with patch.object(adapter, "_ensure_initialized", return_value=True):
        asyncio.run(adapter.store_cache_async([{"role": "user", "content": "hi"}], empty, model="m", role="r"))
    adapter._cache.put.assert_not_called()


def test_store_cache_async_disabled_noop(tmp_path):
    adapter = SemanticLayerAdapter(SemanticLayerConfig(enabled=False), tmp_path)
    adapter._cache = MagicMock()
    resp = LLMResponse(content="x", model="m", usage={}, raw_response={})
    asyncio.run(adapter.store_cache_async([{"role": "user", "content": "hi"}], resp, model="m", role="r"))
    adapter._cache.put.assert_not_called()


def test_store_cache_async_swallows_error(tmp_path):
    adapter = SemanticLayerAdapter(_enabled_settings(), tmp_path)
    adapter._embedder = MagicMock()
    adapter._router = None
    adapter._cache = MagicMock()
    resp = LLMResponse(content="body", model="m", usage={}, raw_response={})
    with patch.object(adapter, "_ensure_initialized", return_value=True), \
         patch.object(adapter, "_embed_one_async", side_effect=RuntimeError("boom")):
        asyncio.run(adapter.store_cache_async([{"role": "user", "content": "hi"}], resp, model="m", role="r"))
    adapter._cache.put.assert_not_called()


# ---- routing / compression edge cases -----------------------------------------

def test_maybe_route_model_async_blank_prompt(tmp_path):
    adapter = SemanticLayerAdapter(_enabled_settings(), tmp_path)
    adapter._router = MagicMock()
    adapter._embedder = MagicMock()
    with patch.object(adapter, "_ensure_initialized", return_value=True):
        out = asyncio.run(
            adapter.maybe_route_model_async(
                [{"role": "user", "content": "   "}], configured_model="cfg", role_provider="ollama"
            )
        )
    assert out == "cfg"


def test_maybe_route_model_async_swallows_error(tmp_path):
    adapter = SemanticLayerAdapter(_enabled_settings(), tmp_path)
    adapter._router = MagicMock()
    adapter._embedder = MagicMock()
    with patch.object(adapter, "_ensure_initialized", return_value=True), \
         patch.object(adapter, "_embed_one_async", side_effect=RuntimeError("boom")):
        out = asyncio.run(
            adapter.maybe_route_model_async(
                [{"role": "user", "content": "hi"}], configured_model="cfg", role_provider="ollama"
            )
        )
    assert out == "cfg"


def test_maybe_compress_prepared_none_passthrough(tmp_path):
    adapter = SemanticLayerAdapter(SemanticLayerConfig(enabled=True, compressor={"enabled": True, "min_chars": 10}), tmp_path)
    adapter._embedder = MagicMock()
    adapter._compressor = MagicMock()
    messages = [{"role": "user", "content": "tiny"}]
    with patch.object(adapter, "_ensure_initialized", return_value=True):
        out = asyncio.run(adapter.maybe_compress_messages_async(messages))
    assert out == messages


def test_maybe_compress_swallows_error(tmp_path):
    adapter = SemanticLayerAdapter(SemanticLayerConfig(enabled=True, compressor={"enabled": True, "min_chars": 10}), tmp_path)
    adapter._embedder = MagicMock()
    adapter._compressor = MagicMock()
    messages = [
        {"role": "system", "content": "some big context document here"},
        {"role": "user", "content": "answer this question please"},
    ]
    with patch.object(adapter, "_ensure_initialized", return_value=True), \
         patch.object(adapter, "_embed_one_async", side_effect=RuntimeError("boom")):
        out = asyncio.run(adapter.maybe_compress_messages_async(messages))
    assert out == messages


def test_compression_inputs_no_user_messages(tmp_path):
    adapter = SemanticLayerAdapter(SemanticLayerConfig(enabled=True, compressor={"enabled": True, "min_chars": 5}), tmp_path)
    messages = [{"role": "system", "content": "a long system-only message here"}]
    assert adapter._compression_inputs(messages) is None


def test_compression_inputs_single_short_blob_returns_none(tmp_path):
    adapter = SemanticLayerAdapter(SemanticLayerConfig(enabled=True, compressor={"enabled": True, "min_chars": 6}), tmp_path)
    # Total chars >= min_chars (passes first gate) but the single blob is <= min_chars,
    # so the context-split guard returns None.
    result = adapter._compression_inputs([{"role": "user", "content": "abcdef"}])
    assert result is None


def test_compress_with_vector_empty_text_returns_original(tmp_path):
    adapter = SemanticLayerAdapter(_enabled_settings(), tmp_path)
    adapter._compressor = MagicMock()
    adapter._compressor.compress.return_value = SimpleNamespace(text="   ")
    messages = [{"role": "user", "content": "q"}]
    assert adapter._compress_with_vector(messages, "q", ["ctx"], [0.1]) == messages


def test_compress_with_vector_no_user_message_appends(tmp_path):
    adapter = SemanticLayerAdapter(_enabled_settings(), tmp_path)
    adapter._compressor = MagicMock()
    adapter._compressor.compress.return_value = SimpleNamespace(
        text="compressed", chunks_total=2, chunks_used=1, estimated_tokens=10
    )
    # No user message present -> replaced stays False -> compressed appended.
    messages = [{"role": "system", "content": "sys"}]
    out = adapter._compress_with_vector(messages, "q", ["ctx"], [0.1])
    assert any("compressed" in m["content"] for m in out)


# ---- check_semantic_layer branches --------------------------------------------

def test_check_semantic_layer_config_unavailable(tmp_path):
    with patch("devcouncil.app.config.load_config", side_effect=RuntimeError("no cfg")):
        rows = check_semantic_layer(tmp_path, config=None)
    assert rows[0][0] == "Semantic layer"
    assert "unavailable" in rows[0][2].lower()


def test_check_semantic_layer_disabled(tmp_path):
    cfg = SimpleNamespace(semantic_layer=SemanticLayerConfig(enabled=False))
    rows = check_semantic_layer(tmp_path, config=cfg)
    assert "Disabled" in rows[0][2]


@patch("devcouncil.llm.semantic_bridge.semantic_deps_available", return_value=False)
def test_check_semantic_layer_missing_deps(_deps, tmp_path):
    cfg = SimpleNamespace(semantic_layer=SemanticLayerConfig(enabled=True))
    rows = check_semantic_layer(tmp_path, config=cfg)
    assert rows[-1][0] == "Semantic deps"


@patch("devcouncil.llm.semantic_bridge.semantic_deps_available", return_value=True)
@patch("devcouncil.llm.semantic_bridge.semantic_embedding_deps_available", return_value=True)
def test_check_semantic_layer_full_ok(_emb, _deps, tmp_path):
    settings = SemanticLayerConfig(enabled=True)
    # Pre-create a persisted index so the "Persisted index present" detail is hit.
    cache_dir = tmp_path / ".devcouncil" / "cache" / "semantic"
    cache_dir.mkdir(parents=True, exist_ok=True)
    (cache_dir / f"{settings.cache.namespace}.faiss").write_text("x", encoding="utf-8")
    cfg = SimpleNamespace(semantic_layer=settings)
    with patch.object(SemanticLayerAdapter, "warm_up", return_value=True):
        rows = check_semantic_layer(tmp_path, config=cfg)
    labels = {r[0] for r in rows}
    assert "Semantic warm-up" in labels
    detail = next(r[2] for r in rows if r[0] == "Semantic cache dir")
    assert "Persisted index present" in detail


@patch("devcouncil.llm.semantic_bridge.semantic_deps_available", return_value=True)
@patch("devcouncil.llm.semantic_bridge.semantic_embedding_deps_available", return_value=True)
def test_check_semantic_layer_cache_dir_not_writable(_emb, _deps, tmp_path):
    cfg = SimpleNamespace(semantic_layer=SemanticLayerConfig(enabled=True))
    with patch("pathlib.Path.mkdir", side_effect=PermissionError("ro")), \
         patch.object(SemanticLayerAdapter, "warm_up", return_value=False):
        rows = check_semantic_layer(tmp_path, config=cfg)
    detail = next(r[2] for r in rows if r[0] == "Semantic cache dir")
    assert "Not writable" in detail


# ---- load_semantic_adapter ----------------------------------------------------

def test_load_semantic_adapter_success_and_cached(tmp_path):
    cfg = SimpleNamespace(semantic_layer=_enabled_settings())
    with patch("devcouncil.app.config.load_config", return_value=cfg):
        first = load_semantic_adapter(tmp_path)
        second = load_semantic_adapter(tmp_path)
    assert first is not None
    assert first is second  # singleton per resolved root


def test_load_semantic_adapter_disabled_returns_none(tmp_path):
    cfg = SimpleNamespace(semantic_layer=SemanticLayerConfig(enabled=False))
    with patch("devcouncil.app.config.load_config", return_value=cfg):
        assert load_semantic_adapter(tmp_path) is None


def test_load_semantic_adapter_generic_error_returns_none(tmp_path):
    with patch("devcouncil.app.config.load_config", side_effect=RuntimeError("boom")):
        assert load_semantic_adapter(tmp_path) is None
