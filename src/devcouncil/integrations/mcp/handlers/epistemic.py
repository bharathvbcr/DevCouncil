"""Whether an answer is complete, and what stopped it from being.

The kernel computes more of this than any comparable tool and published none of
it as a single readable verdict. Coverage loss, the six-tier defect ledger,
ambiguity fan-out truncation, per-language capability — all present, all spread
across keys a caller had to know to look for and know how to combine.

So the tool descriptions carried the reasoning as prose instead::

    If entry_roots are empty or unreachable_unreliable is true, ignore
    unreachable_files and mass inferred dead.

That is a rule an agent has to remember and apply. This module turns it into a
field the response carries: ``epistemic`` is ``exact`` or ``lower_bound``, and
``boundaries`` names every reason it is the latter.

**A missing input is a boundary, not an absence of one.** If a count cannot be
read, the answer is ``lower_bound`` and the boundary says the check could not
run — never ``exact`` by default. That direction is the whole point: a verdict
that quietly upgrades itself when it loses evidence is worse than no verdict.
"""

from __future__ import annotations

from typing import Any, Iterable, Mapping, Sequence

#: The answer accounts for everything in scope.
EXACT = "exact"
#: Something the answer depends on could not be seen; the result is a floor.
LOWER_BOUND = "lower_bound"

#: How the coverage-gap kinds read in a boundary line.
#:
#: The last two are not failures — a grammar read the file and this build has no
#: extractor for its language — and the wording says so, because a reader's next
#: move differs: a parse failure may be fixed by a re-index, a missing extractor
#: never is.
_GAP_PROSE: Mapping[str, str] = {
    "discovery_refused": "{n} file(s) discovery refused and never read",
    "parse_failed": "{n} file(s) failed to parse",
    "pattern_recovered": "{n} file(s) recovered by pattern, contributing no calls",
    "call_blind": (
        "{n} file(s) in a language with no call extractor in this build (permanent, not a transient failure)"
    ),
    "import_blind": (
        "{n} file(s) in a language with no import extractor in this build (permanent, not a transient failure)"
    ),
}


def _gap_boundaries(coverage_gaps: Any) -> list[str]:
    """Boundary lines for each non-empty coverage-gap kind.

    ``coverage_gaps`` is ``None`` when the kernel read no store and a sentinel
    when the binary predates the listing. Both mean "not measured", and both
    produce a boundary rather than silence — an unmeasured gap is exactly the
    case where an ``exact`` verdict would be a lie.
    """
    if not isinstance(coverage_gaps, Mapping):
        return ["coverage gaps were not reported by this kernel, so completeness is unknown"]

    lines: list[str] = []
    for kind, template in _GAP_PROSE.items():
        entry = coverage_gaps.get(kind)
        if not isinstance(entry, Mapping):
            continue
        try:
            total = int(entry.get("total") or 0)
        except (TypeError, ValueError):
            continue
        if total > 0:
            lines.append(template.format(n=total))
    return lines


def epistemic_verdict(
    *,
    coverage_gaps: Any = None,
    degraded_reason: str | None = None,
    truncated: bool = False,
    shown: int | None = None,
    total: int | None = None,
    extra_boundaries: Sequence[str] | Iterable[str] = (),
) -> tuple[str, list[str]]:
    """Classify an answer and name every boundary on it.

    Returns ``(epistemic, boundaries)``. ``exact`` only when nothing was
    truncated, nothing was degraded, and every coverage-gap count that could be
    read was zero.
    """
    boundaries: list[str] = []

    if degraded_reason:
        boundaries.append(str(degraded_reason))

    boundaries.extend(_gap_boundaries(coverage_gaps))

    if truncated:
        if isinstance(shown, int) and isinstance(total, int) and total > shown:
            boundaries.append(
                f"results truncated: {shown} of {total} shown, so the list is a sample and the count is the real total"
            )
        else:
            boundaries.append("results truncated; the list is a sample, not an inventory")

    for extra in extra_boundaries:
        if extra:
            boundaries.append(str(extra))

    # Deduplicate while keeping order: the same fact can arrive from a
    # degraded_reason string and from a gap count, and saying it twice makes the
    # list look longer than the evidence is.
    seen: set[str] = set()
    unique: list[str] = []
    for line in boundaries:
        if line not in seen:
            seen.add(line)
            unique.append(line)

    return (LOWER_BOUND if unique else EXACT), unique


def with_epistemic(payload: dict[str, Any], **kwargs: Any) -> dict[str, Any]:
    """Stamp ``payload`` with its verdict, in place, and return it.

    A helper rather than two lines at each call site so the two keys can never
    be written apart: a payload carrying ``boundaries`` and no ``epistemic``
    reads as complete-with-caveats, which is the confusion this exists to end.
    """
    epistemic, boundaries = epistemic_verdict(**kwargs)
    payload["epistemic"] = epistemic
    payload["boundaries"] = boundaries
    return payload
