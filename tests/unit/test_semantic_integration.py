"""Integration tests for semantic_layer wiring in DevCouncil's ModelRouter."""

from __future__ import annotations

import asyncio
import json
from unittest.mock import AsyncMock, MagicMock, patch

import pytest
import yaml
from pydantic import BaseModel

from devcouncil.app.config import SemanticLayerConfig
from devcouncil.llm.provider import LLMResponse, Provider
from devcouncil.llm.router import ModelRouter
from devcouncil.llm.semantic_bridge import (
    SemanticLayerAdapter,
    check_semantic_layer,
    load_semantic_adapter,
    reset_semantic_adapters_for_tests,
    semantic_deps_available,
)


class DemoOutput(BaseModel):
    answer: str


class CountingProvider(Provider):
    def __init__(self, content: str = '{"answer": "live"}'):
        self.calls = 0
        self.content = content

    async def complete(self, model, messages, temperature=0.0, json_mode=False, run_id=None, **kwargs):
        self.calls += 1
        return LLMResponse(
            content=self.content,
            model=model,
            usage={"prompt_tokens": 5, "completion_tokens": 5, "total_tokens": 10},
            raw_response={},
        )


def _mock_semantic_adapter(**overrides):
    adapter = MagicMock(spec=SemanticLayerAdapter)
    adapter.maybe_compress_messages_async = AsyncMock(side_effect=lambda msgs: msgs)
    adapter.maybe_route_model_async = AsyncMock(
        side_effect=lambda msgs, *, configured_model, role_provider: configured_model
    )
    adapter.lookup_cache_async = AsyncMock(return_value=None)
    adapter.store_cache_async = AsyncMock()
    adapter.warm_up = MagicMock(return_value=True)
    for key, value in overrides.items():
        setattr(adapter, key, value)
    return adapter


def _write_config(tmp_path, semantic_enabled: bool = True) -> None:
    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    config = {
        "models": {"provider": "openrouter", "roles": {"critic_a": {"model": "test/model"}}},
        "semantic_layer": {
            "enabled": semantic_enabled,
            "cache": {"enabled": True, "similarity_threshold": 0.5},
            "router": {"enabled": False},
            "compressor": {"enabled": False},
        },
    }
    (dev_dir / "config.yaml").write_text(yaml.dump(config), encoding="utf-8")


@pytest.fixture(autouse=True)
def _reset_semantic_singleton():
    reset_semantic_adapters_for_tests()
    yield
    reset_semantic_adapters_for_tests()


def test_load_semantic_adapter_disabled(tmp_path):
    _write_config(tmp_path, semantic_enabled=False)
    assert load_semantic_adapter(tmp_path) is None


def test_load_semantic_adapter_enabled(tmp_path):
    _write_config(tmp_path, semantic_enabled=True)
    adapter = load_semantic_adapter(tmp_path)
    assert adapter is not None
    assert adapter.settings.enabled is True
    assert load_semantic_adapter(tmp_path) is adapter


def test_semantic_cache_hit_skips_provider(tmp_path):
    provider = CountingProvider()
    adapter = _mock_semantic_adapter(
        lookup_cache_async=AsyncMock(
            return_value=LLMResponse(
                content='{"answer": "cached"}',
                model="test/model",
                usage={"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0},
                raw_response={"semantic_cache": True},
            )
        ),
    )

    router = ModelRouter(
        provider,
        {"critic_a": {"model": "test/model"}},
        project_root=tmp_path,
        semantic_adapter=adapter,
    )

    result = asyncio.run(
        router.complete_structured(
            role="critic_a",
            messages=[{"role": "user", "content": "hello"}],
            schema=DemoOutput,
        )
    )

    assert result.answer == "cached"
    assert provider.calls == 0
    adapter.lookup_cache_async.assert_awaited_once()
    adapter.store_cache_async.assert_not_awaited()


def test_semantic_store_after_live_call(tmp_path):
    provider = CountingProvider()
    adapter = _mock_semantic_adapter()

    router = ModelRouter(
        provider,
        {"critic_a": {"model": "test/model"}},
        project_root=tmp_path,
        semantic_adapter=adapter,
    )

    result = asyncio.run(
        router.complete_structured(
            role="critic_a",
            messages=[{"role": "user", "content": "unique prompt"}],
            schema=DemoOutput,
        )
    )

    assert result.answer == "live"
    assert provider.calls == 1
    adapter.store_cache_async.assert_awaited_once()


@patch("devcouncil.llm.semantic_bridge.semantic_deps_available", return_value=False)
def test_semantic_adapter_graceful_without_deps(_mock_deps, tmp_path):
    settings = SemanticLayerConfig(enabled=True)
    adapter = SemanticLayerAdapter(settings, tmp_path)
    assert adapter._ensure_initialized() is False
    assert adapter.lookup_cache([{"role": "user", "content": "x"}], model="m", role="r") is None


def test_config_schema_defaults():
    cfg = SemanticLayerConfig()
    assert cfg.enabled is False
    assert cfg.cache.enabled is True
    assert cfg.router.enabled is False
    assert cfg.compressor.enabled is True


def test_check_semantic_layer_disabled(tmp_path):
    _write_config(tmp_path, semantic_enabled=False)
    rows = check_semantic_layer(tmp_path)
    assert any("Semantic layer" in row[0] for row in rows)
    assert any("Disabled" in row[2] for row in rows)


@patch("devcouncil.llm.semantic_bridge.semantic_deps_available", return_value=False)
def test_check_semantic_layer_missing_deps(_mock_deps, tmp_path):
    _write_config(tmp_path, semantic_enabled=True)
    rows = check_semantic_layer(tmp_path)
    assert any(row[0] == "Semantic deps" for row in rows)


def test_semantic_cache_persist_roundtrip(tmp_path):
    import importlib.util
    from pathlib import Path

    conftest_path = Path(__file__).resolve().parent / "semantic_layer" / "conftest.py"
    spec = importlib.util.spec_from_file_location("semantic_layer_test_conftest", conftest_path)
    assert spec and spec.loader
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    MockEmbedder = mod.MockEmbedder

    settings = SemanticLayerConfig(
        enabled=True,
        cache={"enabled": True, "similarity_threshold": 0.99, "exploration_rate": 0.0, "namespace": "test"},
        router={"enabled": False},
        compressor={"enabled": False},
    )
    adapter = SemanticLayerAdapter(settings, tmp_path)

    with patch.object(adapter, "_ensure_initialized", return_value=True):
        adapter._embedder = MockEmbedder()
        from semantic_layer.cache import SemanticCache
        from semantic_layer.config import CacheConfig

        adapter._cache = SemanticCache(
            CacheConfig(similarity_threshold=0.99, exploration_rate=0.0, namespace="test"),
            adapter._embedder,
            384,
        )
        adapter._router = None
        adapter._compressor = None
        adapter._init_failed = False
        adapter._initialized = True

        messages = [{"role": "user", "content": "persist me"}]
        response = LLMResponse(
            content='{"answer": "stored"}',
            model="test/model",
            usage={"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
            raw_response={},
        )
        adapter.store_cache(messages, response, model="test/model", role="critic_a")
        adapter.persist_cache()

    adapter2 = SemanticLayerAdapter(settings, tmp_path)
    with patch.object(adapter2, "_ensure_initialized", return_value=True):
        adapter2._embedder = MockEmbedder()
        from semantic_layer.cache import SemanticCache
        from semantic_layer.config import CacheConfig

        adapter2._cache = SemanticCache(
            CacheConfig(similarity_threshold=0.99, exploration_rate=0.0, namespace="test"),
            adapter2._embedder,
            384,
        )
        adapter2._load_persisted_cache()
        hit = adapter2.lookup_cache(messages, model="test/model", role="critic_a")
        assert hit is not None
        assert json.loads(hit.content)["answer"] == "stored"


@pytest.mark.skipif(not semantic_deps_available(), reason="faiss/numpy not installed")
def test_semantic_adapter_with_mock_embedder(tmp_path):
    import importlib.util
    from pathlib import Path

    conftest_path = Path(__file__).resolve().parent / "semantic_layer" / "conftest.py"
    spec = importlib.util.spec_from_file_location("semantic_layer_test_conftest", conftest_path)
    assert spec and spec.loader
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    MockEmbedder = mod.MockEmbedder

    settings = SemanticLayerConfig(
        enabled=True,
        cache={"enabled": True, "similarity_threshold": 0.99, "exploration_rate": 0.0},
        router={"enabled": False},
        compressor={"enabled": False},
    )
    adapter = SemanticLayerAdapter(settings, tmp_path)

    with patch.object(adapter, "_ensure_initialized", return_value=True):
        adapter._embedder = MockEmbedder()
        from semantic_layer.cache import SemanticCache
        from semantic_layer.config import CacheConfig

        adapter._cache = SemanticCache(
            CacheConfig(similarity_threshold=0.99, exploration_rate=0.0),
            adapter._embedder,
            384,
        )
        adapter._router = None
        adapter._compressor = None
        adapter._init_failed = False
        adapter._initialized = True

        messages = [{"role": "user", "content": "What is DevCouncil?"}]
        response = LLMResponse(
            content='{"answer": "orchestrator"}',
            model="test/model",
            usage={"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
            raw_response={},
        )
        adapter.store_cache(messages, response, model="test/model", role="critic_a")
        hit = adapter.lookup_cache(messages, model="test/model", role="critic_a")
        assert hit is not None
        assert json.loads(hit.content)["answer"] == "orchestrator"


def test_lookup_cache_async_uses_thread_pool(tmp_path):
    adapter = SemanticLayerAdapter(SemanticLayerConfig(enabled=True), tmp_path)
    embedder = MagicMock()
    embedder.embed_one.return_value = [0.1, 0.2, 0.3]
    cache = MagicMock()
    cache.lookup.return_value = MagicMock(hit=False, response=None)

    with patch.object(adapter, "_ensure_initialized", return_value=True):
        adapter._embedder = embedder
        adapter._cache = cache
        adapter._router = None
        adapter._init_failed = False
        adapter._initialized = True
        adapter.settings.cache.enabled = True

        result = asyncio.run(
            adapter.lookup_cache_async(
                [{"role": "user", "content": "hello"}],
                model="m",
                role="r",
            )
        )

    assert result is None
    embedder.embed_one.assert_called_once_with("user:hello")
