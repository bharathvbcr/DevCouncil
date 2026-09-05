"""File-level liveness (entry roots, unwired, unreachable) and confidence ranking.

devcouncil: allow-unwired — package-private; reached via ``RepoMapper._compute_liveness``
(map + liveness ratchet) and the ``dev map dead`` / MCP confidence filters. The
symbol-level dead-code analysis that used to live here was retired with the Python
graph builder; the Rust kernel computes it now.
"""

from __future__ import annotations

import logging
from collections import defaultdict
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional, Set, Tuple

from devcouncil.indexing.graph.schema import (
    Confidence,
)

logger = logging.getLogger(__name__)

_CONFIDENCE_RANK = {
    Confidence.AMBIGUOUS: 0,
    Confidence.INFERRED: 1,
    Confidence.EXTRACTED: 2,
    "ambiguous": 0,
    "inferred": 1,
    "extracted": 2,
}

# Default: if unreachable covers ≥25% of liveness code files, static BFS is
# too blind (dynamic imports / routers / JSX) — omit the flood.
_DEFAULT_UNREACHABLE_UNRELIABLE_RATIO = 0.25
# Ratio alone misreads small repos: 1 dead file out of 3 is real signal, not
# analysis blindness. Only gate when the absolute flood is at least this big.
_MIN_UNREACHABLE_FOR_RATIO_GATE = 5


def _unreachable_unreliable_ratio(root: Path) -> float:
    try:
        from devcouncil.app.config import load_config

        return float(load_config(root).indexing.unreachable_unreliable_ratio)
    except Exception:
        return _DEFAULT_UNREACHABLE_UNRELIABLE_RATIO


def _apply_unreachable_ratio_gate(
    root: Path,
    files: List[str],
    unreachable: List[str],
    unreliable: bool,
) -> Tuple[List[str], bool, dict]:
    """Fail-soft when unreachable density indicates analysis blind spots."""
    from devcouncil.indexing.wiring import is_liveness_code_file

    code_files = sum(1 for f in files if is_liveness_code_file(f))
    ratio = (len(unreachable) / code_files) if code_files else 0.0
    threshold = _unreachable_unreliable_ratio(root)
    meta: Dict[str, Any] = {
        "unreachable_total": len(unreachable),
        "liveness_code_files": code_files,
        "unreachable_ratio": round(ratio, 4),
        "unreachable_ratio_threshold": threshold,
    }
    if unreliable:
        meta["unreachable_unreliable_reason"] = "empty_production_entry_roots"
        return [], True, meta
    if (
        threshold > 0
        and code_files > 0
        and len(unreachable) >= _MIN_UNREACHABLE_FOR_RATIO_GATE
        and ratio >= threshold
    ):
        meta["unreachable_unreliable_reason"] = "high_unreachable_ratio"
        return [], True, meta
    return unreachable, False, meta


def _allow_unwired_lookup(root: Path) -> Callable[[str], bool]:
    """Per-pass path→bool cache for ``has_allow_unwired`` (mtime-safe within one pass)."""
    from devcouncil.indexing.wiring import has_allow_unwired

    cache: Dict[str, bool] = {}

    def _allowed(path: str) -> bool:
        hit = cache.get(path)
        if hit is not None:
            return hit
        value = has_allow_unwired(root, path)
        cache[path] = value
        return value

    return _allowed


def file_liveness(
    root: Path,
    files: List[str],
    file_edges: List[Tuple[str, str]],
    *,
    cap: Optional[int] = None,
    dynamic_index: Optional[dict[str, Set[str]]] = None,
) -> Tuple[List[str], List[str], List[str], bool]:
    """Return (entry_roots, unwired, unreachable, unreachable_unreliable).

    ``cap`` defaults to uncapped (``None`` / ``<= 0``). Caps belong on
    ``repo_map.json`` serialization, not the in-memory / ``code_graph.json`` lists.

    When production entry roots are empty, unreachable BFS would flood every
    non-root file. Fail soft: ``unreachable=[]`` and
    ``unreachable_unreliable=True`` (still compute unwired).

    When unreachable density exceeds ``indexing.unreachable_unreliable_ratio``
    (default 0.25), also fail soft — treat the list as low-confidence noise.

    Prefer a prebuilt ``dynamic_index`` from the caller (one scan per assemble pass).
    """
    from devcouncil.indexing.wiring import (
        build_dynamic_import_index,
        content_liveness_exemption,
        entry_roots,
        is_liveness_code_file,
        is_test_path,
        reference_cleared,
        structural_exemptions,
    )

    limit = None if (cap is None or cap <= 0) else cap
    roots = entry_roots(root, files)
    prod_roots = entry_roots(root, files, production_only=True)
    root_set = set(roots)
    prod_root_set = set(prod_roots)
    dyn_index = (
        build_dynamic_import_index(root, files)
        if dynamic_index is None
        else dynamic_index
    )
    allow_unwired = _allow_unwired_lookup(root)
    unreachable_unreliable = not bool(prod_roots)

    inbound: Dict[str, Set[str]] = defaultdict(set)
    outbound: Dict[str, Set[str]] = defaultdict(set)
    for importer, imported in file_edges:
        inbound[imported].add(importer)
        outbound[importer].add(imported)

    unwired: List[str] = []
    for f in sorted(files):
        if not is_liveness_code_file(f):
            continue
        if f in root_set or structural_exemptions(f):
            continue
        if allow_unwired(f):
            continue
        if any(not is_test_path(i) for i in inbound.get(f, ())):
            continue
        if reference_cleared(
            root, f, skip_files=set(), git_files=files, dynamic_index=dyn_index
        ):
            continue
        # Generated stubs, __main__-guard scripts, go:embed / func init()
        # carriers, and re-export-only package inits are wired by content.
        if content_liveness_exemption(root, f) is not None:
            continue
        unwired.append(f)
        if limit is not None and len(unwired) >= limit:
            break

    unreachable: List[str] = []
    if not unreachable_unreliable:
        reachable: Set[str] = set()
        queue = list(prod_roots)
        seen_q: Set[str] = set(queue)
        while queue:
            cur = queue.pop()
            reachable.add(cur)
            for nxt in outbound.get(cur, ()):
                if nxt not in seen_q:
                    seen_q.add(nxt)
                    queue.append(nxt)

        for f in sorted(files):
            if not is_liveness_code_file(f):
                continue
            if f in prod_root_set or f in root_set or structural_exemptions(f):
                continue
            if f in reachable:
                continue
            # Dynamic entrypoints / markers clear unwired; keep unreachable in parity.
            if allow_unwired(f):
                continue
            if reference_cleared(
                root, f, skip_files=set(), git_files=files, dynamic_index=dyn_index
            ):
                continue
            if content_liveness_exemption(root, f) is not None:
                continue
            unreachable.append(f)
            if limit is not None and len(unreachable) >= limit:
                break

    unreachable, unreachable_unreliable, _ratio_meta = _apply_unreachable_ratio_gate(
        root, files, unreachable, unreachable_unreliable
    )
    if not unreachable_unreliable and unreachable:
        gap_points = {
            f: [i for i in inbound.get(f, ()) if i in reachable][0]
            for f in unreachable
            if any(i in reachable for i in inbound.get(f, ()))
        }
        if gap_points:
            _ratio_meta["reachability_gap_points"] = gap_points
    return prod_roots, unwired, unreachable, unreachable_unreliable


# Framework targets are live only through a reachable registration owner.


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
