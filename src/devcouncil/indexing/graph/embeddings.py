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


def ensure_embeddings_schema(project_root: Path) -> None:
    db = _index_db(project_root)
    db.parent.mkdir(parents=True, exist_ok=True)
    with sqlite3.connect(db) as conn:
        conn.execute(
            f"""
            CREATE TABLE IF NOT EXISTS {_EMBEDDINGS_TABLE} (
                node_id TEXT PRIMARY KEY,
                path TEXT NOT NULL,
                label TEXT NOT NULL,
                model TEXT NOT NULL,
                vector_json TEXT NOT NULL
            )
            """
        )
        conn.commit()


def embeddings_enabled(project_root: Path) -> bool:
    try:
        from devcouncil.app.config import load_config

        return bool(load_config(project_root).indexing.embeddings.enabled)
    except Exception:
        return False


def build_embeddings(project_root: Path) -> int:
    """Rebuild symbol embeddings from the committed codeintel generation."""
    if not embeddings_enabled(project_root):
        return 0
    ensure_embeddings_schema(project_root)
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
    with sqlite3.connect(_index_db(project_root)) as conn:
        conn.execute(f"DELETE FROM {_EMBEDDINGS_TABLE}")
        for node in graph.nodes:
            label = f"{node.path} {node.name} {node.kind}"
            vec = _hash_vector(_tokenize(label))
            conn.execute(
                f"INSERT INTO {_EMBEDDINGS_TABLE} (node_id, path, label, model, vector_json) "
                f"VALUES (?, ?, ?, ?, ?)",
                (node.id, node.path, node.name, model, json.dumps(vec)),
            )
            count += 1
        conn.commit()
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
    qvec = _hash_vector(_tokenize(query))
    matches: List[Dict[str, Any]] = []
    with sqlite3.connect(_index_db(project_root)) as conn:
        for node_id, path, label, model, vector_json in conn.execute(
            f"SELECT node_id, path, label, model, vector_json FROM {_EMBEDDINGS_TABLE}"
        ):
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
    matches.sort(key=lambda m: m["score"], reverse=True)
    return {"ok": True, "matches": matches[:limit], "backend": "hash-v1"}
