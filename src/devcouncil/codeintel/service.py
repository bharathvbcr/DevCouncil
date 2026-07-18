"""Project-scoped lifecycle and compatibility service for code intelligence."""

from __future__ import annotations

import sqlite3
import threading
from collections import OrderedDict
from pathlib import Path
from typing import Any, Callable

from devcouncil.codeintel.store import CodeIntelStore
from devcouncil.indexing.graph.schema import CodeGraph


def canonical_project_root(root: Path) -> Path:
    """Resolve a caller-supplied path to the owning project root.

    Explicit project paths remain authoritative. For a nested path, walk upward
    to the nearest ``.devcouncil`` or ``.git`` marker. This prevents MCP's process
    working directory from silently selecting a different repository.
    """

    resolved = root.expanduser().resolve()
    if resolved.is_file():
        resolved = resolved.parent
    for candidate in (resolved, *resolved.parents):
        if (candidate / ".devcouncil").is_dir() or (candidate / ".git").exists():
            return candidate
    return resolved


class CodeIntelService:
    """Owns one root, one canonical store, and generation-keyed query caching."""

    def __init__(self, project_root: Path):
        self.project_root = canonical_project_root(project_root)
        self.store = CodeIntelStore(self.project_root)
        self._query_cache: OrderedDict[tuple[int, str, str], Any] = OrderedDict()
        self._cache_lock = threading.Lock()
        self._cache_limit = 256

    def persist(
        self,
        graph: CodeGraph,
        *,
        changed_paths: set[str] | None = None,
        analysis_shards: dict[str, dict[str, Any]] | None = None,
    ) -> int:
        try:
            generation = self.store.save_graph(
                graph,
                changed_paths=changed_paths,
                analysis_shards=analysis_shards,
            )
        except sqlite3.DatabaseError as exc:
            if not self.store.quarantine_if_corrupt(exc):
                raise
            # The damaged file is gone; an incremental save against the fresh
            # store would drop every unchanged file, so retry as a full save.
            generation = self.store.save_graph(
                graph,
                changed_paths=None,
                analysis_shards=analysis_shards,
            )
        with self._cache_lock:
            self._query_cache.clear()
        return generation

    def load(self) -> CodeGraph | None:
        return self.store.load_graph()

    def cached_query(self, namespace: str, key: str, loader: Callable[[], Any]) -> Any:
        generation = self.store.current_generation()
        if generation is None:
            return loader()
        cache_key = (generation, namespace, key)
        with self._cache_lock:
            if cache_key in self._query_cache:
                value = self._query_cache.pop(cache_key)
                self._query_cache[cache_key] = value
                return value
        value = loader()
        with self._cache_lock:
            self._query_cache[cache_key] = value
            while len(self._query_cache) > self._cache_limit:
                self._query_cache.popitem(last=False)
        return value

    def status(self) -> dict[str, Any]:
        return self.store.status().as_dict()


_SERVICES: dict[Path, CodeIntelService] = {}
_SERVICES_LOCK = threading.Lock()


def get_codeintel_service(root: Path) -> CodeIntelService:
    canonical = canonical_project_root(root)
    with _SERVICES_LOCK:
        service = _SERVICES.get(canonical)
        if service is None:
            service = CodeIntelService(canonical)
            _SERVICES[canonical] = service
        return service
