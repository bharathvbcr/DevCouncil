"""Tests for SemanticCompressor MMR selection."""

from __future__ import annotations

from semantic_layer.compressor import SemanticCompressor
from semantic_layer.config import CompressorConfig

from .conftest import MockEmbedder


def test_chunk_document_overlap():
    compressor = SemanticCompressor(
        CompressorConfig(chunk_token_size=5, chunk_overlap=2),
        embedder=MockEmbedder(),
    )
    doc = " ".join(f"w{i}" for i in range(12))
    chunks = compressor.chunk_document(doc)

    assert len(chunks) >= 2
    assert all(chunks[i] != chunks[i + 1] for i in range(len(chunks) - 1))


def test_compress_respects_token_budget():
    embedder = MockEmbedder()
    compressor = SemanticCompressor(
        CompressorConfig(
            token_budget=50,
            top_k=10,
            chunk_token_size=20,
            chunk_overlap=0,
            min_chunk_score=-1.0,
        ),
        embedder=embedder,
    )
    docs = [" ".join(f"word{i}" for i in range(100)) for _ in range(3)]

    result = compressor.compress("find relevant info", docs)

    assert result.chunks_used >= 1
    assert result.estimated_tokens <= 50 or result.chunks_used == 1


def test_compress_empty_documents():
    compressor = SemanticCompressor(embedder=MockEmbedder())
    result = compressor.compress("query", [])

    assert result.text == ""
    assert result.chunks_used == 0
    assert result.chunks_total == 0
