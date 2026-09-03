"""Import-edge parse cache — Python ``modules`` and JS/TS ``specs`` keyed by sha256.

Written and read only by :class:`~devcouncil.indexing.repo_mapper.RepoMapper`'s
import-edge passes, which feed ``liveness_snapshot`` and the liveness ratchet. The
symbol/call extraction cache that shared this file was retired with the Python
graph builder.
"""

from __future__ import annotations

import logging
from pathlib import Path
from typing import Any, Dict, Set, cast

from devcouncil.utils.fsio import atomic_write_json
from devcouncil.utils.json_persist import read_json

logger = logging.getLogger(__name__)

# v1 = Python modules; v2 = + JS/TS specs; v3-v8 added extraction fields that the
# retired Python graph builder owned. The surviving ``modules``/``specs`` entries
# are unchanged by that retirement, so the version is held at 8 rather than bumped:
# a bump would discard every warm cache to drop keys nothing reads.
PARSE_CACHE_VERSION = 8

# Keys preserved across partial updates: the Python pass writes ``modules`` and the
# JS/TS pass writes ``specs``, and neither may clobber the other for a shared path.
_PRESERVE_KEYS = ("modules", "specs")


def cache_path(root: Path) -> Path:
    return root / ".devcouncil" / "cache" / "repo_map_parse.json"


def load_parse_cache(root: Path) -> Dict[str, Dict[str, Any]]:
    """Load cache. Version mismatch → empty dict (caller rebuilds in place)."""
    try:
        data = read_json(cache_path(root))
        if data.get("version") == PARSE_CACHE_VERSION and isinstance(data.get("files"), dict):
            return cast(Dict[str, Dict[str, Any]], data["files"])
    except Exception:
        pass
    return {}


def save_parse_cache(root: Path, files: Dict[str, Dict[str, Any]]) -> None:
    try:
        path = cache_path(root)
        path.parent.mkdir(parents=True, exist_ok=True)
        atomic_write_json(path, {"version": PARSE_CACHE_VERSION, "files": files})
    except Exception:
        logger.debug("Failed to write graph parse cache", exc_info=True)


def merge_parse_cache(
    root: Path,
    updates: Dict[str, Dict[str, Any]],
    managed: Set[str],
) -> None:
    """Merge updates for ``managed`` paths; preserve sibling-language / sibling-field keys.

    Paths in ``managed`` but absent from ``updates`` are pruned. When an update
    omits the sibling language's field, the prior entry's value is kept so the
    Python and JS/TS passes share one store.
    """
    cache = load_parse_cache(root)
    merged = {k: v for k, v in cache.items() if k not in managed}
    for path, entry in updates.items():
        prev = cache.get(path)
        if isinstance(prev, dict) and isinstance(entry, dict):
            for key in _PRESERVE_KEYS:
                if key not in entry and key in prev:
                    entry = {**entry, key: prev[key]}
        merged[path] = entry
    if merged != cache:
        save_parse_cache(root, merged)

