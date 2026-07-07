"""Tests for SemanticCache — no live LLM or GPU required."""

from __future__ import annotations

import time

import numpy as np

from semantic_layer.cache import SemanticCache
from semantic_layer.config import CacheConfig

from .conftest import MockEmbedder


def test_exact_hash_hit():
    embedder = MockEmbedder()
    cache = SemanticCache(CacheConfig(max_entries=100), embedder=embedder)
    vec = embedder.embed_one("What is Python?")
    cache.put("What is Python?", "Python is a language.", vec, intent="qa")

    result = cache.lookup("What is Python?", vec, intent="qa")

    assert result.hit is True
    assert result.reason == "exact_hash"
    assert result.response == "Python is a language."
    assert result.similarity == 1.0


def test_semantic_hit_above_threshold():
    embedder = MockEmbedder()
    config = CacheConfig(similarity_threshold=0.5, ood_threshold=0.1, margin_threshold=0.01)
    cache = SemanticCache(config, embedder=embedder)
    base = embedder.embed_one("What is Python?")
    cache.put("What is Python?", "Python is a language.", base, intent="qa")

    # Near-duplicate embedding (different query text, same semantic neighborhood)
    noisy = base + np.random.default_rng(0).standard_normal(base.shape).astype(np.float32) * 0.001
    noisy = noisy / np.linalg.norm(noisy)

    result = cache.lookup("Explain Python language", noisy, intent="qa")

    assert result.hit is True
    assert result.reason == "semantic_hit"
    assert result.similarity >= config.similarity_threshold


def test_miss_below_threshold():
    embedder = MockEmbedder()
    config = CacheConfig(similarity_threshold=0.99, ood_threshold=0.1)
    cache = SemanticCache(config, embedder=embedder)
    cache.put("query a", "response a", embedder.embed_one("query a"), intent="general")

    result = cache.lookup("totally different topic xyz", embedder.embed_one("totally different topic xyz"))

    assert result.hit is False
    assert result.reason in {"below_threshold", "ood", "empty_index"}


def test_intent_mismatch_blocks_hit():
    embedder = MockEmbedder()
    config = CacheConfig(similarity_threshold=0.5, ood_threshold=0.1, margin_threshold=0.01)
    cache = SemanticCache(config, embedder=embedder)
    vec = embedder.embed_one("implement sort")
    cache.put("implement sort", "here is code", vec, intent="code")

    result = cache.lookup("implement sort", vec, intent="qa")

    assert result.hit is False
    assert result.reason == "intent_mismatch"


def test_expired_entry_not_returned():
    embedder = MockEmbedder()
    config = CacheConfig(ttl_seconds=1, max_entries=100)
    cache = SemanticCache(config, embedder=embedder)
    vec = embedder.embed_one("hello")
    entry_id = cache.put("hello", "hi there", vec)
    cache._entries[entry_id].created_at = time.time() - 10

    result = cache.lookup("hello", vec)

    assert result.hit is False


def test_lru_eviction_at_capacity():
    embedder = MockEmbedder()
    config = CacheConfig(max_entries=2, similarity_threshold=0.5, ood_threshold=0.0)
    cache = SemanticCache(config, embedder=embedder)
    cache.put("one", "r1", embedder.embed_one("one"))
    cache.put("two", "r2", embedder.embed_one("two"))
    cache.put("three", "r3", embedder.embed_one("three"))

    assert len(cache._entries) == 2
    assert cache.stats()["evictions"] == 1
