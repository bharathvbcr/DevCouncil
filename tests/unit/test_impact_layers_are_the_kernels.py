"""`dev map impact` and `devcouncil_graph_impact` report the walk's own depths.

Two defects, one cause. Neither surface could ask the kernel *how far* a symbol
was, because `impact` returned a flat edge list and an edge carries two
endpoints and no hop count.

* `cli/commands/graph_cmd.py`'s kernel branch invented the answer. It ran a
  **depth-3** reverse walk and packed everything it returned into one band
  labelled ``depth: 1, confidence: "extracted"`` — a three-hop transitive
  dependent published as a direct, deterministically-resolved caller. It also
  dropped `truncated`, `hidden` and `walk_incomplete`, so a walk stopped by the
  node cap read as a complete one, and it emitted ``symbols: []`` for every
  path, which reads as "this file defines nothing" rather than "not measured".
* `integrations/mcp/handlers/map.py`'s `handle_graph_impact` did not ask at all.
  It was the last MCP graph tool still on `load_code_graph` — the retired
  engine's whole-graph read, measured elsewhere in this suite at 1.2 s and
  hundreds of MB per call, from a tool an agent reads as read-only.

The kernel now bands the walk it performs (`impact --layers`), and both surfaces
render that instead of deriving it.
"""

from __future__ import annotations

import asyncio
import json
import subprocess

import pytest

from devcouncil.cli.commands import graph_cmd
from devcouncil.integrations.mcp.handlers import map as mapmod
from tests.unit.graph_fixtures import kernel_graph


def _git(root, *args):
    subprocess.run(["git", *args], cwd=root, check=True, capture_output=True, text=True)


@pytest.fixture
def four_link_chain(tmp_path):
    """``a -> b -> c -> d``: one distinct caller at each of depths 1, 2 and 3.

    Built by the real kernel over real sources, so the depths under test are the
    ones a production walk computes rather than ones a fixture asserted.
    """
    files = {
        "pyproject.toml": '[project]\nname="t"\nversion="0"\n',
        "pkg/__init__.py": "",
        "pkg/d.py": "def d():\n    return 1\n",
        "pkg/c.py": "from pkg.d import d\n\n\ndef c():\n    return d()\n",
        "pkg/b.py": "from pkg.c import c\n\n\ndef b():\n    return c()\n",
        "pkg/a.py": "from pkg.b import b\n\n\ndef a():\n    return b()\n",
    }
    for rel, content in files.items():
        path = tmp_path / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")
    _git(tmp_path, "init")
    _git(tmp_path, "-c", "user.email=t@t", "-c", "user.name=t", "add", "-A")
    _git(tmp_path, "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", "init")
    kernel_graph(tmp_path)  # builds the store, or skips when no kernel is built
    return tmp_path


def _bands(payload: dict) -> dict[int, list[str]]:
    entry = (payload.get("paths") or [{}])[0]
    blast = entry.get("blast") or {}
    return {
        int(layer["depth"]): list(layer.get("nodes") or [])
        for layer in blast.get("layers") or []
    }


def test_impact_bands_a_deep_caller_at_the_depth_it_was_reached(four_link_chain):
    """The defect itself: everything a depth-3 walk found was called depth 1."""
    payload = graph_cmd._devmap_query_payload(
        four_link_chain, "impact", paths=["pkg/d.py"], max_depth=3
    )
    assert payload is not None, "the kernel must answer; there is no Python engine"
    bands = _bands(payload)

    assert bands, f"no bands at all in {json.dumps(payload)[:600]}"
    assert 2 in bands and bands[2], (
        "b reaches d through c and belongs at depth 2; the payload banded "
        f"{sorted(bands)}"
    )
    assert 3 in bands and bands[3], (
        "a reaches d through two hops and belongs at depth 3; the payload banded "
        f"{sorted(bands)}"
    )
    depth_one = " ".join(bands.get(1) or [])
    assert "pkg/a.py" not in depth_one, (
        "a is three hops from d and must not be reported as a direct caller; "
        f"depth 1 held {bands.get(1)}"
    )


def test_impact_reports_a_measured_confidence_not_a_depth_label(four_link_chain):
    """`confidence` must come from the edges, not from the band index.

    The payload this replaces set `confidence` from a depth table — depth 1
    "extracted", depth 2 "inferred", depth 3 "ambiguous" — so the field named a
    distance while reading as a claim about evidence. A band held together by
    deterministic call edges was reported as ambiguous for no reason but its
    distance, and a band held together by name-only attribution at depth 1 was
    reported as extracted.
    """
    payload = graph_cmd._devmap_query_payload(
        four_link_chain, "impact", paths=["pkg/d.py"], max_depth=3
    )
    layers = ((payload.get("paths") or [{}])[0].get("blast") or {}).get("layers") or []
    assert layers, payload
    for layer in layers:
        assert "confidence_score" in layer, (
            "each band must carry the weakest confidence among the edges that "
            f"reached it: {layer}"
        )
        score = layer["confidence_score"]
        assert score is None or 0.0 <= float(score) <= 1.0, layer


def test_impact_names_the_symbols_it_actually_seeded_from(four_link_chain):
    """`symbols: []` was emitted for every path, measured or not."""
    payload = graph_cmd._devmap_query_payload(
        four_link_chain, "impact", paths=["pkg/d.py"], max_depth=3
    )
    entry = (payload.get("paths") or [{}])[0]
    seeds = (entry.get("blast") or {}).get("seeds") or []
    assert any("pkg/d.py" in seed for seed in seeds), (
        f"the walk seeded from pkg/d.py and must say so: {entry}"
    )
    assert entry.get("symbols"), (
        "an empty symbol list reads as 'this file defines nothing'; it must "
        f"name what the walk started from: {entry}"
    )


def test_impact_carries_the_walks_own_qualifications(four_link_chain):
    """A capped walk must not be published in the shape of a complete one."""
    payload = graph_cmd._devmap_query_payload(
        four_link_chain, "impact", paths=["pkg/d.py"], max_depth=1
    )
    entry = (payload.get("paths") or [{}])[0]
    blast = entry.get("blast") or {}
    assert "walk_incomplete" in blast, (
        "the kernel says when a walk stopped early; dropping it publishes a "
        f"lower bound as a blast radius: {blast}"
    )
    assert blast.get("walk_incomplete"), (
        "a depth-1 walk over a four-link chain stopped early and must say so: "
        f"{blast}"
    )


def test_a_measured_zero_reads_as_measured(four_link_chain):
    """`pkg/a.py` is the head of the chain: nothing reaches it.

    The walk *ran* — it seeded from `pkg/a.py::a` and found no inbound edge — so
    the zero is a finding and must not carry an unavailability reason. This is
    the half that makes the next test's zero mean something.
    """
    payload = graph_cmd._devmap_query_payload(
        four_link_chain, "impact", paths=["pkg/a.py"], max_depth=3
    )
    entry = (payload.get("paths") or [{}])[0]
    assert entry.get("path") == "pkg/a.py", payload
    assert (entry.get("blast") or {}).get("seeds"), (
        f"the walk had a start and must name it: {entry}"
    )
    assert (entry.get("blast") or {}).get("total_impacted") == 0, entry
    assert entry.get("unavailable") is None, (
        f"a walk that ran and found nothing must not read as one that could not run: {entry}"
    )


def test_an_unindexed_path_is_answered_rather_than_falling_back(four_link_chain):
    """A path the generation holds nothing for is an answer, not a fallback.

    `resolution_unavailable_reason` was raised as a client error, `impact`
    returned `None`, and `None` meant "run it on the Python graph" — so asking
    about a path the kernel simply had not indexed triggered a whole-graph read
    out of `index.sqlite`. The reason is now rendered per path, and one
    unanswerable path no longer decides the engine for the whole command.
    """
    payload = graph_cmd._devmap_query_payload(
        four_link_chain, "impact", paths=["pkg/never_written.py"], max_depth=3
    )
    assert payload is not None, (
        "returning None here is what re-ran the question on the Python graph"
    )
    entry = (payload.get("paths") or [{}])[0]
    assert entry.get("path") == "pkg/never_written.py", payload
    assert (entry.get("blast") or {}).get("total_impacted") == 0, entry
    assert entry.get("unavailable"), (
        f"a zero nothing looked for must say so: {entry}"
    )


def test_mcp_graph_impact_does_not_read_the_python_graph(four_link_chain, monkeypatch):
    """The last MCP graph tool on `load_code_graph`.

    The patch that made that function raise is gone with the function itself;
    the whole-graph read that is left is `read_code_graph`, and this tool must
    not reach that either -- it asks the kernel for a banded walk.
    """
    monkeypatch.setattr(
        "devcouncil.indexing.graph.build.read_code_graph",
        lambda root: (_ for _ in ()).throw(
            AssertionError("handle_graph_impact read the whole graph in Python")
        ),
    )

    contents = asyncio.run(
        mapmod.handle_graph_impact(four_link_chain, {"paths": ["pkg/d.py"]})
    )
    payload = json.loads(contents[0].text)
    assert payload.get("ok") is True, payload
    assert payload.get("source") == "devmap", payload
    bands = _bands(payload)
    assert 2 in bands and bands[2], f"the MCP tool must carry the bands too: {bands}"


def test_mcp_graph_impact_without_a_kernel_is_an_error_not_a_python_answer(
    tmp_path, monkeypatch
):
    monkeypatch.setattr(
        graph_cmd, "_devmap_query_payload", lambda *a, **k: None, raising=False
    )
    contents = asyncio.run(mapmod.handle_graph_impact(tmp_path, {"paths": ["a.py"]}))
    payload = json.loads(contents[0].text)
    assert payload.get("ok") is False, payload
    assert "devmap" in json.dumps(payload).lower(), payload


def test_cli_impact_without_a_kernel_names_the_kernel(tmp_path, monkeypatch):
    from typer.testing import CliRunner

    monkeypatch.setattr(
        graph_cmd, "_devmap_query_payload", lambda *a, **k: None, raising=False
    )
    result = CliRunner().invoke(
        graph_cmd.app, ["impact", "a.py", "--project-root", str(tmp_path)]
    )
    assert result.exit_code == 1, result.output
    assert "devmap" in result.output.lower(), result.output
