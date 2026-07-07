"""Per-root ModelRouter cache for MCP verify/supervise handlers."""

from __future__ import annotations

import logging
from pathlib import Path
from typing import Any

from devcouncil.integrations.mcp.handlers import ast_lsp as ast_lsp_handlers

logger = logging.getLogger(__name__)

_ROUTER_CACHE: dict[str, tuple[Any, Any]] = {}


def reset_caches() -> None:
    """Drop all per-root MCP caches (router/ast/lsp). For test isolation."""
    _ROUTER_CACHE.clear()
    ast_lsp_handlers.reset_caches()


def load_router(root: Path):
    """Build a ModelRouter from project config, or return None when no provider key
    is configured. Cached per resolved project root with config stat invalidation."""
    key = str(root.resolve())
    try:
        cfg_stat = (root / ".devcouncil" / "config.yaml").stat()
        signature: object = (cfg_stat.st_mtime_ns, cfg_stat.st_size, cfg_stat.st_ino)
    except OSError:
        signature = None
    cached = _ROUTER_CACHE.get(key)
    if cached is not None and cached[0] == signature:
        return cached[1]
    router = _build_router(root)
    _ROUTER_CACHE[key] = (signature, router)
    return router


def _build_router(root: Path):
    try:
        from devcouncil.app.config import get_api_key, load_config
        from devcouncil.llm.provider import create_provider, validate_model_provider
        from devcouncil.llm.router import ModelRouter

        config = load_config(root)
        validate_model_provider(config.models.provider)
        api_key = get_api_key(config.models.provider, root)
        provider = create_provider(config.models.provider, api_key, project_root=root, provider_prefs=config.provider)
        role_config = {name: role.model_dump() for name, role in config.models.roles.items()}
        return ModelRouter(provider, role_config, project_root=root)
    except Exception:
        logger.warning("Failed to build MCP model router for %s", root, exc_info=True)
        return None
