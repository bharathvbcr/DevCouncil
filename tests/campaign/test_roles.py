"""Role hierarchy + machine-enforced role-hierarchy boundaries."""

from __future__ import annotations

import pytest

from devcouncil.campaign.roles import (
    ROLES,
    Action,
    ForbiddenActionError,
    Rank,
    assert_allowed,
    get_role,
    load_role_instructions,
)


def test_every_rank_has_a_role():
    assert set(ROLES) == set(Rank)


def test_chain_of_command():
    assert get_role(Rank.DIRECTOR).reports_to is None
    assert get_role(Rank.COORDINATOR).reports_to is Rank.DIRECTOR
    assert get_role(Rank.WORKER).reports_to is Rank.REVIEWER
    assert get_role(Rank.REVIEWER).reports_to is Rank.COORDINATOR


@pytest.mark.parametrize(
    "rank,action",
    [
        (Rank.DIRECTOR, Action.EXECUTE_TASK),      # the Director never toils
        (Rank.DIRECTOR, Action.WRITE_DASHBOARD),   # only the Coordinator writes it
        (Rank.COORDINATOR, Action.EXECUTE_TASK),        # the Coordinator dispatches, never implements
        (Rank.COORDINATOR, Action.QC_REVIEW),           # QC belongs to the Reviewer
        (Rank.WORKER, Action.QC_REVIEW),       # a worker never reviews its own work
        (Rank.WORKER, Action.CONTACT_HUMAN),   # only Director/Coordinator reach the operator
        (Rank.REVIEWER, Action.EXECUTE_TASK),      # a thinker, not a doer
        (Rank.REVIEWER, Action.ASSIGN),            # the Reviewer never commands Worker
    ],
)
def test_forbidden_actions_raise(rank, action):
    with pytest.raises(ForbiddenActionError):
        assert_allowed(rank, action)


@pytest.mark.parametrize(
    "rank,action",
    [
        (Rank.DIRECTOR, Action.RELAY_ORDER),
        (Rank.COORDINATOR, Action.WRITE_DASHBOARD),
        (Rank.COORDINATOR, Action.ASSIGN),
        (Rank.WORKER, Action.EXECUTE_TASK),
        (Rank.REVIEWER, Action.QC_REVIEW),
    ],
)
def test_permitted_actions_pass(rank, action):
    assert_allowed(rank, action)  # must not raise


def test_forbidden_is_complement_of_allowed():
    role = get_role(Rank.WORKER)
    assert role.allowed.isdisjoint(role.forbidden)
    assert role.allowed | role.forbidden == set(Action)


def test_load_instructions_returns_markdown():
    text = load_role_instructions(Rank.COORDINATOR)
    assert "Coordinator" in text
    assert text.strip()
