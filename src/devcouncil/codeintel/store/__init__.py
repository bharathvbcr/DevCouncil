"""SQLite persistence for runtime evidence.

``CodeIntelStore`` — the Python engine's versioned graph store — was exported
here until the Rust kernel became the only writer of the graph and its last two
Python callers lost theirs. See :mod:`devcouncil.codeintel.store.runtime`.
"""

from devcouncil.codeintel.store.runtime import RuntimeEvidenceStore

__all__ = ["RuntimeEvidenceStore"]
