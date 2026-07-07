"""Semantic cache with FAISS backend, TTL, LRU eviction, and multi-gate validation."""

from __future__ import annotations

import hashlib
import json
import threading
import time
import uuid
from collections import OrderedDict
from dataclasses import dataclass, field
from typing import Any, Protocol

import faiss
import numpy as np
from numpy.typing import NDArray

from .config import CacheConfig
from .embeddings import EmbeddingService, FloatVector

FloatMatrix = NDArray[np.float32]


class EmbedderProtocol(Protocol):
    def embed_one(self, text: str) -> FloatVector: ...

    def embed(self, texts: list[str]) -> FloatMatrix: ...


@dataclass
class CacheEntry:
    entry_id: str
    query_text: str
    query_hash: str
    response_text: str
    embedding: FloatVector
    namespace: str
    intent: str
    created_at: float
    last_accessed: float
    access_count: int = 0
    metadata: dict[str, Any] = field(default_factory=dict)

    def is_expired(self, ttl_seconds: int, now: float | None = None) -> bool:
        now = now or time.time()
        return (now - self.created_at) > ttl_seconds


@dataclass
class CacheLookupResult:
    hit: bool
    response: str | None = None
    similarity: float = 0.0
    entry_id: str | None = None
    reason: str = "miss"


class SemanticCache:
    """
    FAISS IndexFlatIP + LRU metadata store.
    IndexFlatIP with normalized vectors == cosine similarity.
    """

    def __init__(
        self,
        config: CacheConfig | None = None,
        embedder: EmbedderProtocol | None = None,
        dimension: int = 384,
    ) -> None:
        self.config = config or CacheConfig()
        self.embedder = embedder or EmbeddingService.get_instance()
        self.dimension = dimension
        self._index = faiss.IndexFlatIP(dimension)
        self._entries: OrderedDict[str, CacheEntry] = OrderedDict()
        self._id_to_faiss_row: dict[str, int] = {}
        self._faiss_row_to_id: dict[int, str] = {}
        self._lock = threading.RLock()
        self._stats = {"hits": 0, "misses": 0, "evictions": 0, "false_positive_forced": 0}

    @staticmethod
    def _normalize_query(text: str) -> str:
        return " ".join(text.lower().split())

    @staticmethod
    def _hash_query(text: str) -> str:
        normalized = SemanticCache._normalize_query(text)
        return hashlib.sha256(normalized.encode()).hexdigest()

    def _evict_lru(self) -> None:
        """Remove oldest-accessed entry when at capacity."""
        if len(self._entries) < self.config.max_entries:
            return
        self._entries.popitem(last=False)
        self._stats["evictions"] += 1
        # Note: FAISS IndexFlatIP does not support deletion;
        # stale rows are ignored during search (production: use IndexIDMap2)

    def _search_faiss(
        self, query_vec: FloatVector, k: int = 5
    ) -> list[tuple[str, float]]:
        if self._index.ntotal == 0:
            return []

        q = query_vec.reshape(1, -1).astype(np.float32)
        scores, indices = self._index.search(q, min(k, self._index.ntotal))
        results: list[tuple[str, float]] = []
        now = time.time()

        for score, idx in zip(scores[0], indices[0]):
            if idx < 0:
                continue
            entry_id = self._faiss_row_to_id.get(int(idx))
            if entry_id is None:
                continue
            entry = self._entries.get(entry_id)
            if entry is None:
                continue
            if entry.is_expired(self.config.ttl_seconds, now):
                continue
            if entry.namespace != self.config.namespace:
                continue
            results.append((entry_id, float(score)))
        return results

    def lookup(
        self,
        query: str,
        query_embedding: FloatVector | None = None,
        intent: str | None = None,
        force_miss: bool = False,
    ) -> CacheLookupResult:
        with self._lock:
            query_hash = self._hash_query(query)

            # Exact hash short-circuit
            for entry in self._entries.values():
                if (
                    entry.query_hash == query_hash
                    and entry.namespace == self.config.namespace
                    and not entry.is_expired(self.config.ttl_seconds)
                ):
                    if intent is not None and entry.intent != intent:
                        self._stats["misses"] += 1
                        return CacheLookupResult(hit=False, similarity=1.0, reason="intent_mismatch")
                    entry.last_accessed = time.time()
                    entry.access_count += 1
                    self._entries.move_to_end(entry.entry_id)
                    self._stats["hits"] += 1
                    return CacheLookupResult(
                        hit=True,
                        response=entry.response_text,
                        similarity=1.0,
                        entry_id=entry.entry_id,
                        reason="exact_hash",
                    )

            if force_miss:
                self._stats["misses"] += 1
                return CacheLookupResult(hit=False, reason="forced_exploration")

            vec = query_embedding if query_embedding is not None else self.embedder.embed_one(query)
            candidates = self._search_faiss(vec, k=5)

            if not candidates:
                self._stats["misses"] += 1
                return CacheLookupResult(hit=False, reason="empty_index")

            best_id, best_sim = candidates[0]

            # OOD gate
            if best_sim < self.config.ood_threshold:
                self._stats["misses"] += 1
                return CacheLookupResult(hit=False, similarity=best_sim, reason="ood")

            # Similarity gate
            if best_sim < self.config.similarity_threshold:
                self._stats["misses"] += 1
                return CacheLookupResult(hit=False, similarity=best_sim, reason="below_threshold")

            # Margin gate (ambiguous neighborhood)
            if len(candidates) >= 2:
                second_sim = candidates[1][1]
                if (best_sim - second_sim) < self.config.margin_threshold:
                    self._stats["misses"] += 1
                    return CacheLookupResult(
                        hit=False,
                        similarity=best_sim,
                        reason="insufficient_margin",
                    )

            entry = self._entries[best_id]

            # Intent consistency gate
            if intent is not None and entry.intent != intent:
                self._stats["misses"] += 1
                return CacheLookupResult(hit=False, similarity=best_sim, reason="intent_mismatch")

            entry.last_accessed = time.time()
            entry.access_count += 1
            self._entries.move_to_end(entry.entry_id)
            self._stats["hits"] += 1
            return CacheLookupResult(
                hit=True,
                response=entry.response_text,
                similarity=best_sim,
                entry_id=entry.entry_id,
                reason="semantic_hit",
            )

    def put(
        self,
        query: str,
        response: str,
        query_embedding: FloatVector | None = None,
        intent: str = "general",
        metadata: dict[str, Any] | None = None,
    ) -> str:
        with self._lock:
            self._evict_lru()
            vec = query_embedding if query_embedding is not None else self.embedder.embed_one(query)
            entry_id = str(uuid.uuid4())
            now = time.time()
            entry = CacheEntry(
                entry_id=entry_id,
                query_text=query,
                query_hash=self._hash_query(query),
                response_text=response,
                embedding=vec,
                namespace=self.config.namespace,
                intent=intent,
                created_at=now,
                last_accessed=now,
                metadata=metadata or {},
            )
            row = self._index.ntotal
            self._index.add(vec.reshape(1, -1).astype(np.float32))
            self._id_to_faiss_row[entry_id] = row
            self._faiss_row_to_id[row] = entry_id
            self._entries[entry_id] = entry
            return entry_id

    def invalidate_expired(self) -> int:
        """Remove expired entries from metadata (FAISS rows remain stale)."""
        with self._lock:
            now = time.time()
            expired = [
                eid for eid, e in self._entries.items()
                if e.is_expired(self.config.ttl_seconds, now)
            ]
            for eid in expired:
                del self._entries[eid]
            return len(expired)

    @property
    def hit_rate(self) -> float:
        total = self._stats["hits"] + self._stats["misses"]
        return self._stats["hits"] / total if total else 0.0

    def stats(self) -> dict[str, Any]:
        return {**self._stats, "hit_rate": self.hit_rate, "size": len(self._entries)}

    def persist(self, path: str) -> None:
        with self._lock:
            faiss.write_index(self._index, f"{path}.faiss")
            serializable = {
                eid: {
                    "query_text": e.query_text,
                    "query_hash": e.query_hash,
                    "response_text": e.response_text,
                    "embedding": e.embedding.tolist(),
                    "namespace": e.namespace,
                    "intent": e.intent,
                    "created_at": e.created_at,
                    "last_accessed": e.last_accessed,
                    "access_count": e.access_count,
                    "metadata": e.metadata,
                }
                for eid, e in self._entries.items()
            }
            with open(f"{path}.json", "w", encoding="utf-8") as f:
                json.dump({"entries": serializable, "row_map": self._faiss_row_to_id}, f)

    def load(self, path: str) -> None:
        with self._lock:
            self._index = faiss.read_index(f"{path}.faiss")
            with open(f"{path}.json", encoding="utf-8") as f:
                data = json.load(f)
            self._entries.clear()
            self._faiss_row_to_id = {int(k): v for k, v in data["row_map"].items()}
            self._id_to_faiss_row = {v: int(k) for k, v in self._faiss_row_to_id.items()}
            for eid, raw in data["entries"].items():
                self._entries[eid] = CacheEntry(
                    entry_id=eid,
                    query_text=raw["query_text"],
                    query_hash=raw["query_hash"],
                    response_text=raw["response_text"],
                    embedding=np.array(raw["embedding"], dtype=np.float32),
                    namespace=raw["namespace"],
                    intent=raw["intent"],
                    created_at=raw["created_at"],
                    last_accessed=raw["last_accessed"],
                    access_count=raw["access_count"],
                    metadata=raw.get("metadata", {}),
                )
