"""Central configuration for the semantic layer."""

from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum
from typing import Literal


class ModelTier(str, Enum):
    SMALL = "small"  # 1B-3B parameters
    LARGE = "large"  # 8B-70B parameters


@dataclass(frozen=True)
class EmbeddingConfig:
    model_name: str = "sentence-transformers/all-MiniLM-L6-v2"
    dimension: int = 384
    device: Literal["cpu", "cuda"] = "cpu"  # CPU avoids VRAM contention with LLM
    batch_size: int = 32
    normalize: bool = True


@dataclass
class CacheConfig:
    backend: Literal["faiss", "chroma"] = "faiss"
    similarity_threshold: float = 0.92
    ood_threshold: float = 0.75
    margin_threshold: float = 0.03
    ttl_seconds: int = 3600
    max_entries: int = 10_000
    namespace: str = "default"
    exploration_rate: float = 0.02  # force miss for FPR calibration


@dataclass
class RouterConfig:
    complexity_threshold: float = 0.45
    small_model: str = "qwen2.5:1.5b"
    large_model: str = "llama3.1:8b"
    weights: dict[str, float] = field(
        default_factory=lambda: {
            "length": 0.25,
            "structure": 0.30,
            "embed_disp": 0.25,
            "domain": 0.20,
        }
    )


@dataclass
class CompressorConfig:
    token_budget: int = 2048
    top_k: int = 8
    chunk_token_size: int = 256
    chunk_overlap: int = 32
    min_chunk_score: float = 0.35
    mmr_lambda: float = 0.7


@dataclass
class LLMConfig:
    backend: Literal["ollama", "llama_cpp", "hf"] = "ollama"
    base_url: str = "http://localhost:11434"
    timeout_seconds: float = 120.0


@dataclass
class SemanticLayerConfig:
    embedding: EmbeddingConfig = field(default_factory=EmbeddingConfig)
    cache: CacheConfig = field(default_factory=CacheConfig)
    router: RouterConfig = field(default_factory=RouterConfig)
    compressor: CompressorConfig = field(default_factory=CompressorConfig)
    llm: LLMConfig = field(default_factory=LLMConfig)
    latency_budget_ms: float = 15.0
