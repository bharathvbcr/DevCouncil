"""
session_state.py - Shared session state and Antigravity hook types.

Maintains agent session state, leases, worktree paths, and typed hook contracts.
Supports both Google Antigravity SDK native types and local runtime fallbacks.
"""

import os
import sys
from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Callable, Dict, List, Optional


class StopDecision(str, Enum):
    ALLOW_STOP = "ALLOW_STOP"
    CONTINUE = "CONTINUE"


@dataclass
class ToolCall:
    name: str
    arguments: Dict[str, Any] = field(default_factory=dict)


@dataclass
class HookResult:
    allow: bool = True
    reason: Optional[str] = None


@dataclass
class StopArgs:
    continuation_count: int = 0
    trajectory_length: int = 0


@dataclass
class StopHookResult:
    decision: StopDecision = StopDecision.ALLOW_STOP
    reason: Optional[str] = None


@dataclass
class SessionState:
    repo_path: str = field(default_factory=os.getcwd)
    task_id: Optional[str] = field(
        default_factory=lambda: os.environ.get("DEVCOUNCIL_TASK_ID")
    )
    client_id: str = field(
        default_factory=lambda: f"antigravity-worker-{os.getpid()}"
    )
    lease_token: Optional[str] = None
    worktree_path: Optional[str] = None
    continuation_threshold: int = 5  # Max Hero Loop self-repair attempts
    active_branch: Optional[str] = None

    def reset(self):
        self.repo_path = os.getcwd()
        self.task_id = os.environ.get("DEVCOUNCIL_TASK_ID")
        self.client_id = f"antigravity-worker-{os.getpid()}"
        self.lease_token = None
        self.worktree_path = None
        self.active_branch = None


# Singleton session state instance
state = SessionState()


# Decorator helpers that pass through if google.antigravity is not present
def hook_decorator(hook_name: str):
    def decorator(fn: Callable):
        setattr(fn, "__antigravity_hook__", hook_name)
        return fn
    return decorator


class DummyHooks:
    on_session_start = staticmethod(hook_decorator("on_session_start"))
    pre_tool_call_decide = staticmethod(hook_decorator("pre_tool_call_decide"))
    post_tool_call = staticmethod(hook_decorator("post_tool_call"))
    stop = staticmethod(hook_decorator("stop"))
    on_session_end = staticmethod(hook_decorator("on_session_end"))


# Attempt importing google.antigravity or fallback to DummyHooks
try:
    from google.antigravity.hooks import hooks as ag_hooks
    from google.antigravity import types as ag_types
except ImportError:
    ag_hooks = DummyHooks
    ag_types = None
