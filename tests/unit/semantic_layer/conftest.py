"""Shared test fixtures for semantic_layer unit tests."""

from __future__ import annotations

import hashlib

import numpy as np
from numpy.typing import NDArray

FloatVector = NDArray[np.float32]
FloatMatrix = NDArray[np.float32]

SIMPLE_ANCHORS = [
    "hello",
    "what time is it",
    "define photosynthesis",
    "translate hello to french",
    "what is 2 plus 2",
]


class MockEmbedder:
    """Deterministic L2-normalized vectors — no sentence-transformers required."""

    def __init__(self, dimension: int = 384) -> None:
        self.config = type("Cfg", (), {"dimension": dimension})()
        self.dimension = dimension

    def _vector_for_text(self, text: str) -> FloatVector:
        digest = hashlib.sha256(text.encode()).digest()
        rng = np.random.default_rng(int.from_bytes(digest[:8], "big"))
        vec = rng.standard_normal(self.dimension).astype(np.float32)
        norm = np.linalg.norm(vec)
        if norm == 0:
            vec[0] = 1.0
            return vec
        return vec / norm

    def embed_one(self, text: str) -> FloatVector:
        return self._vector_for_text(text)

    def embed(self, texts: list[str]) -> FloatMatrix:
        return np.stack([self._vector_for_text(t) for t in texts])


class MockLLMBackend:
    """In-memory LLM stub for pipeline tests."""

    def __init__(self, response: str = "mock response") -> None:
        self.response = response
        self.calls: list[tuple[str, str, str | None]] = []

    def generate(self, model: str, prompt: str, system: str | None = None) -> str:
        self.calls.append((model, prompt, system))
        return self.response

    def close(self) -> None:
        return None
