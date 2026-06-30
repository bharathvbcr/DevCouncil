"""Gating state machine for DevCouncil project lifecycle.

States (from §12):
  NEW -> REPO_MAPPED -> REQUIREMENTS_DRAFTED -> PLANS_GENERATED
  -> CRITIQUES_GENERATED -> ARBITRATED -> AWAITING_USER_DECISIONS
  -> PLAN_APPROVED -> TASK_READY -> TASK_EXECUTING -> TASK_VERIFYING
  -> TASK_BLOCKED | TASK_VERIFIED -> PROJECT_DONE
"""

from __future__ import annotations

import logging
from enum import Enum
from typing import Dict, List, Set

logger = logging.getLogger(__name__)



class ProjectPhase(str, Enum):
    NEW = "NEW"
    REPO_MAPPED = "REPO_MAPPED"
    REQUIREMENTS_DRAFTED = "REQUIREMENTS_DRAFTED"
    PLANS_GENERATED = "PLANS_GENERATED"
    CRITIQUES_GENERATED = "CRITIQUES_GENERATED"
    ARBITRATED = "ARBITRATED"
    AWAITING_USER_DECISIONS = "AWAITING_USER_DECISIONS"
    PLAN_APPROVED = "PLAN_APPROVED"
    TASK_READY = "TASK_READY"
    TASK_EXECUTING = "TASK_EXECUTING"
    TASK_VERIFYING = "TASK_VERIFYING"
    TASK_BLOCKED = "TASK_BLOCKED"
    TASK_VERIFIED = "TASK_VERIFIED"
    PROJECT_DONE = "PROJECT_DONE"


# Valid transitions: from_phase -> set of valid next phases
TRANSITIONS: Dict[ProjectPhase, Set[ProjectPhase]] = {
    ProjectPhase.NEW: {ProjectPhase.REPO_MAPPED},
    ProjectPhase.REPO_MAPPED: {ProjectPhase.REQUIREMENTS_DRAFTED},
    ProjectPhase.REQUIREMENTS_DRAFTED: {ProjectPhase.PLANS_GENERATED},
    ProjectPhase.PLANS_GENERATED: {ProjectPhase.CRITIQUES_GENERATED},
    ProjectPhase.CRITIQUES_GENERATED: {ProjectPhase.ARBITRATED},
    ProjectPhase.ARBITRATED: {
        ProjectPhase.AWAITING_USER_DECISIONS,
        ProjectPhase.PLAN_APPROVED,
    },
    ProjectPhase.AWAITING_USER_DECISIONS: {ProjectPhase.PLAN_APPROVED},
    ProjectPhase.PLAN_APPROVED: {ProjectPhase.TASK_READY},
    ProjectPhase.TASK_READY: {ProjectPhase.TASK_EXECUTING},
    ProjectPhase.TASK_EXECUTING: {ProjectPhase.TASK_VERIFYING},
    ProjectPhase.TASK_VERIFYING: {
        ProjectPhase.TASK_VERIFIED,
        ProjectPhase.TASK_BLOCKED,
    },
    ProjectPhase.TASK_BLOCKED: {
        ProjectPhase.TASK_READY,  # after repair
    },
    ProjectPhase.TASK_VERIFIED: {
        ProjectPhase.TASK_READY,  # next task
        ProjectPhase.PROJECT_DONE,
    },
    ProjectPhase.PROJECT_DONE: set(),
}


class InvalidTransitionError(Exception):
    """Raised when a state transition is not allowed."""

    def __init__(self, current: ProjectPhase, target: ProjectPhase):
        self.current = current
        self.target = target
        super().__init__(
            f"Invalid transition: {current.value} -> {target.value}. "
            f"Valid targets: {', '.join(t.value for t in TRANSITIONS.get(current, set()))}"
        )


class StateMachine:
    """Manages project phase transitions with validation."""

    def __init__(self, initial: ProjectPhase = ProjectPhase.NEW):
        self._phase = initial
        self._history: List[ProjectPhase] = [initial]

    @property
    def phase(self) -> ProjectPhase:
        return self._phase

    @property
    def history(self) -> List[ProjectPhase]:
        return list(self._history)

    def can_transition(self, target: ProjectPhase) -> bool:
        """Check if a transition to the target phase is valid."""
        valid = TRANSITIONS.get(self._phase, set())
        return target in valid

    def transition(self, target: ProjectPhase) -> None:
        """Transition to a new phase.
        
        Raises InvalidTransitionError if the transition is not allowed.
        """
        if not self.can_transition(target):
            logger.error("Invalid phase transition: %s -> %s", self._phase.value, target.value)
            raise InvalidTransitionError(self._phase, target)
        self._phase = target
        self._history.append(target)

    def valid_transitions(self) -> Set[ProjectPhase]:
        """Return the set of valid next phases from the current state."""
        return TRANSITIONS.get(self._phase, set())
