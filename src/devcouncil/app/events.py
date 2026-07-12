from typing import Any
import logging

logger = logging.getLogger(__name__)

class EventBus:
    """Simple asynchronous event bus for DevCouncil orchestration."""

    async def emit(self, event_type: str, payload: Any = None):
        """Emit an event (currently log-only; listeners were unused)."""
        logger.debug("Event emitted: %s", event_type)
        _ = payload

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
