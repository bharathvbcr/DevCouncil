"""Confidence-tier arithmetic for dead-code and liveness answers.

What is left after W3.3. ``file_liveness`` — the Python entry-root, unwired and
unreachable scan — was retired here: it duplicated
``devmap-extract/src/wiring.rs`` and ``devmap-query``'s ``unwired_candidates``
in a second language, and two implementations of "is this file wired" can and
did disagree. The kernel decides now, and the two rules this side held that the
kernel lacked — the ``devcouncil: allow-unwired`` marker and dynamic-import
clearing — were ported into ``wiring.rs`` first, under a parity test that
measures both implementations against the same corpus.

The two thresholds that scan used to decide whether its unreachable set was
trustworthy went with it: ``liveness_unreachable_unreliable`` is now the
kernel's call, out of ``dead_clusters.refused_oversized_graph``
(``devmap-query/src/code_graph.rs:970``, ``manifest.rs:454``).

The tier helpers below stay: three live callers (``map.py`` twice,
``graph_cmd.py``), pure arithmetic over ``Confidence``, and nothing in the
kernel to duplicate.
"""

from __future__ import annotations

from devcouncil.indexing.graph.schema import (
    Confidence,
)

_CONFIDENCE_RANK = {
    Confidence.AMBIGUOUS: 0,
    Confidence.INFERRED: 1,
    Confidence.EXTRACTED: 2,
    "ambiguous": 0,
    "inferred": 1,
    "extracted": 2,
}


def confidence_label(conf: object) -> str:
    """The tier as a string, whether an entry carries the enum or a raw value.

    Two producers reach the dead-code report: the Rust kernel's rows, whose
    ``confidence`` is whatever JSON held (a string, or absent), and the Python
    graph's entries, which carry an enum. Unwrapping that in each consumer is
    how the two start disagreeing about what ``ambiguous`` is called, so it is
    unwrapped here, once.
    """
    return str(getattr(conf, "value", conf))


def confidence_at_least(conf: object, minimum: str) -> bool:
    """True when ``conf`` ranks at or above ``minimum`` (extracted > inferred > ambiguous)."""
    want = _CONFIDENCE_RANK.get(minimum, 0)
    have = _CONFIDENCE_RANK.get(confidence_label(conf), 0)
    return have >= want
