"""Cross-process writer lease for the code-intelligence store.

What used to live here — ``SyncCoordinator`` (a watchdog watcher), ``IndexScope``
and ``sync_affected_paths`` (incremental extraction with the Python engine) —
wrote ``repo_map.json`` and ``code_graph.json`` on every edit, racing the Rust
kernel for the same two files. The kernel is the only writer now and its daemon
does its own watching, so only the lease survives.
"""

from devcouncil.codeintel.sync.lease import LeaseHolder, WriterLease, read_holder

__all__ = [
    "LeaseHolder",
    "WriterLease",
    "read_holder",
]
