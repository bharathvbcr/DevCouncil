"""Benchmark harness for semantic layer latency and cache hit rate."""

from __future__ import annotations

import statistics
import time
from typing import Sequence

from .config import SemanticLayerConfig
from .pipeline import SemanticPipeline


def benchmark_cache_path(
    pipeline: SemanticPipeline,
    queries: Sequence[str],
    warmup: int = 5,
) -> dict[str, float]:
    """Measure semantic-layer-only latency (no LLM) for cache hits."""
    from .embeddings import EmbeddingService

    embedder = EmbeddingService.get_instance()
    latencies: list[float] = []

    for q in list(queries)[:warmup]:
        embedder.embed_one(q)

    for q in queries:
        t0 = time.perf_counter()
        vec = embedder.embed_one(q)
        pipeline.cache.lookup(q, vec)
        latencies.append((time.perf_counter() - t0) * 1000)

    if not latencies:
        return {
            "p50_ms": 0.0,
            "p95_ms": 0.0,
            "p99_ms": 0.0,
            "mean_ms": 0.0,
            "hit_rate": pipeline.cache.hit_rate,
        }

    latencies.sort()
    last = len(latencies) - 1
    return {
        "p50_ms": latencies[len(latencies) // 2],
        "p95_ms": latencies[int(last * 0.95)],
        "p99_ms": latencies[int(last * 0.99)],
        "mean_ms": statistics.mean(latencies),
        "hit_rate": pipeline.cache.hit_rate,
    }


def run_benchmark(queries: list[str], documents: list[str] | None = None) -> None:
    config = SemanticLayerConfig()
    pipeline = SemanticPipeline(config)

    # Prime cache
    for q in queries[:10]:
        pipeline.run(q, documents)

    stats = benchmark_cache_path(pipeline, queries)
    print("Semantic layer cache-path benchmark:")
    for k, v in stats.items():
        print(f"  {k}: {v:.2f}" if isinstance(v, float) else f"  {k}: {v}")


if __name__ == "__main__":
    sample_queries = [
        "What is Python?",
        "Explain Python programming language",
        "Define Python language",
        "How do I write a for loop in Python?",
        "Implement a distributed cache with consistent hashing",
    ] * 20
    run_benchmark(sample_queries)
