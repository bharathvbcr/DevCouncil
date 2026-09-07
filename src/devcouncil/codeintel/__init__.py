"""Transactional, project-scoped code intelligence services.

The Rust kernel (``rust-port/``) is the code-intelligence engine: it extracts,
resolves, analyses and persists the graph. What is left here is the Python side
of two things the kernel does not do — the opt-in debug tracer under
:mod:`devcouncil.codeintel.debug`, with the runtime-evidence store it writes,
and the tree-sitter grammar registry under
:mod:`devcouncil.codeintel.languages`.

``CodeIntelStore`` (the Python graph store) and ``StoreStatus`` were exported
here until nothing wrote or read a graph generation any more.
"""

from devcouncil.codeintel.service import CodeIntelService, get_codeintel_service
from devcouncil.codeintel.store import RuntimeEvidenceStore

__all__ = [
    "CodeIntelService",
    "RuntimeEvidenceStore",
    "get_codeintel_service",
]
