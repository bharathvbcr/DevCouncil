"""Semantic RAG context compression with MMR diversification."""

from __future__ import annotations

from dataclasses import dataclass

import numpy as np
from numpy.typing import NDArray

from .config import CompressorConfig
from .embeddings import EmbeddingService, FloatVector


@dataclass
class CompressedContext:
    text: str
    chunks_used: int
    chunks_total: int
    estimated_tokens: int
    scores: list[float]


class SemanticCompressor:
    def __init__(
        self,
        config: CompressorConfig | None = None,
        embedder: EmbeddingService | None = None,
    ) -> None:
        self.config = config or CompressorConfig()
        self.embedder = embedder or EmbeddingService.get_instance()

    @staticmethod
    def _estimate_tokens(text: str) -> int:
        # Rough heuristic: ~4 chars per token for English
        return max(1, len(text) // 4)

    def chunk_document(self, document: str) -> list[str]:
        """Split document into overlapping word-based chunks."""
        words = document.split()
        chunk_words = self.config.chunk_token_size  # treating as word proxy
        overlap = self.config.chunk_overlap
        if not words:
            return []

        chunks: list[str] = []
        start = 0
        while start < len(words):
            end = min(len(words), start + chunk_words)
            chunks.append(" ".join(words[start:end]))
            if end >= len(words):
                break
            start = max(start + 1, end - overlap)
        return chunks

    def _mmr_select(
        self,
        query_vec: FloatVector,
        chunk_vecs: NDArray[np.float32],
        chunk_texts: list[str],
        token_budget: int,
    ) -> tuple[list[int], list[float]]:
        """Maximal Marginal Relevance selection under token budget."""
        n = len(chunk_texts)
        if n == 0:
            return [], []

        relevance = chunk_vecs @ query_vec
        selected: list[int] = []
        scores: list[float] = []
        tokens_used = 0
        lam = self.config.mmr_lambda

        candidate_mask = relevance >= self.config.min_chunk_score
        candidates = [i for i in range(n) if candidate_mask[i]]

        while candidates and len(selected) < self.config.top_k:
            best_idx = -1
            best_mmr = -float("inf")

            for i in candidates:
                chunk_tokens = self._estimate_tokens(chunk_texts[i])
                if tokens_used + chunk_tokens > token_budget and selected:
                    continue

                rel = float(relevance[i])
                redundancy = 0.0
                if selected:
                    redundancy = max(
                        float(chunk_vecs[i] @ chunk_vecs[j]) for j in selected
                    )
                mmr = lam * rel - (1.0 - lam) * redundancy
                if mmr > best_mmr:
                    best_mmr = mmr
                    best_idx = i

            if best_idx < 0:
                break

            selected.append(best_idx)
            scores.append(float(relevance[best_idx]))
            tokens_used += self._estimate_tokens(chunk_texts[best_idx])
            candidates.remove(best_idx)

            if tokens_used >= token_budget:
                break

        return selected, scores

    def compress(
        self,
        query: str,
        documents: list[str],
        query_embedding: FloatVector | None = None,
        token_budget: int | None = None,
    ) -> CompressedContext:
        budget = token_budget or self.config.token_budget
        all_chunks: list[str] = []
        for doc in documents:
            all_chunks.extend(self.chunk_document(doc))

        if not all_chunks:
            return CompressedContext(text="", chunks_used=0, chunks_total=0, estimated_tokens=0, scores=[])

        q_vec = query_embedding if query_embedding is not None else self.embedder.embed_one(query)
        chunk_vecs = self.embedder.embed(all_chunks)

        selected_indices, scores = self._mmr_select(q_vec, chunk_vecs, all_chunks, budget)
        selected_chunks = [all_chunks[i] for i in selected_indices]
        combined = "\n\n---\n\n".join(selected_chunks)

        return CompressedContext(
            text=combined,
            chunks_used=len(selected_chunks),
            chunks_total=len(all_chunks),
            estimated_tokens=self._estimate_tokens(combined),
            scores=scores,
        )
