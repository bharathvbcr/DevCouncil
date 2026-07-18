"""Opt-in local embeddings for semantic graph search (hash fallback)."""

from __future__ import annotations

import hashlib
import json
import logging
import math
import re
import sqlite3
from pathlib import Path
from typing import Any, Dict, List

logger = logging.getLogger(__name__)

_EMBEDDINGS_TABLE = "symbol_embeddings"


def _index_db(project_root: Path) -> Path:
    return project_root / ".devcouncil" / "codeintel" / "index.sqlite"


def _tokenize(text: str) -> List[str]:
    return [t for t in re.split(r"[^a-zA-Z0-9_]+", text.lower()) if len(t) > 1]


def _hash_vector(tokens: List[str], dims: int = 64) -> List[float]:
    vec = [0.0] * dims
    for token in tokens:
        digest = hashlib.sha256(token.encode("utf-8")).digest()
        for i in range(dims):
            vec[i] += (digest[i % len(digest)] - 128) / 128.0
    norm = math.sqrt(sum(v * v for v in vec)) or 1.0
    return [v / norm for v in vec]


def _cosine(a: List[float], b: List[float]) -> float:
    return sum(x * y for x, y in zip(a, b))


def _current_generation(project_root: Path) -> int | None:
    try:
        from devcouncil.codeintel import get_codeintel_service

        return get_codeintel_service(project_root).store.current_generation()
    except Exception:
        return None


def ensure_embeddings_schema(project_root: Path) -> None:
    db = _index_db(project_root)
    db.parent.mkdir(parents=True, exist_ok=True)
    with sqlite3.connect(db) as conn:
        conn.execute(
            f"""
            CREATE TABLE IF NOT EXISTS {_EMBEDDINGS_TABLE} (
                node_id TEXT NOT NULL,
                path TEXT NOT NULL,
                label TEXT NOT NULL,
                model TEXT NOT NULL,
                vector_json TEXT NOT NULL,
                generation_id INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (generation_id, node_id)
            )
            """
        )
        cols = {
            str(row[1])
            for row in conn.execute(f"PRAGMA table_info({_EMBEDDINGS_TABLE})").fetchall()
        }
        if "generation_id" not in cols:
            # Legacy single-generation table: rebuild on next build_embeddings.
            conn.execute(f"ALTER TABLE {_EMBEDDINGS_TABLE} ADD COLUMN generation_id INTEGER NOT NULL DEFAULT 0")
        conn.execute(
            f"CREATE INDEX IF NOT EXISTS idx_{_EMBEDDINGS_TABLE}_gen "
            f"ON {_EMBEDDINGS_TABLE}(generation_id)"
        )
        conn.commit()


def embeddings_enabled(project_root: Path) -> bool:
    try:
        from devcouncil.app.config import load_config

        return bool(load_config(project_root).indexing.embeddings.enabled)
    except Exception:
        return False


def build_embeddings(project_root: Path) -> int:
    """Rebuild symbol embeddings for the committed codeintel generation."""
    if not embeddings_enabled(project_root):
        return 0
    ensure_embeddings_schema(project_root)
    generation = _current_generation(project_root)
    if generation is None:
        return 0
    from devcouncil.codeintel.query.engine import CodeIntelQueryEngine

    try:
        graph = CodeIntelQueryEngine(project_root)._graph()
    except FileNotFoundError:
        return 0
    model = "hash-v1"
    try:
        from devcouncil.app.config import load_config

        model = load_config(project_root).indexing.embeddings.model_name or model
    except Exception:
        pass
    count = 0
    try:
        with sqlite3.connect(_index_db(project_root)) as conn:
            conn.execute(
                f"DELETE FROM {_EMBEDDINGS_TABLE} WHERE generation_id=?",
                (generation,),
            )
            # Drop older generations; keep only current + previous.
            conn.execute(
                f"DELETE FROM {_EMBEDDINGS_TABLE} WHERE generation_id NOT IN (?, ?)",
                (generation, max(0, generation - 1)),
            )
            for node in graph.nodes:
                label = f"{node.path} {node.name} {node.kind}"
                vec = _hash_vector(_tokenize(label))
                conn.execute(
                    f"INSERT OR REPLACE INTO {_EMBEDDINGS_TABLE} "
                    f"(node_id, path, label, model, vector_json, generation_id) "
                    f"VALUES (?, ?, ?, ?, ?, ?)",
                    (node.id, node.path, node.name, model, json.dumps(vec), generation),
                )
                count += 1
            conn.commit()
    except sqlite3.Error:
        logger.warning("embedding rebuild failed; continuing without embeddings", exc_info=True)
        return 0
    return count


def semantic_search(
    project_root: Path,
    query: str,
    *,
    limit: int = 50,
) -> Dict[str, Any]:
    """Semantic search over stored embeddings; empty when disabled."""
    if not embeddings_enabled(project_root):
        return {"ok": False, "reason": "embeddings disabled", "matches": []}
    ensure_embeddings_schema(project_root)
    generation = _current_generation(project_root)
    qvec = _hash_vector(_tokenize(query))
    matches: List[Dict[str, Any]] = []
    scanned = 0
    max_scan = max(500, limit * 200)
    try:
        with sqlite3.connect(_index_db(project_root)) as conn:
            if generation is None:
                rows = conn.execute(
                    f"SELECT node_id, path, label, model, vector_json FROM {_EMBEDDINGS_TABLE}"
                )
            else:
                rows = conn.execute(
                    f"SELECT node_id, path, label, model, vector_json FROM {_EMBEDDINGS_TABLE} "
                    f"WHERE generation_id=?",
                    (generation,),
                )
            for node_id, path, label, model, vector_json in rows:
                scanned += 1
                if scanned > max_scan:
                    break
                try:
                    vec = json.loads(vector_json)
                except json.JSONDecodeError:
                    continue
                score = _cosine(qvec, vec)
                matches.append({
                    "id": node_id,
                    "path": path,
                    "label": label,
                    "score": round(score, 4),
                    "model": model,
                })
    except sqlite3.Error as exc:
        logger.warning("semantic search failed: %s", exc)
        return {"ok": False, "reason": f"embeddings store error: {exc}", "matches": []}
    matches.sort(key=lambda item: item["score"], reverse=True)
    return {
        "ok": True,
        "matches": matches[: max(1, limit)],
        "scanned": scanned,
        "truncated": scanned > max_scan,
        "generation": generation,
        "backend": "hash-v1",
    }
