"""Process-wide run attribution for telemetry.

Every model_calls.jsonl record has a ``run_id`` field, but historically it was
None on 100% of real records: the LLM router accepts ``run_id`` per call, and
none of the planning/verification services threaded it through. Rather than
touching every call site (and every future one), the orchestrator declares the
current run here and the router falls back to it when a caller doesn't pass one
explicitly.

A ``ContextVar`` (not a bare global) so concurrent runs in one process — e.g.
an MCP server planning two projects — attribute calls to their own run.
"""

from __future__ import annotations

from contextvars import ContextVar
from typing import Optional

_current_run_id: ContextVar[Optional[str]] = ContextVar("devcouncil_run_id", default=None)


def set_current_run_id(run_id: Optional[str]) -> None:
    """Declare the run all subsequent (context-local) model calls belong to."""
    _current_run_id.set(run_id)


def get_current_run_id() -> Optional[str]:
    return _current_run_id.get()
