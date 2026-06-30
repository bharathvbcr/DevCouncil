"""Shared pytest fixtures.

Several modules memoize expensive state in module-level caches keyed by resolved
project root (the parsed config, local secrets, the SQLAlchemy ``Database`` handle,
the MCP server's router/adapter caches, and the skill registry). Every test uses a
unique ``tmp_path`` so these caches don't collide in practice, but the autouse
fixture below clears them after each test as a defensive guarantee — so a test that
deletes/recreates state under a path a later test happens to reuse can never receive
a stale cached instance.
"""

from __future__ import annotations

import pytest


def _reset_all_caches() -> None:
    # Each reset is best-effort and independent: importing one module must not
    # prevent the others from being cleared.
    try:
        from devcouncil.storage.db import reset_db_cache

        reset_db_cache()
    except Exception:
        pass
    try:
        from devcouncil.integrations.mcp.server import _reset_caches

        _reset_caches()
    except Exception:
        pass
    try:
        from devcouncil.skills.registry import clear_skill_caches

        clear_skill_caches()
    except Exception:
        pass
    try:
        from devcouncil.app import config

        config._CONFIG_CACHE.clear()
        config._SECRETS_CACHE.clear()
        # Drop any cached gcloud token so a monkeypatched token in one test can't leak
        # into the next via the 50-minute TTL cache.
        config._gcloud_token_cache = None
    except Exception:
        pass


@pytest.fixture(autouse=True)
def _clear_module_caches():
    yield
    _reset_all_caches()
