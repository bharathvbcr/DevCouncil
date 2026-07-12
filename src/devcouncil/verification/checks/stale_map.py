"""Stale-map gate: flag when ``repo_map.json`` lags the current git HEAD / file set.

Advisory on easy/normal rigor; blocking on hard (same posture as unwired/dead_symbol).
Never raises — degrades to zero gaps when the map is absent or fingerprinting is
unavailable (legacy maps without ``generated_head``/``indexed_hash`` are never stale).
"""

from __future__ import annotations

import logging
from pathlib import Path
from typing import Callable, List, Mapping, Optional

from devcouncil.domain.gap import Gap
from devcouncil.domain.task import Task

logger = logging.getLogger(__name__)


def detect_stale_map_gaps(
    *,
    task: Task,
    project_root: Path,
    next_gap_id: Callable[[str, str], str],
    stale_map_enabled: bool = True,
    stale_map_blocking: bool = False,
    repo_map: Optional[Mapping] = None,
) -> List[Gap]:
    """Emit a ``stale_map`` gap when the on-disk map no longer matches the repo."""
    if not stale_map_enabled:
        return []
    try:
        data = repo_map
        if data is None:
            map_path = project_root / ".devcouncil" / "repo_map.json"
            if not map_path.is_file():
                return []
            from devcouncil.utils.json_persist import read_json

            loaded = read_json(map_path)
            if not isinstance(loaded, dict):
                return []
            data = loaded

        from devcouncil.indexing.repo_mapper import RepoMapper

        if not RepoMapper(project_root).map_is_stale(dict(data)):
            return []

        stored_head = str(data.get("generated_head") or "") or "(unknown)"
        return [Gap(
            id=next_gap_id(task.id, "STALEMAP"),
            severity="high" if stale_map_blocking else "medium",
            gap_type="stale_map",
            task_id=task.id,
            description=(
                "The repository map (``.devcouncil/repo_map.json``) is behind the "
                f"current code (stored HEAD {stored_head}). Subsystem neighbors, "
                "dependents, and liveness lists may be wrong until it is refreshed."
            ),
            evidence=[".devcouncil/repo_map.json", f"generated_head:{stored_head}"],
            recommended_fix="Run `dev map` to regenerate the repository map, then re-verify.",
            blocking=stale_map_blocking,
            suggested_command="dev map",
        )]
    except Exception:
        logger.debug("detect_stale_map_gaps failed; degrading to zero gaps", exc_info=True)
        return []
