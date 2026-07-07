"""Semantic layer for local LLM pipelines — cache, routing, and RAG compression."""

from __future__ import annotations

from .benchmark import benchmark_cache_path, run_benchmark
from .cache import CacheEntry, CacheLookupResult, SemanticCache
from .compressor import CompressedContext, SemanticCompressor
from .config import (
    CacheConfig,
    CompressorConfig,
    EmbeddingConfig,
    LLMConfig,
    ModelTier,
    RouterConfig,
    SemanticLayerConfig,
)
from .embeddings import EmbeddingService
from .llm_backends import (
    HuggingFaceBackend,
    LLMBackend,
    LlamaCppBackend,
    OllamaBackend,
    create_backend,
)
from .pipeline import PipelineResponse, SemanticPipeline
from .router import RouteDecision, SemanticRouter
from .tuner import LookupEvent, ThresholdAutoTuner

__all__ = [
    "CacheConfig",
    "CacheEntry",
    "CacheLookupResult",
    "CompressedContext",
    "CompressorConfig",
    "EmbeddingConfig",
    "EmbeddingService",
    "HuggingFaceBackend",
    "LLMBackend",
    "LLMConfig",
    "LlamaCppBackend",
    "LookupEvent",
    "ModelTier",
    "OllamaBackend",
    "PipelineResponse",
    "RouteDecision",
    "RouterConfig",
    "SemanticCache",
    "SemanticCompressor",
    "SemanticLayerConfig",
    "SemanticPipeline",
    "SemanticRouter",
    "ThresholdAutoTuner",
    "benchmark_cache_path",
    "create_backend",
    "run_benchmark",
]

__version__ = "1.0.0"
