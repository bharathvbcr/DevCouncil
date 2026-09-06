"""W2.2 — an answer says whether it is complete, and what stopped it.

The kernel computed more of this than any comparable tool and published none of
it as a single readable verdict, so the tool descriptions carried the reasoning
as prose an agent had to remember:

    If entry_roots are empty or unreachable_unreliable is true, ignore
    unreachable_files and mass inferred dead.

These tests pin the field that replaces it. The direction that matters most is
the failure direction: a verdict that quietly upgrades itself to ``exact`` when
it loses evidence is worse than no verdict at all, so every "input missing" case
below asserts ``lower_bound``.
"""

from __future__ import annotations

from devcouncil.integrations.mcp.handlers.epistemic import (
    EXACT,
    LOWER_BOUND,
    epistemic_verdict,
    with_epistemic,
)


def _gaps(**totals: int) -> dict[str, dict[str, int]]:
    """A coverage-gaps mapping in the shape ``devmap status`` emits."""
    kinds = (
        "discovery_refused",
        "parse_failed",
        "pattern_recovered",
        "call_blind",
        "import_blind",
    )
    return {kind: {"total": totals.get(kind, 0), "shown": 0, "truncated": False} for kind in kinds}


def test_a_complete_answer_is_exact_with_no_boundaries():
    """The OFF direction, and the one that makes every other test mean something.

    Without it, an implementation that returned ``lower_bound`` unconditionally
    would satisfy all of them — and a caveat that is always on carries exactly
    as much information as one that is never on.
    """
    verdict, boundaries = epistemic_verdict(coverage_gaps=_gaps(), truncated=False)
    assert verdict == EXACT
    assert boundaries == []


def test_unreported_coverage_gaps_are_a_boundary_not_a_pass():
    """A count that could not be read must never render as a count of zero.

    ``coverage_gaps`` is ``None`` when the kernel read no store and a sentinel
    when the binary predates the listing. Treating either as "nothing was
    missing" is the exact failure this whole hardening pass is about.
    """
    for absent in (None, "UNREPORTED", 0, []):
        verdict, boundaries = epistemic_verdict(coverage_gaps=absent)
        assert verdict == LOWER_BOUND, absent
        assert any("unknown" in line for line in boundaries), (absent, boundaries)


def test_each_coverage_gap_kind_produces_its_own_boundary():
    verdict, boundaries = epistemic_verdict(
        coverage_gaps=_gaps(parse_failed=2, discovery_refused=1)
    )
    assert verdict == LOWER_BOUND
    joined = " | ".join(boundaries)
    assert "2 file(s) failed to parse" in joined
    assert "1 file(s) discovery refused" in joined


def test_a_capability_gap_says_it_is_permanent():
    """The distinction a reader acts on.

    A parse failure may be fixed by a re-index; a language with no extractor
    never will be. Rendering both as "coverage loss" tells a reader to re-run a
    build that cannot change the answer.
    """
    _, boundaries = epistemic_verdict(coverage_gaps=_gaps(call_blind=3))
    assert any("no call extractor" in line and "permanent" in line for line in boundaries)

    _, boundaries = epistemic_verdict(coverage_gaps=_gaps(import_blind=4))
    assert any("no import extractor" in line and "permanent" in line for line in boundaries)


def test_truncation_is_a_boundary_and_names_both_numbers():
    verdict, boundaries = epistemic_verdict(
        coverage_gaps=_gaps(), truncated=True, shown=50, total=312
    )
    assert verdict == LOWER_BOUND
    assert any("50 of 312" in line for line in boundaries)


def test_truncation_without_counts_still_says_the_list_is_a_sample():
    verdict, boundaries = epistemic_verdict(coverage_gaps=_gaps(), truncated=True)
    assert verdict == LOWER_BOUND
    assert any("sample" in line for line in boundaries)


def test_a_degraded_reason_is_carried_verbatim():
    """The kernel's own wording, not a paraphrase.

    It already names counts and causes; restating it here would give a reader
    two descriptions of one fact that can drift apart.
    """
    reason = "call extraction did not cover the whole corpus: 2 file(s) failed to parse"
    verdict, boundaries = epistemic_verdict(coverage_gaps=_gaps(), degraded_reason=reason)
    assert verdict == LOWER_BOUND
    assert reason in boundaries


def test_the_same_fact_is_not_reported_twice():
    """A degraded reason and a gap count can describe one hole.

    Saying it twice makes the boundary list look longer than the evidence is,
    which is its own way of misreporting completeness.
    """
    line = "2 file(s) failed to parse"
    _, boundaries = epistemic_verdict(
        coverage_gaps=_gaps(parse_failed=2),
        degraded_reason=line,
        extra_boundaries=[line],
    )
    assert boundaries.count(line) == 1


def test_empty_extra_boundaries_are_dropped():
    """Call sites pass conditional strings that are empty when inapplicable.

    An empty string in the list would make the answer ``lower_bound`` for no
    stated reason, which is the least useful possible verdict.
    """
    verdict, boundaries = epistemic_verdict(coverage_gaps=_gaps(), extra_boundaries=["", None, ""])
    assert verdict == EXACT
    assert boundaries == []


def test_with_epistemic_writes_both_keys_together():
    """The two keys can never be written apart.

    A payload carrying ``boundaries`` and no ``epistemic`` reads as
    complete-with-caveats, which is the confusion the field exists to end.
    """
    payload: dict[str, object] = {"dead_code": []}
    returned = with_epistemic(payload, coverage_gaps=_gaps(call_blind=1))
    assert returned is payload
    assert payload["epistemic"] == LOWER_BOUND
    assert payload["boundaries"]
    assert set(payload) == {"dead_code", "epistemic", "boundaries"}


def test_a_malformed_gap_entry_does_not_crash_or_silently_pass():
    """Garbage in one kind must not make the whole verdict optimistic."""
    gaps = _gaps(parse_failed=1)
    gaps["call_blind"] = {"total": "not a number"}  # type: ignore[assignment]
    verdict, boundaries = epistemic_verdict(coverage_gaps=gaps)
    assert verdict == LOWER_BOUND
    assert any("failed to parse" in line for line in boundaries)
