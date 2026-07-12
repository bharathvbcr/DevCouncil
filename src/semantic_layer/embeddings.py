"""Shared embedding service — singleton, low-latency, thread-safe."""

from __future__ import annotations

import threading
import time
from typing import Sequence

import numpy as np
from numpy.typing import NDArray

from .config import EmbeddingConfig

FloatVector = NDArray[np.float32]
FloatMatrix = NDArray[np.float32]


class EmbeddingService:
    """Lazy-loaded sentence-transformers embedder with L2 normalization."""

    _instance: EmbeddingService | None = None
    _lock = threading.Lock()

    def __init__(self, config: EmbeddingConfig | None = None) -> None:
        self.config = config or EmbeddingConfig()
        self._model = None
        self._model_lock = threading.Lock()

    @classmethod
    def get_instance(cls, config: EmbeddingConfig | None = None) -> EmbeddingService:
        with cls._lock:
            if cls._instance is None:
                cls._instance = cls(config)
            return cls._instance

    def _load_model(self) -> None:
        if self._model is not None:
            return
        with self._model_lock:
            if self._model is not None:
                return
            from sentence_transformers import SentenceTransformer

            self._model = SentenceTransformer(
                self.config.model_name,
                device=self.config.device,
            )

    def embed(self, texts: Sequence[str]) -> FloatMatrix:
        """Embed a batch of texts. Returns (N, dim) float32 array."""
        self._load_model()
        assert self._model is not None

        t0 = time.perf_counter()
        vectors: FloatMatrix = self._model.encode(
            list(texts),
            batch_size=self.config.batch_size,
            convert_to_numpy=True,
            normalize_embeddings=self.config.normalize,
            show_progress_bar=False,
        ).astype(np.float32)
        elapsed_ms = (time.perf_counter() - t0) * 1000
        if elapsed_ms > 10:
            # Log in production; kept silent here for brevity
            pass
        return vectors

    def embed_one(self, text: str) -> FloatVector:
        return self.embed([text])[0]
