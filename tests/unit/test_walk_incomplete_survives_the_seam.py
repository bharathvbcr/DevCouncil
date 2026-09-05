"""A capped walk must not reach Python looking like a complete one.

The kernel distinguishes two different kinds of "you are not seeing everything":

* the *token budgeter* trimmed a complete set — reported through
  ``shown``/``hidden``/``total``/``truncated``, and
* the *walk itself* stopped early (depth or node cap) — reported through
  ``walk_incomplete``, because a walk that withheld an unknown quantity cannot
  be expressed in those counters without breaking ``shown + hidden == total``.

``BudgetedResponse`` carried the first and silently dropped the second. That
matters at the **default** depth, not some exotic one: an adversarial sweep
found the reverse walk routinely reports ``walk_incomplete`` with
``truncated: false`` and ``hidden: 0`` at depth 1 — the default in the CLI, in
this client, and in the IPC schema. So the common case delivered a partial
answer to Python wearing the shape of a complete one, which is the conflation
this codebase fails closed on everywhere else.
"""

from __future__ import annotations

import pytest

from devcouncil.devmap_client import BudgetedResponse, DevMapClient, DevMapClientError


def _client() -> DevMapClient:
    return DevMapClient.__new__(DevMapClient)


def _envelope(**overrides) -> dict:
    payload = {
        "shown": 1,
        "hidden": 0,
        "total": 1,
        "truncated": False,
        "tokens_used": 10,
        "items": [{"edge_kind": "Calls", "source_symbol": "a", "target_symbol": "b"}],
        "resolution": "Available",
    }
    payload.update(overrides)
    return payload


def test_the_field_exists_on_the_response_type():
    assert hasattr(BudgetedResponse(0, 0, 0, False, 0), "walk_incomplete"), (
        "BudgetedResponse cannot carry the kernel's walk-incompleteness signal, "
        "so every consumer sees a capped walk as a complete one"
    )


def test_a_capped_walk_keeps_its_reason_through_the_client():
    reason = "stopped at the depth limit after visiting 2000 nodes"
    parsed = _client()._budgeted(_envelope(walk_incomplete=reason), 2000)

    assert parsed.walk_incomplete == reason, (
        "the kernel said the walk stopped early and the client dropped it; "
        "the response now looks complete"
    )
    # And it must not be smuggled into the budget counters, which have their own
    # invariant the client enforces.
    assert parsed.truncated is False and parsed.hidden == 0


def test_a_complete_walk_carries_no_marker():
    parsed = _client()._budgeted(_envelope(), 2000)
    assert parsed.walk_incomplete is None, (
        "a complete walk was marked incomplete, which would make the signal "
        "meaningless"
    )


def test_the_budget_invariants_are_still_enforced():
    """The new field must not become an escape hatch from the existing checks."""
    with pytest.raises(DevMapClientError, match="count invariant"):
        _client()._budgeted(_envelope(shown=1, hidden=5, total=99, walk_incomplete="x"), 2000)
    with pytest.raises(DevMapClientError, match="truncation invariant"):
        _client()._budgeted(
            _envelope(shown=1, hidden=0, total=1, truncated=True, walk_incomplete="x"), 2000
        )
