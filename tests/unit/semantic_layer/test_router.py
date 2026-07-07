"""Tests for SemanticRouter complexity scoring."""

from __future__ import annotations

from semantic_layer.config import ModelTier, RouterConfig
from semantic_layer.router import SemanticRouter

from .conftest import MockEmbedder, SIMPLE_ANCHORS


def test_simple_query_routes_to_small_model():
    embedder = MockEmbedder()
    router = SemanticRouter(RouterConfig(complexity_threshold=0.45), embedder=embedder)
    router._anchor_embeddings = embedder.embed(SIMPLE_ANCHORS)

    decision = router.route("hello there", embedder.embed_one("hello there"))

    assert decision.tier == ModelTier.SMALL
    assert decision.model_name == router.config.small_model
    assert decision.complexity_score < 0.45


def test_complex_query_routes_to_large_model():
    embedder = MockEmbedder()
    router = SemanticRouter(RouterConfig(complexity_threshold=0.45), embedder=embedder)
    router._anchor_embeddings = embedder.embed(SIMPLE_ANCHORS)

    query = "Implement a distributed cache with consistent hashing and benchmark it"
    decision = router.route(query, embedder.embed_one(query), requires_tools=True)

    assert decision.tier == ModelTier.LARGE
    assert decision.model_name == router.config.large_model
    assert decision.complexity_score >= 0.45


def test_complexity_is_weighted_sum_not_broken_loop():
    embedder = MockEmbedder()
    config = RouterConfig(
        complexity_threshold=0.99,
        weights={"length": 0.25, "structure": 0.30, "embed_disp": 0.25, "domain": 0.20},
    )
    router = SemanticRouter(config, embedder=embedder)
    router._anchor_embeddings = embedder.embed(SIMPLE_ANCHORS)

    query = "hello"
    vec = embedder.embed_one(query)
    decision = router.route(query, vec)

    expected = (
        0.25 * decision.features["length"]
        + 0.30 * decision.features["structure"]
        + 0.25 * decision.features["embed_disp"]
        + 0.20 * decision.features["domain"]
    )
    assert abs(decision.complexity_score - expected) < 1e-6


def test_code_query_intent():
    embedder = MockEmbedder()
    router = SemanticRouter(embedder=embedder)
    router._anchor_embeddings = embedder.embed(SIMPLE_ANCHORS)

    query = "def foo():\n    pass"
    decision = router.route(query, embedder.embed_one(query))

    assert decision.intent == "code"
