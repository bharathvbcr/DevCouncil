"""Tests for the gating state machine."""

import pytest
from devcouncil.app.state_machine import (
    StateMachine,
    ProjectPhase,
    InvalidTransitionError,
)


class TestStateMachine:
    def test_initial_state(self):
        sm = StateMachine()
        assert sm.phase == ProjectPhase.NEW

    def test_custom_initial_state(self):
        sm = StateMachine(initial=ProjectPhase.PLAN_APPROVED)
        assert sm.phase == ProjectPhase.PLAN_APPROVED

    def test_valid_transition(self):
        sm = StateMachine()
        sm.transition(ProjectPhase.REPO_MAPPED)
        assert sm.phase == ProjectPhase.REPO_MAPPED

    def test_invalid_transition_raises(self):
        sm = StateMachine()
        with pytest.raises(InvalidTransitionError) as exc_info:
            sm.transition(ProjectPhase.PLAN_APPROVED)
        assert "NEW" in str(exc_info.value)
        assert "PLAN_APPROVED" in str(exc_info.value)

    def test_full_planning_flow(self):
        sm = StateMachine()
        flow = [
            ProjectPhase.REPO_MAPPED,
            ProjectPhase.REQUIREMENTS_DRAFTED,
            ProjectPhase.PLANS_GENERATED,
            ProjectPhase.CRITIQUES_GENERATED,
            ProjectPhase.ARBITRATED,
            ProjectPhase.PLAN_APPROVED,
        ]
        for phase in flow:
            sm.transition(phase)
        assert sm.phase == ProjectPhase.PLAN_APPROVED

    def test_history_tracking(self):
        sm = StateMachine()
        sm.transition(ProjectPhase.REPO_MAPPED)
        sm.transition(ProjectPhase.REQUIREMENTS_DRAFTED)
        assert sm.history == [
            ProjectPhase.NEW,
            ProjectPhase.REPO_MAPPED,
            ProjectPhase.REQUIREMENTS_DRAFTED,
        ]

    def test_can_transition_check(self):
        sm = StateMachine()
        assert sm.can_transition(ProjectPhase.REPO_MAPPED) is True
        assert sm.can_transition(ProjectPhase.PROJECT_DONE) is False

    def test_valid_transitions_returns_set(self):
        sm = StateMachine()
        valid = sm.valid_transitions()
        assert ProjectPhase.REPO_MAPPED in valid
        assert len(valid) == 1

    def test_task_verification_fork(self):
        sm = StateMachine(initial=ProjectPhase.TASK_VERIFYING)
        assert sm.can_transition(ProjectPhase.TASK_VERIFIED) is True
        assert sm.can_transition(ProjectPhase.TASK_BLOCKED) is True

    def test_blocked_to_ready_repair_loop(self):
        sm = StateMachine(initial=ProjectPhase.TASK_BLOCKED)
        sm.transition(ProjectPhase.TASK_READY)
        assert sm.phase == ProjectPhase.TASK_READY

    def test_project_done_is_terminal(self):
        sm = StateMachine(initial=ProjectPhase.PROJECT_DONE)
        assert len(sm.valid_transitions()) == 0
        with pytest.raises(InvalidTransitionError):
            sm.transition(ProjectPhase.NEW)
