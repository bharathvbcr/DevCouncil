"""Debugger control and fingerprint-scoped runtime evidence."""

from devcouncil.codeintel.debug.discovery import AdapterInfo, discover_adapters
from devcouncil.codeintel.debug.protocol import DAPClient, DAPError
from devcouncil.codeintel.debug.session import DebugSession, DebugSessionManager, get_debug_manager

__all__ = [
    "AdapterInfo",
    "DAPClient",
    "DAPError",
    "DebugSession",
    "DebugSessionManager",
    "discover_adapters",
    "get_debug_manager",
]
