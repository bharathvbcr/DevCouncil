"""**R9.** ``min_rung`` stopped at the kernel.

W2.3's point was that an agent should be able to ask for deterministic-only
edges. The Rust CLI has taken ``--min-rung`` on ``deps``, ``impact`` and
``trace`` since it landed, and ``IpcCommand`` takes ``min_rung`` on all three —
and **no Python or MCP surface exposed one**, so the caller the feature exists
for still could not use it. A capability nothing can reach is a capability
nothing keeps working.

These tests pin the whole chain in one direction each: the client sends the
floor on both transports, the MCP tools declare it, and both refuse an unknown
name rather than answering at full breadth. That last one is the honesty half —
a typo silently dropped produces a *filtered* answer the caller believes is
narrow, which is the reading that gets acted on.
"""

from __future__ import annotations

import json
from typing import Any

import pytest

from devcouncil.devmap_client import (
    MIN_RUNG_NAMES,
    DevMapClient,
    DevMapClientError,
)
from devcouncil.integrations.mcp.handlers import codeintel


class _RecordingClient(DevMapClient):
    """A client that records what it would have sent, on both transports.

    Subclassed rather than mocked so the argument assembly under test is the
    real one: a mock of ``_request`` would let a method build its payload any
    way at all and still pass.
    """

    def __init__(self) -> None:  # noqa: D107 - deliberately skips DevMapClient.__init__
        self.payloads: list[dict[str, Any]] = []
        self.argvs: list[list[str]] = []

    def _request(self, payload: dict[str, Any], args: list[str]) -> dict[str, Any]:
        self.payloads.append(payload)
        self.argvs.append(args)
        empty = {
            "shown": 0,
            "hidden": 0,
            "total": 0,
            "truncated": False,
            "tokens_used": 0,
            "items": [],
        }
        if payload.get("cmd") == "neighbors":
            # The composed shape, so `neighbors` validates its answer the way
            # it would against the kernel rather than tripping on the fake.
            return {
                "neighbors": [
                    {"target": target, "callers": dict(empty), "callees": dict(empty)}
                    for target in payload["targets"]
                ]
            }
        return empty


@pytest.mark.parametrize("rung", MIN_RUNG_NAMES)
def test_every_edge_walking_method_sends_the_floor_on_both_transports(rung: str) -> None:
    for call in (
        lambda c: c.deps("a.py::f", min_rung=rung),
        lambda c: c.impact("a.py::f", min_rung=rung),
        lambda c: c.trace("a.py::f", to_symbol="b.py::g", min_rung=rung),
    ):
        client = _RecordingClient()
        call(client)
        payload = client.payloads[-1]
        argv = client.argvs[-1]
        assert payload.get("min_rung") == rung, (
            f"the IPC payload for {payload.get('cmd')!r} must carry the floor: {payload}"
        )
        assert "--min-rung" in argv and rung in argv, (
            f"the CLI fallback for {payload.get('cmd')!r} must carry it too: {argv}"
        )
        # The two transports answer the same question; a floor honoured on one
        # and dropped on the other is a client whose answer depends on whether a
        # daemon happens to be running.
        assert argv[argv.index("--min-rung") + 1] == rung


def test_omitting_the_floor_changes_nothing_for_existing_callers() -> None:
    for call in (
        lambda c: c.deps("a.py::f"),
        lambda c: c.impact("a.py::f"),
        lambda c: c.trace("a.py::f", to_symbol="b.py::g"),
    ):
        client = _RecordingClient()
        call(client)
        assert "min_rung" not in client.payloads[-1]
        assert "--min-rung" not in client.argvs[-1]


@pytest.mark.parametrize("bad", ["exact", "DETERMINISTIC", "", "1.0", "none"])
def test_an_unknown_floor_is_refused_rather_than_dropped(bad: str) -> None:
    for call in (
        lambda c: c.deps("a.py::f", min_rung=bad),
        lambda c: c.impact("a.py::f", min_rung=bad),
        lambda c: c.trace("a.py::f", to_symbol="b.py::g", min_rung=bad),
    ):
        client = _RecordingClient()
        with pytest.raises(DevMapClientError, match="min_rung"):
            call(client)
        assert not client.payloads, (
            "a refused floor must not reach the kernel at all; sending the "
            "request without it would answer at full breadth"
        )


def test_the_mcp_tools_declare_the_floor_they_now_accept() -> None:
    by_name = {tool.name: tool for tool in codeintel.tools()}
    for name in ("devcouncil_code_path", "devcouncil_code_impact"):
        schema = by_name[name].input_schema
        prop = schema["properties"].get("minRung")
        assert prop is not None, f"{name} must declare minRung: {sorted(schema['properties'])}"
        assert set(prop["enum"]) == set(MIN_RUNG_NAMES), (
            f"{name}'s enum must be the kernel's rung names: {prop['enum']}"
        )
        assert "minRung" not in schema.get("required", []), (
            "the floor is optional; requiring it would change every existing call"
        )


def test_the_mcp_argument_reader_refuses_an_unknown_floor() -> None:
    assert codeintel._min_rung_of({}) is None
    for rung in MIN_RUNG_NAMES:
        assert codeintel._min_rung_of({"minRung": rung}) == rung
    with pytest.raises(ValueError, match="minRung"):
        codeintel._min_rung_of({"minRung": "exact"})


def test_the_client_rung_names_match_the_kernels() -> None:
    """The two lists live in different processes and must not drift.

    Read out of the Rust source rather than restated here: a second hand-written
    copy of a vocabulary is exactly the shape ``CALL_EXTRACTION_LANGUAGES``
    rotted in.
    """
    from pathlib import Path

    rung_rs = (
        Path(__file__).resolve().parents[2]
        / "rust-port"
        / "crates"
        / "devmap-query"
        / "src"
        / "rung.rs"
    )
    if not rung_rs.exists():  # pragma: no cover - source tree only
        pytest.skip("kernel source is not present in this checkout")
    text = rung_rs.read_text(encoding="utf-8")
    # `label()` is the wire name and the one the histogram buckets under.
    labels = {
        line.split("=>")[1].strip().strip(',').strip('"')
        for line in text.splitlines()
        if "=>" in line and "Rung::" in line and line.strip().endswith('",')
    }
    assert labels, "the label arms could not be read; this guard would pass vacuously"
    assert labels == set(MIN_RUNG_NAMES), (
        f"the client's rung names {sorted(MIN_RUNG_NAMES)} disagree with the "
        f"kernel's {sorted(labels)}"
    )


def test_dead_takes_no_floor_and_the_reason_is_recorded() -> None:
    """``dead`` deliberately has no ``min_rung``, unlike its three siblings.

    Not an omission. The liveness verdict is computed at build time over the
    whole edge set and persisted as rows; applying a rung floor would mean
    re-deciding which symbols have callers using only deterministic edges, which
    is a full re-analysis at query time and unbounded by the token budget the
    surface promises. A flag that accepted the argument and ignored it would be
    worse than its absence.

    Pinned so the asymmetry stays a decision. If ``dead`` ever grows a floor,
    this test fails and whoever added it has to say what it means.
    """
    client = _RecordingClient()
    client.dead_symbols(budget=2000)
    assert "min_rung" not in client.payloads[-1]
    assert "--min-rung" not in client.argvs[-1]

    schema = {tool.name: tool for tool in codeintel.tools()}[
        "devcouncil_code_dead"
    ].input_schema
    assert "minRung" not in schema["properties"], (
        "dead's verdict is persisted, not walked; a floor here would have to "
        "re-run liveness at query time"
    )


def test_the_dead_tool_description_names_both_populations() -> None:
    """R8's other half: neither tool description mentioned clusters."""
    tool = {t.name: t for t in codeintel.tools()}["devcouncil_code_dead"]
    assert "dead_clusters" in tool.description
    assert "one-hop" in tool.description, (
        "the description must say *why* a second population exists, not just "
        "that it does"
    )


def test_the_client_carries_clusters_and_keeps_absent_apart_from_empty() -> None:
    """R8 at the client boundary."""
    client = _RecordingClient()

    def respond(extra: dict[str, Any]) -> Any:
        base = {
            "shown": 0,
            "hidden": 0,
            "total": 0,
            "truncated": False,
            "tokens_used": 0,
            "items": [],
        }
        base.update(extra)
        return client._budgeted(base, 2000)

    assert respond({}).dead_clusters is None, (
        "a response with no cluster key says the pass did not run"
    )
    assert respond({"dead_clusters": []}).dead_clusters == [], (
        "an empty list says it ran and found none, which is a finding"
    )
    populated = respond(
        {"dead_clusters": [{"size": 3, "members": ["a", "b", "c"]}],
         "dead_clusters_truncated": 7}
    )
    assert populated.dead_clusters[0]["size"] == 3
    assert populated.dead_clusters_truncated == 7

    with pytest.raises(DevMapClientError, match="dead_clusters"):
        respond({"dead_clusters": "not a list"})
    with pytest.raises(DevMapClientError, match="dead_clusters"):
        respond({"dead_clusters": [["not", "an", "object"]]})


def test_a_refused_scan_is_a_third_state_and_never_an_empty_finding() -> None:
    """R8's honesty hole: the scan has three outcomes and the pair holds two.

    ``DeadClusterScan`` carries ``refused_oversized_graph``. Past the kernel's
    node ceiling the walk does not run and ``clusters`` is empty *because
    nothing looked*, so a field-for-field mapping renders "too large to walk" as
    ``[]`` — the pass ran and found no abandoned subsystems. That is the
    strongest reading of the weakest evidence, and the reading a caller acts on.

    The kernel therefore sends no list at all and puts the reason beside it.
    Asserted here at the client, which is the last place a wrong shape can be
    caught before it becomes a rendered answer.
    """
    client = _RecordingClient()

    def respond(extra: dict[str, Any]) -> Any:
        base = {
            "shown": 0,
            "hidden": 0,
            "total": 0,
            "truncated": False,
            "tokens_used": 0,
            "items": [],
        }
        base.update(extra)
        return client._budgeted(base, 2000)

    refused = respond({"dead_clusters_incomplete": "the call graph exceeded 400000 symbols"})
    assert refused.dead_clusters is None, (
        "a refusal must not arrive holding a list of any length"
    )
    assert "400000" in (refused.dead_clusters_incomplete or ""), (
        "and must name the ceiling, so a reader can tell whether their graph is "
        "near it"
    )

    # OFF direction: an ordinary answer carries no caveat. A reason set
    # unconditionally would put one on every response, which is how a caveat
    # stops being read on the response that needs it.
    assert respond({"dead_clusters": []}).dead_clusters_incomplete is None
    assert respond({}).dead_clusters_incomplete is None

    # Both at once is the kernel contradicting itself. Refused rather than
    # resolved, because either choice would be this client's guess presented as
    # the kernel's answer.
    with pytest.raises(DevMapClientError, match="dead_clusters_incomplete"):
        respond({"dead_clusters": [], "dead_clusters_incomplete": "too large"})
    with pytest.raises(DevMapClientError, match="dead_clusters_incomplete"):
        respond({"dead_clusters_incomplete": 400_000})


def test_the_graph_schema_keeps_absent_clusters_apart_from_none_found() -> None:
    """R8: ``CodeGraph`` had no field, so pydantic dropped every cluster."""
    from devcouncil.indexing.graph.schema import CodeGraph

    absent = CodeGraph.model_validate({"nodes": [], "edges": []})
    assert absent.dead_clusters is None, (
        "a graph written before the pass existed did not run it; an empty list "
        "would say it ran"
    )

    payload = json.loads(
        json.dumps({"nodes": [], "edges": [], "dead_clusters": [
            {"cluster_id": 0, "members": ["a.py::f"], "size": 1,
             "confidence": 0.5, "reason": "cycle"}
        ], "dead_clusters_truncated": 2})
    )
    loaded = CodeGraph.model_validate(payload)
    assert loaded.dead_clusters is not None
    assert loaded.dead_clusters[0]["members"] == ["a.py::f"]
    assert loaded.dead_clusters_truncated == 2
    assert loaded.dead_clusters_incomplete is None

    # And the refusal survives the same round trip. Without a field for it
    # pydantic drops the key, and a `code_graph.json` recording "too large to
    # walk" loads as one that simply predates the pass — which tells the reader
    # to rebuild, and the rebuild refuses again.
    refused = CodeGraph.model_validate(
        {"nodes": [], "edges": [], "dead_clusters_incomplete": "graph too large"}
    )
    assert refused.dead_clusters is None
    assert refused.dead_clusters_incomplete == "graph too large"


def test_the_mcp_dead_envelope_carries_a_refusal_as_a_boundary(tmp_path, monkeypatch) -> None:
    """The refusal has to reach the key an agent is told to read.

    ``epistemic.boundaries`` is where this tool's own description sends the
    reader for what the answer could not cover. A class of finding that was not
    computed at all belongs there beside the coverage gaps — carrying the reason
    only as an extra payload key would leave the verdict reading ``exact``, with
    the caveat somewhere the agent was never told to look.
    """
    from devcouncil.devmap_client import BudgetedResponse

    class _Status:
        is_fresh = True
        generation_id = 7
        pending_count = 0
        degraded_reason = ""
        coverage_gaps: dict[str, Any] = {}
        raw: dict[str, Any] = {"schema_version": 17, "analyzer_version": "test"}

    def response(**extra: Any) -> BudgetedResponse:
        return BudgetedResponse(
            shown=0, hidden=0, total=0, truncated=False, tokens_used=0,
            items=[], resolution=None, **extra,
        )

    class _Client:
        def __init__(self, resp: BudgetedResponse) -> None:
            self._resp = resp

        def dead_symbols(self, budget: int = 2000) -> BudgetedResponse:
            return self._resp

        def status(self) -> _Status:
            return _Status()

    def envelope(resp: BudgetedResponse) -> dict[str, Any]:
        monkeypatch.setattr(codeintel, "try_connect", lambda root: _Client(resp))
        return codeintel._dead_via_client(tmp_path, "inferred")

    refused = envelope(
        response(dead_clusters_incomplete="the call graph exceeded 400000 symbols")
    )
    assert refused["dead_clusters"] is None, (
        "an empty list would say the component pass ran and found nothing"
    )
    assert refused["dead_clusters_incomplete"] == (
        "the call graph exceeded 400000 symbols"
    )
    assert any("400000" in boundary for boundary in refused["boundaries"]), (
        f"the refusal must be named as a boundary: {refused['boundaries']}"
    )
    assert refused["epistemic"] != "exact", (
        "an answer missing an entire class of finding is not exact: "
        f"{refused['epistemic']}"
    )

    # OFF direction: a scan that ran adds no boundary and stays exact.
    ran = envelope(response(dead_clusters=[]))
    assert ran["dead_clusters"] == []
    assert ran["dead_clusters_incomplete"] is None
    assert not any("400000" in boundary for boundary in ran["boundaries"])
    assert ran["epistemic"] == "exact", (
        f"nothing was withheld from this answer: {ran}"
    )


def test_the_artifact_and_the_model_declare_the_same_top_level_keys() -> None:
    """``code_graph.json``'s writer and ``CodeGraph`` must not drift apart.

    The kernel asserts its own key set twice — in the writer's unit test and in
    the CLI artifact test — both against a real artifact, and both formerly with
    their own hand-written copy of the list. Nothing checked the Python half, so
    a field added to the pydantic model and not to the writer loads as its
    default on every artifact: for ``dead_clusters`` that default was ``None``,
    and for ``dead_clusters_incomplete`` it would have silently swallowed the
    refusal that whole change exists to surface. A missing key does not fail
    anywhere; it just quietly answers something else.

    The three copies are now one — ``CODE_GRAPH_TOP_LEVEL_KEYS`` — and this
    reads it, for the reason ``test_the_client_rung_names_match_the_kernels``
    gives: a second hand-written copy of a vocabulary is a second thing to keep
    in step, and it is always the copy nobody edited that is right.
    """
    import re
    from pathlib import Path

    from devcouncil.indexing.graph.schema import CodeGraph

    source = (
        Path(__file__).resolve().parents[2]
        / "rust-port"
        / "crates"
        / "devmap-query"
        / "src"
        / "code_graph.rs"
    )
    if not source.exists():  # pragma: no cover - source tree only
        pytest.skip("kernel source is not present in this checkout")
    text = source.read_text(encoding="utf-8")
    marker = "pub const CODE_GRAPH_TOP_LEVEL_KEYS: &[&str] = &["
    assert marker in text, (
        "the kernel's key declaration has moved or been renamed; this guard "
        "would pass vacuously"
    )
    body = text[text.index(marker) + len(marker) : text.index("];", text.index(marker))]
    declared = set(re.findall(r'"([a-z_]+)"', body))
    assert declared, f"no keys parsed out of the kernel declaration: {body!r}"
    assert declared == set(CodeGraph.model_fields), (
        "the artifact writer and CodeGraph disagree; keys only the kernel "
        f"writes: {sorted(declared - set(CodeGraph.model_fields))}, fields only "
        f"the model declares: {sorted(set(CodeGraph.model_fields) - declared)}"
    )


@pytest.mark.parametrize("rung", MIN_RUNG_NAMES)
def test_the_composed_neighbors_query_carries_the_floor_too(rung: str) -> None:
    """A composition must accept every filter its parts accept.

    ``neighbors`` *is* ``impact`` and ``deps`` answered in one exchange, and it
    pinned ``min_rung`` to nothing while both halves took a floor — so
    ``dev map query``, whose edge lists come from this command, could not be
    asked for deterministic-only edges even though the two queries behind it
    could.

    Third parameter this fan-out has pinned, and the other two were both
    defects: ``min_confidence`` hardcoded on the inbound side, and ``max_depth``
    pinned to 1 where the composition test compared at depth 1 and could not
    see it. A dropped filter answers the *broader* question, which is the one
    reading a caller never guards against.
    """
    client = _RecordingClient()
    client.neighbors(["a.py::f", "b.py::g"], min_rung=rung)
    payload = client.payloads[-1]
    argv = client.argvs[-1]
    assert payload["min_rung"] == rung, f"the IPC payload must carry it: {payload}"
    assert argv[argv.index("--min-rung") + 1] == rung, (
        f"and so must the CLI fallback, or the answer's breadth depends on "
        f"whether a daemon is running: {argv}"
    )
    # The flag takes a value, so it must not sit between the command and its
    # positional targets in a way that swallows one.
    assert argv.index("--min-rung") < argv.index("a.py::f")
    assert "b.py::g" in argv


def test_omitting_the_floor_leaves_the_composed_query_untouched() -> None:
    """The OFF direction, at the shape every existing caller sends."""
    client = _RecordingClient()
    client.neighbors(["a.py::f"])
    assert "min_rung" not in client.payloads[-1]
    assert "--min-rung" not in client.argvs[-1]


@pytest.mark.parametrize("bad", ["exact", "DETERMINISTIC", "", "1.0"])
def test_the_composed_query_refuses_an_unknown_floor(bad: str) -> None:
    client = _RecordingClient()
    with pytest.raises(DevMapClientError, match="min_rung"):
        client.neighbors(["a.py::f"], min_rung=bad)
    assert not client.payloads, (
        "a refused floor must not reach the kernel; the request would come "
        "back at full breadth and read as the narrow answer that was asked for"
    )


def test_the_kernel_accepts_the_floor_on_every_command_the_client_sends_it_on() -> None:
    """The two processes must agree about which commands take a floor.

    Read out of the kernel's own dispatcher rather than restated: ``deps``,
    ``impact``, ``trace`` and ``neighbors`` are listed in one ``match`` arm in
    ``protocol.rs`` — the arm whose comment says a command that gains the
    parameter and forgets the validation should be one edit, not two. A client
    sending ``min_rung`` on a command that arm does not name would have it
    accepted unvalidated or ignored outright.
    """
    import re
    from pathlib import Path

    protocol = (
        Path(__file__).resolve().parents[2]
        / "rust-port"
        / "crates"
        / "devmap-serve"
        / "src"
        / "protocol.rs"
    )
    if not protocol.exists():  # pragma: no cover - source tree only
        pytest.skip("kernel source is not present in this checkout")
    text = protocol.read_text(encoding="utf-8")
    marker = "fn request_min_rung("
    assert marker in text, "the kernel's rung-bearing command list has moved"
    end = text.index(chr(10) + "}", text.index(marker))
    body = text[text.index(marker) : end]
    accepted = {
        name.lower() for name in re.findall(r"IpcCommand::(\w+) \{ min_rung", body)
    }
    assert accepted, f"no commands parsed out of the kernel arm: {body!r}"

    sent = set()
    for name in ("deps", "impact", "trace", "neighbors"):
        client = _RecordingClient()
        if name == "neighbors":
            client.neighbors(["a.py::f"], min_rung="deterministic")
        elif name == "trace":
            client.trace("a.py::f", to_symbol="b.py::g", min_rung="deterministic")
        else:
            getattr(client, name)("a.py::f", min_rung="deterministic")
        if "min_rung" in client.payloads[-1]:
            sent.add(name)
    assert sent == accepted, (
        f"the client sends a floor on {sorted(sent)} and the kernel validates "
        f"it on {sorted(accepted)}; a command on one list and not the other "
        "either ignores the floor or accepts an unchecked one"
    )
