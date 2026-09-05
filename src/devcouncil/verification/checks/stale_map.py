"""Stale-map gate: flag when ``repo_map.json`` is missing or lags git HEAD.

Missing map is always stale (fail-closed on hard rigor). Advisory on easy/normal;
blocking on hard. Never raises.
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
                return [Gap(
                    id=next_gap_id(task.id, "STALEMAP-MISSING"),
                    severity="high" if stale_map_blocking else "medium",
                    gap_type="stale_map",
                    task_id=task.id,
                    description=(
                        "Repository map ``.devcouncil/repo_map.json`` is missing. "
                        "Subsystem neighbors, dependents, and liveness gates cannot run."
                    ),
                    evidence=[".devcouncil/repo_map.json"],
                    recommended_fix="Run `dev map` to generate the repository map, then re-verify.",
                    blocking=stale_map_blocking,
                    suggested_command="dev map",
                )]
            from devcouncil.utils.json_persist import read_json

            loaded = read_json(map_path)
            if not isinstance(loaded, dict):
                return []
            data = loaded

        # Both signals, and stale wins.
        #
        # `client.is_map_stale()` is `not is_fresh or pending_count > 0`, and
        # `is_fresh` is itself `pending_count == 0` — so it only asks whether the
        # kernel has queued work. A CLI-driven build leaves that queue empty, so
        # consulting it *first and alone* answered "fresh" for a map whose
        # source had demonstrably changed. Measured: kernel `False`, fingerprint
        # `True`, on the same repository after one edit.
        #
        # The fingerprints in `repo_map.json` — the artifact the Rust kernel
        # writes — are what actually answer "does this map still describe the
        # code", so they are always consulted, and either signal reporting stale
        # makes it stale.
        # Tri-state on purpose: True/False are the kernel's answer, None means
        # it had none to give and the fingerprints decide.
        stale: bool | None = None
        try:
            from devcouncil.devmap_client import try_connect

            client = try_connect(project_root)
            if client is not None:
                stale = client.is_map_stale() or None
        except Exception:
            # Was `stale = None` with no record, so a kernel that raised on
            # every call looked identical to one that was simply not built.
            logger.warning(
                "devmap (Rust) staleness check failed; falling back to the "
                "repo_map.json fingerprints",
                exc_info=True,
            )
            stale = None
        if stale is None:
            # Not a second engine: `map_is_stale` compares `repo_map.json` —
            # the artifact the Rust kernel writes, fingerprints included —
            # against git. Same question, answered from the kernel's own output.
            from devcouncil.indexing.repo_mapper import RepoMapper

            stale = RepoMapper(project_root).map_is_stale(dict(data))
        if not stale:
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
