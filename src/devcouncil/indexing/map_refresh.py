"""Best-effort refresh of ``.devcouncil/repo_map.json`` when fingerprints drift."""

from __future__ import annotations

import logging
from pathlib import Path

from devcouncil.utils.json_persist import read_json

logger = logging.getLogger(__name__)


def refresh_stale_map_if_needed(
    project_root: Path,
    *,
    on_checkout: bool = True,
    on_verify: bool = True,
) -> bool:
    """Regenerate ``repo_map.json`` when its tracked-content fingerprint is stale.

    Honors ``execution.refresh_stale_map_on_checkout`` and
    ``execution.refresh_stale_map_on_verify`` (both default on). Pass
    ``on_checkout=False`` or ``on_verify=False`` to skip a context. Never raises;
    returns True when a remap ran successfully.

    The rebuild goes through the Rust kernel — the only writer of the map. This
    used to call the Python engine, so a `dev verify` or a task checkout on a
    stale map rewrote `repo_map.json` from `index.sqlite` (generation 76, built
    from an old HEAD) while `dev map` wrote it from `devmap.sqlite` (generation
    511): two writers, one artifact, and the last one to run won.
    """
    try:
        from devcouncil.app.config import load_config

        try:
            cfg = load_config(project_root)
            checkout_enabled = bool(
                getattr(cfg.execution, "refresh_stale_map_on_checkout", True)
            )
            verify_enabled = bool(
                getattr(cfg.execution, "refresh_stale_map_on_verify", True)
            )
        except Exception:
            checkout_enabled = True
            verify_enabled = True

        if on_checkout and not on_verify:
            enabled = checkout_enabled
        elif on_verify and not on_checkout:
            enabled = verify_enabled
        else:
            enabled = checkout_enabled or verify_enabled
        if not enabled:
            return False

        map_path = project_root / ".devcouncil" / "repo_map.json"
        if not map_path.is_file():
            data: dict = {}
        else:
            loaded = read_json(map_path)
            data = loaded if isinstance(loaded, dict) else {}

        from devcouncil.indexing.repo_mapper import RepoMapper

        mapper = RepoMapper(project_root)
        if map_path.is_file() and not mapper.map_is_stale(data):
            return False

        from devcouncil.devmap_engine import DevMapEngineError
        from devcouncil.indexing.map_artifacts import refresh_map_artifacts

        try:
            refresh_map_artifacts(project_root, map_path, quiet=True)
        except DevMapEngineError as exc:
            # A kernel that cannot run (not built, older than the store, a
            # writer holding the lock) must not fail a checkout or a verify.
            # The map stays stale until the next successful `dev map`, and the
            # reason is logged rather than swallowed.
            logger.warning("map refresh deferred: %s", exc)
            return False
        return True
    except Exception:
        logger.warning("map refresh failed", exc_info=True)
        return False


# Back-compat alias for plan expected tests.
refresh_repository_map = refresh_stale_map_if_needed
