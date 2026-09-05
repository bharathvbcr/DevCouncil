"""The composed `query` view must cost one kernel exchange, not two per definition.

`devcouncil_graph_query` was the last Dev Map read path that did not get faster
when the store did. Routing it to the Rust kernel changed nothing on its own
(1099 ms Python vs 1112 ms kernel) because the cost was never the language: the
view asks for callers *and* callees for each of the first five definitions, and
:class:`~devcouncil.devmap_client.DevMapClient` falls back to a ``devmap``
subprocess whenever no daemon socket is live. Eleven exchanges meant eleven
process spawns.

Measured on one settled store (gen 799, 14,189 nodes, 71,598 edges), same
process, same binary, min-of-5: the per-target fan-out cost **742.8 ms** and the
batched exchange **318.3 ms** — 2.33x — with byte-identical payloads.

So the tests that matter here are the ones that pin *call counts* and
*equivalence*, not a timing: a batch that answers something different is a
regression with a good benchmark.
"""

from __future__ import annotations

import pytest

from devcouncil.cli.commands import graph_cmd
from devcouncil.devmap_client import DevMapClientError


class _Resp:
    """Minimal stand-in for a BudgetedResponse the composition reads."""

    def __init__(self, items, resolution="Available", walk_incomplete=None):
        self.items = items
        self.resolution = resolution
        self.walk_incomplete = walk_incomplete


class _Client:
    """Records which kernel surface each direction went through."""

    def __init__(self, *, batched=True):
        self.batched = batched
        self.neighbor_calls: list[list[str]] = []
        self.single_calls: list[tuple[str, str]] = []

    def _edges(self, target):
        return [
            {"edge_kind": "Calls", "source_symbol": f"caller_of::{target}",
             "target_symbol": f"callee_of::{target}"}
        ]

    def neighbors(self, targets, depth=1, min_confidence=0.0):
        if not self.batched:
            raise DevMapClientError("kernel predates the neighbors command")
        self.neighbor_calls.append(list(targets))
        return [
            {"target": t, "callers": _Resp(self._edges(t)), "callees": _Resp(self._edges(t))}
            for t in targets
        ]

    def impact(self, target, depth=1):
        self.single_calls.append(("impact", target))
        return _Resp(self._edges(target))

    def deps(self, target, depth=1):
        self.single_calls.append(("deps", target))
        # Models the real kernel: `dependencies` resolves a FILE path, so a
        # symbol id comes back unavailable. A stub that answered here would let
        # the fallback's symbol handling go untested.
        if "::" in target:
            return _Resp([], resolution={"Unavailable": {"reason": f"{target} is not indexed"}})
        return _Resp(self._edges(target))

    def trace(self, target, depth=1, to_symbol=None):
        self.single_calls.append(("trace", target))
        return _Resp(self._edges(target))


TARGETS = ["a.py::f", "b.py::g", "c.py::h"]


def test_three_targets_cost_one_exchange_not_six():
    """The whole point of the change, stated as a call count.

    Fails against the pre-change code, which issued ``2 * len(targets)``
    separate calls.
    """
    client = _Client()
    graph_cmd._neighbor_edges(client, TARGETS)

    assert client.neighbor_calls == [TARGETS], (
        "the batched command was not used; per-target calls were "
        f"{client.single_calls}"
    )
    assert client.single_calls == [], (
        "the batch answered, yet per-target calls were still issued: "
        f"{client.single_calls}"
    )


def test_a_kernel_without_the_batched_command_still_answers():
    """An older `devmap` on PATH must degrade to slower, not to broken."""
    client = _Client(batched=False)
    answers = graph_cmd._neighbor_edges(client, TARGETS)

    assert sorted(answers) == sorted(TARGETS)
    # Every target here is symbol-shaped, so `deps` declines and the fallback
    # must reach for the symbol-scoped traversal, exactly as the kernel does.
    assert client.single_calls == [
        (method, target)
        for target in TARGETS
        for method in ("impact", "deps", "trace")
    ], f"the fallback did not issue the per-target calls: {client.single_calls}"


def test_the_two_paths_return_the_same_answer():
    """Equivalence is the property that makes the speed-up safe to ship.

    Load-bearing for the *symbol* case in particular: `deps` cannot resolve a
    symbol id, so a fallback that stopped there would report every symbol's
    callees as unknown while the batched path reported them. Two paths that
    quietly answer differently are worse than one that is merely slower — and
    this is red against a fallback that does not reach for the traversal.
    """
    batched = graph_cmd._neighbor_edges(_Client(batched=True), TARGETS)
    per_target = graph_cmd._neighbor_edges(_Client(batched=False), TARGETS)
    assert batched == per_target

    _, _, callees, callees_unavailable = per_target[TARGETS[0]]
    assert callees and callees_unavailable is None, (
        "the fallback left a symbol's callees unknown; the kernel answers them"
    )


def test_an_unavailable_direction_is_none_not_empty():
    """A direction the kernel could not resolve must not read as 'no callers'.

    ``None`` and ``[]`` mean different things here, and conflating them is what
    gets a live function deleted.
    """
    client = _Client()
    client.neighbors = lambda targets, depth=1, min_confidence=0.0: [
        {
            "target": targets[0],
            "callers": _Resp([], resolution={"Unavailable": {"reason": "not indexed"}}),
            "callees": _Resp([]),
        }
    ]
    callers, callers_unavailable, callees, callees_unavailable = graph_cmd._neighbor_edges(
        client, [TARGETS[0]]
    )[TARGETS[0]]

    assert callers is None, "an unresolvable direction came back as an empty list"
    assert callers_unavailable and "not indexed" in callers_unavailable
    assert callees == [] and callees_unavailable is None, (
        "a genuinely empty direction must stay empty, not become unknown"
    )


def test_the_unavailable_wording_names_the_kernel_command():
    """The batched and per-target paths must be indistinguishable to a reader.

    Both reasons say ``impact``/``deps`` — the kernel command — so which code
    path answered never leaks into a message describing the store. Red against
    a batch path that names the direction (`callers`/`callees`) instead.
    """
    client = _Client()
    client.neighbors = lambda targets, depth=1, min_confidence=0.0: [
        {
            "target": targets[0],
            "callers": _Resp([], resolution={"Unavailable": {"reason": "not indexed"}}),
            "callees": _Resp([], resolution={"Unavailable": {"reason": "not indexed"}}),
        }
    ]
    _, callers_unavailable, _, callees_unavailable = graph_cmd._neighbor_edges(
        client, [TARGETS[0]]
    )[TARGETS[0]]

    assert callers_unavailable.startswith("impact resolution unavailable:"), (
        callers_unavailable
    )
    assert callees_unavailable.startswith("deps resolution unavailable:"), (
        callees_unavailable
    )


def test_an_empty_target_list_asks_the_kernel_nothing():
    client = _Client()
    assert graph_cmd._neighbor_edges(client, []) == {}
    assert client.neighbor_calls == [] and client.single_calls == []


def test_a_misaligned_batch_answer_is_refused_by_the_client():
    """The client must not attribute one symbol's callers to another.

    The kernel echoes each target back; if that ever drifts, silently zipping
    request to response would mislabel every entry after the drift.
    """
    from devcouncil.devmap_client import DevMapClient

    client = DevMapClient.__new__(DevMapClient)
    client._request = lambda payload, args: {
        "neighbors": [{"target": "WRONG", "callers": {}, "callees": {}}]
    }
    with pytest.raises(DevMapClientError, match="misaligned"):
        client.neighbors(["a.py::f"])


def test_a_short_batch_answer_is_refused_by_the_client():
    from devcouncil.devmap_client import DevMapClient

    client = DevMapClient.__new__(DevMapClient)
    client._request = lambda payload, args: {"neighbors": []}
    with pytest.raises(DevMapClientError, match="1 targets|for 1 targets"):
        client.neighbors(["a.py::f"])


class _OldClient:
    """A client from before the batched command existed.

    Not a contrived stub: `DevMapClient` gained `neighbors` in this change, and
    anything holding an older instance — a partially upgraded install, a test
    double written against the previous surface — presents exactly this shape.
    """

    def __init__(self):
        self.calls: list[tuple[str, str]] = []

    def impact(self, target, depth=1):
        self.calls.append(("impact", target))
        return _Resp([{"edge_kind": "Calls", "source_symbol": f"caller_of::{target}"}])

    def deps(self, target, depth=1):
        self.calls.append(("deps", target))
        return _Resp([{"edge_kind": "Calls", "target_symbol": f"callee_of::{target}"}])


def test_a_client_without_the_batched_command_falls_back_instead_of_crashing():
    """The absence of the method must route around it, not raise.

    The first version of `_neighbor_edges` called `client.neighbors(...)` inside
    a `try/except DevMapClientError`, which does not catch `AttributeError`. A
    client lacking the method — the very "partial upgrade" case the fallback
    exists for — took down the whole query instead. Red against that version.
    """
    client = _OldClient()
    answers = graph_cmd._neighbor_edges(client, TARGETS)

    assert sorted(answers) == sorted(TARGETS)
    assert client.calls == [
        (method, target) for target in TARGETS for method in ("impact", "deps")
    ], f"the fallback did not run: {client.calls}"
    callers, callers_unavailable, callees, callees_unavailable = answers[TARGETS[0]]
    assert callers and callees, "the fallback produced no edges"
    assert callers_unavailable is None and callees_unavailable is None


def test_a_client_without_trace_keeps_the_reason_deps_gave():
    """A missing `trace` must not raise, and must not erase the known reason."""

    class _NoTrace(_OldClient):
        def deps(self, target, depth=1):
            self.calls.append(("deps", target))
            return _Resp([], resolution={"Unavailable": {"reason": "not indexed"}})

    client = _NoTrace()
    _, _, callees, callees_unavailable = graph_cmd._neighbor_edges(client, [TARGETS[0]])[
        TARGETS[0]
    ]
    assert callees is None
    assert "not indexed" in callees_unavailable, callees_unavailable


def test_an_empty_list_from_a_capped_walk_is_not_reported_as_none():
    """The case the dropped signal actually endangered.

    A walk that hit its depth or node cap and found nothing yet reports
    `walk_incomplete` with `truncated: false` and `hidden: 0` — the counters
    cannot express it. Rendered as `[]`, that reads as "nothing calls this",
    which is the reading that gets a live function deleted.
    """
    client = _Client()
    client.neighbors = lambda targets, depth=1, min_confidence=0.0: [
        {
            "target": targets[0],
            "callers": _Resp([]),
            "callees": _Resp([]),
        }
    ]
    # Only the callers side stopped early; the callees side genuinely has none.
    def _neighbors(targets, depth=1, min_confidence=0.0):
        callers = _Resp([])
        callers.walk_incomplete = "stopped at the depth limit after 2000 nodes"
        callees = _Resp([])
        callees.walk_incomplete = None
        return [{"target": targets[0], "callers": callers, "callees": callees}]

    client.neighbors = _neighbors
    callers, callers_unavailable, callees, callees_unavailable = graph_cmd._neighbor_edges(
        client, [TARGETS[0]]
    )[TARGETS[0]]

    assert callers is None, "a capped walk's empty list was published as a real answer"
    assert "walk incomplete" in callers_unavailable and "depth limit" in callers_unavailable
    assert callees == [] and callees_unavailable is None, (
        "a genuinely empty direction must stay empty, not be marked unknown"
    )
