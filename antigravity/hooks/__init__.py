"""
antigravity.hooks package.

Provides the 5 complete Antigravity agent lifecycle hooks:
- on_session_start_hook (on_session_start.py)
- pre_tool_call_decide_hook (pre_tool_call_decide.py)
- post_tool_call_hook (post_tool_call.py)
- stop_hook (stop.py - The Hero Loop)
- on_session_end_hook (on_session_end.py)
"""

from .on_session_start import on_session_start_hook
from .pre_tool_call_decide import pre_tool_call_decide_hook
from .post_tool_call import post_tool_call_hook
from .stop import stop_hook
from .on_session_end import on_session_end_hook
from .session_state import (
    HookResult,
    SessionState,
    StopArgs,
    StopDecision,
    StopHookResult,
    ToolCall,
    state,
)

__all__ = [
    "on_session_start_hook",
    "pre_tool_call_decide_hook",
    "post_tool_call_hook",
    "stop_hook",
    "on_session_end_hook",
    "HookResult",
    "SessionState",
    "StopArgs",
    "StopDecision",
    "StopHookResult",
    "ToolCall",
    "state",
]
