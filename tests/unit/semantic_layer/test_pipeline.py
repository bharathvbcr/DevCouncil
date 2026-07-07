"""Tests for SemanticPipeline orchestration with mocked LLM."""

from __future__ import annotations

from semantic_layer.config import CacheConfig, SemanticLayerConfig
from semantic_layer.pipeline import SemanticPipeline

from .conftest import MockEmbedder, MockLLMBackend


def test_pipeline_cache_miss_then_hit():
    embedder = MockEmbedder()
    llm = MockLLMBackend(response="Python is great.")
    config = SemanticLayerConfig(
        cache=CacheConfig(
            exploration_rate=0.0,
            similarity_threshold=0.5,
            ood_threshold=0.0,
            margin_threshold=0.0,
        ),
    )
    pipeline = SemanticPipeline(config, llm=llm)
    pipeline.embedder = embedder
    pipeline.cache.embedder = embedder
    pipeline.router.embedder = embedder
    pipeline.compressor.embedder = embedder

    first = pipeline.run("What is Python?")
    assert first.cache_hit is False
    assert first.text == "Python is great."
    assert len(llm.calls) == 1

    second = pipeline.run("What is Python?")
    assert second.cache_hit is True
    assert second.text == "Python is great."
    assert len(llm.calls) == 1


def test_pipeline_routes_simple_query_to_small_model():
    embedder = MockEmbedder()
    llm = MockLLMBackend()
    config = SemanticLayerConfig(cache=CacheConfig(exploration_rate=0.0))
    pipeline = SemanticPipeline(config, llm=llm)
    pipeline.embedder = embedder
    pipeline.cache.embedder = embedder
    pipeline.router.embedder = embedder
    pipeline.router._anchor_embeddings = embedder.embed(
        [
            "hello",
            "what time is it",
            "define photosynthesis",
            "translate hello to french",
            "what is 2 plus 2",
        ]
    )
    pipeline.compressor.embedder = embedder

    result = pipeline.run("hello")

    assert result.cache_hit is False
    assert result.model_used == config.router.small_model
