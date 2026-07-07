"""Role hierarchy + machine-enforced chain-of-command boundaries."""

from __future__ import annotations

import pytest

from devcouncil.shogun.roles import (
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
    assert get_role(Rank.SHOGUN).reports_to is None
    assert get_role(Rank.KARO).reports_to is Rank.SHOGUN
    assert get_role(Rank.ASHIGARU).reports_to is Rank.GUNSHI
    assert get_role(Rank.GUNSHI).reports_to is Rank.KARO


@pytest.mark.parametrize(
    "rank,action",
    [
        (Rank.SHOGUN, Action.EXECUTE_TASK),      # the Shogun never toils
        (Rank.SHOGUN, Action.WRITE_DASHBOARD),   # only the Karo writes it
        (Rank.KARO, Action.EXECUTE_TASK),        # the Karo dispatches, never implements
        (Rank.KARO, Action.QC_REVIEW),           # QC belongs to the Gunshi
        (Rank.ASHIGARU, Action.QC_REVIEW),       # a worker never reviews its own work
        (Rank.ASHIGARU, Action.CONTACT_HUMAN),   # only Shogun/Karo reach the Lord
        (Rank.GUNSHI, Action.EXECUTE_TASK),      # a thinker, not a doer
        (Rank.GUNSHI, Action.ASSIGN),            # the Gunshi never commands Ashigaru
    ],
)
def test_forbidden_actions_raise(rank, action):
    with pytest.raises(ForbiddenActionError):
        assert_allowed(rank, action)


@pytest.mark.parametrize(
    "rank,action",
    [
        (Rank.SHOGUN, Action.RELAY_ORDER),
        (Rank.KARO, Action.WRITE_DASHBOARD),
        (Rank.KARO, Action.ASSIGN),
        (Rank.ASHIGARU, Action.EXECUTE_TASK),
        (Rank.GUNSHI, Action.QC_REVIEW),
    ],
)
def test_permitted_actions_pass(rank, action):
    assert_allowed(rank, action)  # must not raise


def test_forbidden_is_complement_of_allowed():
    role = get_role(Rank.ASHIGARU)
    assert role.allowed.isdisjoint(role.forbidden)
    assert role.allowed | role.forbidden == set(Action)


def test_load_instructions_returns_markdown():
    text = load_role_instructions(Rank.KARO)
    assert "Karo" in text
    assert text.strip()
