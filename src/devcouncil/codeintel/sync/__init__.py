"""Native filesystem synchronization for code-intelligence generations."""

from devcouncil.codeintel.sync.coordinator import (
    SyncCoordinator,
    SyncState,
    get_sync_coordinator,
    stop_all_coordinators,
)
from devcouncil.codeintel.sync.incremental import sync_affected_paths
from devcouncil.codeintel.sync.scope import IndexScope

__all__ = [
    "IndexScope",
    "SyncCoordinator",
    "SyncState",
    "get_sync_coordinator",
    "sync_affected_paths",
    "stop_all_coordinators",
]
