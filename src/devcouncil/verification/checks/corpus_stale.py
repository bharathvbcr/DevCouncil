"""Corpus freshness gate for tasks touching docs/corpus paths."""

from __future__ import annotations

import logging
from pathlib import Path
from typing import Callable, List, Optional

from devcouncil.domain.gap import Gap
from devcouncil.domain.task import Task

logger = logging.getLogger(__name__)


def _norm(path: str) -> str:
    return path.replace("\\", "/").lstrip("./")


def _task_touches_corpus(
    *,
    task: Task,
    changed_files: List[str],
    corpus_paths: List[str],
) -> bool:
    corpus_norm = {_norm(p) for p in corpus_paths}
    for pf in task.planned_files:
        rel = _norm(pf.path)
        if rel in corpus_norm or any(rel.startswith(c + "/") for c in corpus_norm):
            return True
        if rel.endswith(".md") or rel.startswith("docs/"):
            return True
    for path in changed_files:
        rel = _norm(path)
        if rel in corpus_norm or any(rel.startswith(c + "/") for c in corpus_norm):
            return True
        if rel.endswith(".md") or rel.startswith("docs/"):
            return True
    return False


def _corpus_is_stale(project_root: Path) -> tuple[bool, str]:
    try:
        from devcouncil.indexing.wiring import corpus_graph_path, load_corpus_settings
        from devcouncil.indexing.repo_mapper import RepoMapper

        settings = load_corpus_settings(project_root)
        if not settings.enabled:
            return False, "corpus disabled"
        graph_path = corpus_graph_path(project_root)
        if not graph_path.is_file():
            return True, "corpus graph missing"
        mapper = RepoMapper(project_root)
        map_path = project_root / ".devcouncil" / "repo_map.json"
        if map_path.is_file():
            from devcouncil.utils.json_persist import read_json

            loaded = read_json(map_path)
            if isinstance(loaded, dict) and mapper.map_is_stale(loaded):
                return True, "repo map stale vs corpus fingerprint"
        return False, "fresh"
    except Exception as exc:
        logger.debug("corpus stale check failed: %s", exc)
        return True, "corpus check error"


def detect_corpus_stale_gaps(
    *,
    task: Task,
    project_root: Path,
    changed_files: List[str],
    next_gap_id: Callable[[str, str], str],
    corpus_stale_enabled: bool = True,
    corpus_stale_blocking: bool = False,
    corpus_paths: Optional[List[str]] = None,
) -> List[Gap]:
    if not corpus_stale_enabled:
        return []
    try:
        from devcouncil.indexing.wiring import load_corpus_settings

        settings = load_corpus_settings(project_root)
        paths = corpus_paths if corpus_paths is not None else list(settings.paths)
        if not _task_touches_corpus(task=task, changed_files=changed_files, corpus_paths=paths):
            return []
        stale, reason = _corpus_is_stale(project_root)
        if not stale:
            return []
        return [Gap(
            id=next_gap_id(task.id, "CORPUSSTALE"),
            severity="high" if corpus_stale_blocking else "medium",
            gap_type="corpus_stale",
            task_id=task.id,
            description=(
                f"Corpus index is stale or missing ({reason}). "
                "Doc/corpus navigation and doc-code-ref gates may be wrong."
            ),
            evidence=[".devcouncil/corpus/graph.json", reason],
            recommended_fix="Run `dev map` (corpus rebuild) or enable auto_refresh_on_verify.",
            blocking=corpus_stale_blocking,
            suggested_command="dev map",
        )]
    except Exception:
        logger.debug("detect_corpus_stale_gaps failed", exc_info=True)
        return []


def refresh_corpus_on_verify_if_needed(project_root: Path) -> bool:
    """Best-effort corpus rebuild when enabled and stale."""
    try:
        from devcouncil.indexing.wiring import load_corpus_settings

        settings = load_corpus_settings(project_root)
        if not settings.enabled or not settings.auto_refresh_on_verify:
            return False
        stale, _ = _corpus_is_stale(project_root)
        if not stale:
            return False
        from devcouncil.indexing.wiring import build_corpus

        build_corpus(project_root)
        return True
    except Exception:
        logger.debug("corpus verify refresh failed", exc_info=True)
        return False
