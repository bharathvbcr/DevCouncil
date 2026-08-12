"""Project-scoped lifecycle and compatibility service for code intelligence."""

from __future__ import annotations

import sqlite3
import subprocess
import threading
import time
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
        progress: Callable[[str, int, int], None] | None = None,
    ) -> int:
        try:
            generation = self.store.save_graph(
                graph,
                changed_paths=changed_paths,
                analysis_shards=analysis_shards,
                progress=progress,
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
                progress=progress,
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


def _git_head(root: Path) -> str:
    try:
        result = subprocess.run(
            ["git", "-C", str(root), "rev-parse", "HEAD"],
            capture_output=True,
            text=True,
            timeout=5.0,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired):
        return ""
    return result.stdout.strip() if result.returncode == 0 else ""


def index_freshness(root: Path) -> dict[str, Any]:
    """Compare the committed generation's provenance against the repo HEAD.

    ``fresh`` is True when the index was built from the current HEAD, False
    when it was built from a different commit, and None when it cannot be
    determined (no index, no git, no recorded head). Callers must treat None
    as *unknown*, never as fresh — a check that could not run must not report
    the same result as a check that ran and passed.
    """
    service = get_codeintel_service(root)
    result: dict[str, Any] = {
        "fresh": None,
        "reason": "",
        "generation": None,
        "index_head": "",
        "current_head": "",
        "indexed_at": None,
        "age_seconds": None,
    }
    try:
        provenance = service.store.generation_provenance()
    except sqlite3.Error as exc:
        result["reason"] = f"index store unreadable: {exc}"
        return result
    if provenance is None:
        result["reason"] = "no committed index generation"
        return result
    result["generation"] = provenance["generation"]
    result["index_head"] = provenance["generated_head"]
    result["indexed_at"] = provenance["created_at"]
    result["age_seconds"] = max(0.0, time.time() - provenance["created_at"])
    current = _git_head(service.project_root)
    result["current_head"] = current
    if not current:
        result["reason"] = "git HEAD unavailable; index freshness unknown"
        return result
    if not provenance["generated_head"]:
        result["reason"] = "index generation has no recorded head; freshness unknown"
        return result
    result["fresh"] = provenance["generated_head"] == current
    if not result["fresh"]:
        result["reason"] = (
            f"index generation {provenance['generation']} was built from "
            f"{provenance['generated_head'][:12]} but HEAD is {current[:12]}"
        )
    return result


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
