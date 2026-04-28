from typing import Any, Callable, Dict, List
import inspect
import logging

logger = logging.getLogger(__name__)

class EventBus:
    """Simple asynchronous event bus for DevCouncil orchestration."""
    def __init__(self):
        self._listeners: Dict[str, List[Callable]] = {}

    def subscribe(self, event_type: str, callback: Callable):
        if event_type not in self._listeners:
            self._listeners[event_type] = []
        self._listeners[event_type].append(callback)

    async def emit(self, event_type: str, payload: Any = None):
        """Emit an event asynchronously to all registered listeners.
        
        Supports both sync and async callbacks.
        """
        logger.debug(f"Event emitted: {event_type}")
        if event_type in self._listeners:
            for callback in self._listeners[event_type]:
                try:
                    if inspect.iscoroutinefunction(callback):
                        await callback(payload)
                    else:
                        callback(payload)
                except Exception as e:
                    logger.error(f"Error in event listener for {event_type}: {e}")

# Global event bus instance
bus = EventBus()

# Standard Event Types
class EventTypes:
    PLANNING_STARTED = "planning_started"
    PLANNING_COMPLETED = "planning_completed"
    TASK_EXECUTING = "task_executing"
    TASK_VERIFIED = "task_verified"
    TASK_BLOCKED = "task_blocked"
    GATE_FAILED = "gate_failed"
    MODEL_CALLED = "model_called"
