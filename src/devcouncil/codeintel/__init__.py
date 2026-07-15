"""Transactional, project-scoped code intelligence services.

The legacy :mod:`devcouncil.indexing.graph` package remains the public
compatibility surface.  New code should obtain a :class:`CodeIntelService` for a
canonical project root and query its committed SQLite generation.
"""

from devcouncil.codeintel.service import CodeIntelService, get_codeintel_service
from devcouncil.codeintel.store import CodeIntelStore, StoreStatus

__all__ = [
    "CodeIntelService",
    "CodeIntelStore",
    "StoreStatus",
    "get_codeintel_service",
]
