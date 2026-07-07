"""Orchestrator: semantic cache → router → compressor → LLM."""

from __future__ import annotations

import random
import time
from dataclasses import dataclass, field
from typing import Any

from .cache import SemanticCache
from .compressor import SemanticCompressor
from .config import SemanticLayerConfig
from .embeddings import EmbeddingService
from .llm_backends import LLMBackend, create_backend
from .router import SemanticRouter
from .tuner import ThresholdAutoTuner


@dataclass
class PipelineResponse:
    text: str
    cache_hit: bool
    model_used: str | None
    latency_ms: dict[str, float] = field(default_factory=dict)
    metadata: dict[str, Any] = field(default_factory=dict)


class SemanticPipeline:
    def __init__(
        self,
        config: SemanticLayerConfig | None = None,
        llm: LLMBackend | None = None,
    ) -> None:
        self.config = config or SemanticLayerConfig()
        self.embedder = EmbeddingService.get_instance(self.config.embedding)
        self.cache = SemanticCache(self.config.cache, self.embedder, self.config.embedding.dimension)
        self.router = SemanticRouter(self.config.router, self.embedder)
        self.compressor = SemanticCompressor(self.config.compressor, self.embedder)
        self.llm: LLMBackend = llm or create_backend(self.config.llm)
        self.tuner = ThresholdAutoTuner(self.config.cache)

    def run(
        self,
        query: str,
        documents: list[str] | None = None,
        system_prompt: str | None = None,
        requires_tools: bool = False,
    ) -> PipelineResponse:
        timings: dict[str, float] = {}
        t0 = time.perf_counter()

        # Single embed pass — shared across all components
        t_embed = time.perf_counter()
        query_vec = self.embedder.embed_one(query)
        timings["embed_ms"] = (time.perf_counter() - t_embed) * 1000

        has_rag = bool(documents)
        route = self.router.route(query, query_vec, has_rag=has_rag, requires_tools=requires_tools)

        # Exploration: force miss for calibration
        force_miss = random.random() < self.config.cache.exploration_rate

        t_cache = time.perf_counter()
        cache_result = self.cache.lookup(query, query_vec, intent=route.intent, force_miss=force_miss)
        timings["cache_ms"] = (time.perf_counter() - t_cache) * 1000

        if cache_result.hit and cache_result.response is not None:
            timings["total_ms"] = (time.perf_counter() - t0) * 1000
            self.tuner.record(cache_result.similarity, was_hit=True)
            return PipelineResponse(
                text=cache_result.response,
                cache_hit=True,
                model_used=None,
                latency_ms=timings,
                metadata={
                    "similarity": cache_result.similarity,
                    "reason": cache_result.reason,
                    "tier": route.tier.value,
                },
            )

        # RAG compression
        ctx = ""
        if documents:
            t_comp = time.perf_counter()
            compressed = self.compressor.compress(query, documents, query_vec)
            ctx = compressed.text
            timings["compress_ms"] = (time.perf_counter() - t_comp) * 1000
            timings["chunks_used"] = compressed.chunks_used

        prompt = query if not ctx else f"Context:\n{ctx}\n\nQuestion: {query}"

        t_llm = time.perf_counter()
        response = self.llm.generate(route.model_name, prompt, system=system_prompt)
        timings["llm_ms"] = (time.perf_counter() - t_llm) * 1000

        self.cache.put(query, response, query_vec, intent=route.intent)
        self.tuner.record(cache_result.similarity, was_hit=False)

        timings["total_ms"] = (time.perf_counter() - t0) * 1000
        return PipelineResponse(
            text=response,
            cache_hit=False,
            model_used=route.model_name,
            latency_ms=timings,
            metadata={
                "tier": route.tier.value,
                "complexity": route.complexity_score,
                "features": route.features,
            },
        )
