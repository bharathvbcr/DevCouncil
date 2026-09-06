"""Liveness ratchet: flag existing code newly stranded by this task.

Diff-scoped gates catch *new* unwired files / dead symbols. This check catches
the complementary failure: a task removes the last importer/caller of *existing*
code, leaving it stranded relative to the checkout-time baseline.

Skip when no complete baseline exists (pre-feature tasks). Never raises.
"""

from __future__ import annotations

import logging
from pathlib import Path
from typing import Any, Callable, List, Mapping, Optional, Set, TypeGuard

from devcouncil.domain.gap import Gap
from devcouncil.domain.task import Task

logger = logging.getLogger(__name__)

#: Bumped 1 → 2 when the snapshot moved off the retired Python scanner onto the
#: kernel (:func:`kernel_liveness_snapshot`). Bump this together with
#: ``wiring.LIVENESS_SCAN_VERSION``: the symbol half already refuses a mismatched
#: ``scan_version``, and the diff below refuses a mismatched ``schema_version``,
#: so an old baseline invalidates cleanly instead of being compared across two
#: engines that disagree about what "unwired" means.
LIVENESS_SCHEMA_VERSION = 2

#: A list the kernel truncated cannot support a set difference.
#:
#: ``unwired_candidates`` and ``dead_symbol_candidates`` are capped samples in
#: the manifest, published beside their real totals. Diffing two samples
#: manufactures regressions from wherever the two cuts fell — a file present in
#: the baseline's 200 and absent from the current 200 reads as "newly stranded"
#: when nothing about it changed. So a truncated side makes the snapshot
#: incomplete, and an incomplete baseline makes the ratchet skip.
TRUNCATED_LIST_IS_INCOMPLETE = True


def _norm(path: str) -> str:
    s = str(path).replace("\\", "/")
    while s.startswith("./"):
        s = s[2:]
    return s


def _as_str_set(values: Any) -> Set[str]:
    if not isinstance(values, list):
        return set()
    return {_norm(str(v)) for v in values if v is not None and str(v).strip()}


def _as_list(value: object) -> list[Any]:
    return list(value) if isinstance(value, list) else []


def _symbol_key(entry: str) -> str:
    """Normalize ``path:line name`` / ``path:line:name`` to a stable identity key.

    Prefer ``path::name`` so line-number drift does not create false regressions.
    """
    text = str(entry).strip()
    if not text:
        return ""
    # Common map format: "path/to.py:12 symbol_name"
    if " " in text:
        loc, _, name = text.partition(" ")
        path = loc.rsplit(":", 1)[0] if ":" in loc else loc
        return f"{_norm(path)}::{name.strip()}"
    # Fallback: path:line:name or path:name
    parts = text.split(":")
    if len(parts) >= 3 and parts[-2].isdigit():
        return f"{_norm(parts[0])}::{parts[-1].strip()}"
    if len(parts) == 2:
        return f"{_norm(parts[0])}::{parts[1].strip()}"
    return _norm(text)


def _symbol_display(entry: str) -> tuple[str, Optional[int], str]:
    """Return (path, line|None, name) for gap fields."""
    text = str(entry).strip()
    if " " in text:
        loc, _, name = text.partition(" ")
        if ":" in loc:
            path, _, line_s = loc.rpartition(":")
            line = int(line_s) if line_s.isdigit() else None
            return _norm(path), line, name.strip()
        return _norm(loc), None, name.strip()
    parts = text.split(":")
    if len(parts) >= 3 and parts[-2].isdigit():
        return _norm(parts[0]), int(parts[-2]), parts[-1].strip()
    if len(parts) == 2:
        return _norm(parts[0]), None, parts[1].strip()
    return _norm(text), None, ""


def _is_plain_int(value: Any) -> TypeGuard[int]:
    """An integer, and not a bool wearing one's clothes.

    `bool` subclasses `int` in Python, so `isinstance(True, int)` is true and a
    manifest carrying `"net_permille": true` would arrive as the integer 1. The
    ratchet compares two ints and would then report a fall from 500 permille to
    1 as a catastrophic resolution collapse that never happened.
    """
    return isinstance(value, int) and not isinstance(value, bool)


def _shown_total(meta: Any, key: str) -> tuple[int, int, bool]:
    """``(shown, total, truncated)`` for one ``liveness_meta`` section.

    A section the kernel did not send reports ``truncated=True``: not knowing
    whether a list was cut is not the same as knowing it was not, and only one
    of those two can safely feed a set difference.
    """
    section = meta.get(key) if isinstance(meta, Mapping) else None
    if not isinstance(section, Mapping):
        return (0, 0, True)
    # Both halves must be present and integral. A section carrying `shown` and
    # no `total` used to read as `(5, 0, False)` — untruncated — because a
    # missing total defaulted to zero and `0 > 5` is false. A cut that cannot be
    # measured is not a cut that did not happen, and that reading is what would
    # let a capped sample become a ratchet baseline.
    shown_raw = section.get("shown")
    total_raw = section.get("total")
    if not _is_plain_int(shown_raw) or not _is_plain_int(total_raw):
        return (0, 0, True)
    shown, total = int(shown_raw), int(total_raw)
    return (shown, total, bool(section.get("truncated")) or total > shown)


def _symbol_index_from_graph(project_root: Path, head: str) -> tuple[list[str], bool]:
    """``path::name`` for every symbol the kernel's graph knows.

    Returns ``(index, usable)``. The ratchet needs this to tell a symbol that
    went dead from one this task just wrote — without it the symbol half of the
    diff refuses to flag anything, which is the "branch that cannot execute"
    shape this work order removes.

    ``usable`` is False whenever the index cannot be trusted as a complete
    inventory:

    * the graph is a size-capped export (``compatibility_export_tier``
      ``compact`` drops node extras, ``stub`` drops nodes entirely), or
    * it was generated from a different commit than the manifest beside it —
      pairing a symbol index from one generation with candidate lists from
      another turns the difference between two generations into a "regression".
    """
    try:
        from devcouncil.indexing.graph.build import load_code_graph
    except ImportError:
        return ([], False)
    try:
        graph = load_code_graph(project_root)
    except Exception:
        logger.warning("code graph unreadable for the symbol index", exc_info=True)
        return ([], False)
    if graph is None:
        return ([], False)

    meta = getattr(graph, "meta", None) or {}
    tier = meta.get("compatibility_export_tier")
    if tier is not None and tier != "slim":
        return ([], False)
    graph_head = str(getattr(graph, "generated_head", "") or "")
    if head and graph_head and graph_head != head:
        return ([], False)

    index = sorted(
        {
            f"{_norm(str(node.path))}::{node.name}"
            for node in (getattr(graph, "nodes", None) or [])
            if getattr(node, "path", "") and getattr(node, "name", "")
        }
    )
    return (index, bool(index))


def kernel_liveness_snapshot(project_root: Path) -> Optional[dict]:
    """Liveness lists from the kernel — the engine that actually ships.

    Replaces ``RepoMapper.liveness_snapshot``, which ran the Python
    ``_compute_liveness`` and its regex token scanner. Everything else in the
    system reads the Rust kernel, so the ratchet compared a Python baseline
    against a Python current on a code path nothing else used, and both differed
    from what ``dev map dead`` reports. A regression in it did not mean what a
    user would see.

    Both sides of the diff come through this one function, so they cannot drift
    onto different engines. Returns ``None`` when the kernel cannot answer: the
    caller then declines to write a baseline rather than writing an empty one,
    because an empty baseline reads as "nothing was stranded" and would clear
    every future diff.
    """
    try:
        from devcouncil.devmap_client import DevMapClient, DevMapClientError

        client = DevMapClient(project_root)
        try:
            # Regenerates the map and the graph together, so the lists below and
            # the symbol index come from one generation. `read_repo_map` would
            # be cheaper and wrong: the ratchet's current side has to reflect
            # the tree *after* this task's edits.
            manifest = client.manifest()
        except DevMapClientError as exc:
            logger.warning("kernel liveness snapshot unavailable: %s", exc)
            return None
        if not isinstance(manifest, Mapping):
            return None

        meta = manifest.get("liveness_meta")
        _, _, dead_truncated = _shown_total(meta, "dead_symbol")
        _, _, unwired_truncated = _shown_total(meta, "unwired")
        # `unreachable_files` is bounded by the dead-cluster caps rather than by
        # a `liveness_meta` section; the manifest reports that cut separately.
        unreachable_truncated = bool(manifest.get("dead_clusters_truncated"))

        head = str(manifest.get("generated_head") or "")
        symbol_index, index_usable = _symbol_index_from_graph(project_root, head)

        rate = manifest.get("resolution_rate")
        net_permille = None
        if isinstance(rate, Mapping) and _is_plain_int(rate.get("net_permille")):
            net_permille = rate["net_permille"]

        truncated = sorted(
            name
            for name, cut in (
                ("dead_symbol_candidates", dead_truncated),
                ("unwired_candidates", unwired_truncated),
                ("unreachable_files", unreachable_truncated),
                ("symbol_index", not index_usable),
            )
            if cut
        )
        return {
            "entry_roots": _as_list(manifest.get("entry_roots")),
            "unwired_candidates": _as_list(manifest.get("unwired_candidates")),
            "unreachable_files": _as_list(manifest.get("unreachable_files")),
            "dead_symbol_candidates": _as_list(manifest.get("dead_symbol_candidates")),
            "symbol_index": symbol_index,
            # The kernel's own reachability verdict, computed since W1.1 from the
            # component pass rather than hardcoded `true`. That is what makes the
            # unreachable half of the diff a live branch again instead of one
            # that could never fire.
            "liveness_unreachable_unreliable": bool(
                manifest.get("liveness_unreachable_unreliable")
            ),
            # Named, not counted: a reader is told *which* list was cut, and the
            # baseline writer refuses to record a snapshot built from any of
            # them. See :data:`TRUNCATED_LIST_IS_INCOMPLETE`.
            "truncated_lists": truncated,
            "net_resolution_permille": net_permille,
            "generated_head": head,
            "engine": "devmap_rust",
        }
    except Exception:
        logger.warning("kernel liveness snapshot failed", exc_info=True)
        return None


def baseline_is_complete(baseline: Mapping[str, Any] | None) -> bool:
    """True when baseline exists and carries the write-completed marker."""
    return bool(baseline and isinstance(baseline, Mapping) and baseline.get("complete") is True)


def _liveness_roots_unreliable(snap: Mapping[str, Any] | None) -> bool:
    """True when unreachable lists must not be trusted (empty roots / flag)."""
    if not snap or not isinstance(snap, Mapping):
        return True
    if snap.get("liveness_unreachable_unreliable") is True:
        return True
    roots = snap.get("entry_roots")
    if not isinstance(roots, list) or not roots:
        return True
    return False


def detect_liveness_regressions(
    baseline: Mapping[str, Any] | None,
    current: Mapping[str, Any] | None,
    task_added_files: Set[str] | None = None,
    *,
    task: Task | None = None,
    next_gap_id: Callable[[str, str], str] | None = None,
    blocking: bool = False,
    diff_added_lines: Mapping[str, Set[int]] | None = None,
) -> List[Gap]:
    """Diff baseline vs current liveness; flag newly stranded pre-existing code.

    Files/symbols added by this task are excluded (covered by ``unwired_file`` /
    ``dead_symbol``). Dead symbols must have existed at baseline (``symbol_index``)
    — a brand-new unused def in an existing file is not ``stranded_code``.

    Returns an empty list when ``baseline`` is missing or incomplete.
    When either side has empty entry roots / unreliable unreachable, skip
    unreachable diffs (avoids empty-root stranded floods).
    """
    gaps: List[Gap] = []
    if not baseline_is_complete(baseline):
        return gaps
    if not current or not isinstance(current, Mapping):
        return gaps
    assert baseline is not None
    try:
        base_schema = baseline.get("schema_version")
        cur_schema = current.get("schema_version") if isinstance(current, Mapping) else None
        if base_schema and cur_schema and base_schema != cur_schema:
            logger.warning(
                "liveness schema version mismatch (baseline=%s, current=%s); skipping ratchet diff",
                base_schema,
                cur_schema,
            )
            return gaps

        added = {_norm(p) for p in (task_added_files or set())}
        task_id = task.id if task is not None else "TASK"
        gap_id = next_gap_id or (lambda tid, kind: f"{tid}-{kind}-1")

        # The W2.1 net resolution rate, ratcheted before the candidate lists.
        #
        # A call the resolver stopped being able to attribute does not appear in
        # any list here — the edge simply is not there, and a symbol whose only
        # caller became unresolvable reads as "nothing calls it". So the rate
        # moves first, and a drop in it explains stranding the other checks
        # would otherwise report as the task's fault.
        #
        # Both sides must have measured it. A missing figure on either side is
        # not a drop to zero.
        base_rate = baseline.get("net_resolution_permille")
        cur_rate = current.get("net_resolution_permille")
        # `_is_plain_int` on both sides: a legacy or hand-edited baseline can
        # carry `true`, which `isinstance(_, int)` accepts and which would
        # compare as 1 — a fall from 500 permille to 1 that never happened.
        if _is_plain_int(base_rate) and _is_plain_int(cur_rate) and cur_rate < base_rate:
            gaps.append(Gap(
                id=gap_id(task_id, "RESRATE"),
                severity="high" if blocking else "medium",
                gap_type="resolution_regression",
                task_id=task_id,
                description=(
                    f"Call resolution fell from {base_rate / 10:.1f}% to "
                    f"{cur_rate / 10:.1f}% during this task: the resolver can attribute "
                    "a smaller share of calls than it could at checkout, so every "
                    "liveness finding below is a weaker claim than it was."
                ),
                evidence=[
                    f"net_resolution_permille:{base_rate}->{cur_rate}",
                    "regression:resolution_rate",
                ],
                recommended_fix=(
                    "Find what stopped resolving — a changed import, a moved "
                    "definition, a new dynamic dispatch — and restore the binding, "
                    "or record why the calls are now legitimately unattributable."
                ),
                blocking=blocking,
            ))
        added_lines = {
            _norm(p): set(lines)
            for p, lines in (diff_added_lines or {}).items()
        }

        base_unwired = _as_str_set(baseline.get("unwired_candidates"))
        base_unreachable = _as_str_set(baseline.get("unreachable_files"))
        cur_unwired = _as_str_set(current.get("unwired_candidates"))
        cur_unreachable = _as_str_set(current.get("unreachable_files"))

        newly_unwired = (cur_unwired - base_unwired) - added
        skip_unreachable = (
            _liveness_roots_unreliable(baseline) or _liveness_roots_unreliable(current)
        )
        newly_unreachable = (
            set()
            if skip_unreachable
            else (cur_unreachable - base_unreachable) - added
        )
        removed_from_base = (base_unwired - cur_unwired) | (base_unreachable - cur_unreachable)
        removed_basenames = {Path(r).name for r in removed_from_base if r}

        raw_stranded = newly_unwired | newly_unreachable
        stranded_files = sorted(
            p for p in raw_stranded if Path(p).name not in removed_basenames
        )

        for path in stranded_files:
            reasons = []
            if path in newly_unwired:
                reasons.append("unwired")
            if path in newly_unreachable:
                reasons.append("unreachable")
            reason = "/".join(reasons) or "stranded"
            gaps.append(Gap(
                id=gap_id(task_id, "STRAND"),
                severity="high" if blocking else "medium",
                gap_type="stranded_code",
                task_id=task_id,
                description=(
                    f"Pre-existing file `{path}` became {reason} during this task "
                    "(lost its last production importer/reachability path)."
                ),
                evidence=[path, f"regression:{reason}"],
                recommended_fix=(
                    "Restore the import/call that kept this module live, or delete "
                    "the stranded module if it is intentionally unused."
                ),
                blocking=blocking,
                file=path,
            ))

        # Symbol-level diff only when baseline was recorded with the same scan
        # algorithm — stale/missing scan_version would otherwise flag every newly
        # detected dead symbol as stranded_code after a detector hardening.
        from devcouncil.indexing.wiring import LIVENESS_SCAN_VERSION

        if baseline.get("scan_version") != LIVENESS_SCAN_VERSION:
            return gaps

        base_syms = {
            _symbol_key(s): str(s)
            for s in (baseline.get("dead_symbol_candidates") or [])
            if isinstance(s, str) and _symbol_key(s)
        }
        cur_syms = {
            _symbol_key(s): str(s)
            for s in (current.get("dead_symbol_candidates") or [])
            if isinstance(s, str) and _symbol_key(s)
        }
        # Symbols known at checkout. Without this index (legacy baselines), refuse
        # to flag new dead symbols — avoids double-firing on brand-new defs.
        base_index_raw = baseline.get("symbol_index") or []
        base_index = {
            _norm(str(k)) if "::" in str(k) else _symbol_key(str(k))
            for k in base_index_raw
            if k
        }
        # Normalize path::name keys
        base_index_norm: Set[str] = set()
        for k in base_index:
            if "::" in k:
                path_part, _, name = k.partition("::")
                base_index_norm.add(f"{_norm(path_part)}::{name}")
            else:
                base_index_norm.add(k)

        for key in sorted(set(cur_syms) - set(base_syms)):
            entry = cur_syms[key]
            path, line, name = _symbol_display(entry)
            if path in added:
                continue
            # Require the symbol existed at baseline (was live then, dead now).
            if not base_index_norm or key not in base_index_norm:
                continue
            # Defining line added in this task's diff → new symbol, not stranded.
            if line is not None and line in added_lines.get(path, ()):
                continue
            label = name or entry
            gaps.append(Gap(
                id=gap_id(task_id, "STRANDSYM"),
                severity="high" if blocking else "medium",
                gap_type="stranded_code",
                task_id=task_id,
                description=(
                    f"Pre-existing symbol `{label}` at {path}"
                    f"{':' + str(line) if line else ''} became unreferenced "
                    "during this task (lost its last caller)."
                ),
                evidence=[entry, f"symbol:{label}", "regression:dead_symbol"],
                recommended_fix=(
                    "Restore the import/call that referenced this symbol, or delete "
                    "the stranded module/symbol if it is intentionally unused."
                ),
                blocking=blocking,
                file=path,
                line=line,
            ))
    except Exception:
        logger.debug("detect_liveness_regressions failed; degrading to zero gaps", exc_info=True)
        return []
    return gaps


def load_liveness_baseline(project_root: Path, task_id: str) -> Optional[dict]:
    """Load checkout-time liveness baseline; None when absent, unreadable, or incomplete."""
    path = project_root / ".devcouncil" / "liveness_baseline" / f"{task_id}.json"
    if not path.is_file():
        return None
    try:
        from devcouncil.utils.json_persist import read_json

        data = read_json(path)
        if not isinstance(data, dict):
            return None
        if data.get("complete") is not True:
            logger.warning(
                "liveness baseline for %s is incomplete; treating as missing",
                task_id,
            )
            return None
        return data
    except Exception:
        logger.debug("failed to load liveness baseline for %s", task_id, exc_info=True)
        return None


def delete_liveness_baseline(project_root: Path, task_id: str) -> bool:
    """Remove the checkout baseline for ``task_id``. Returns True when deleted."""
    path = project_root / ".devcouncil" / "liveness_baseline" / f"{task_id}.json"
    try:
        if path.is_file():
            path.unlink()
            return True
    except OSError:
        logger.debug("failed to delete liveness baseline for %s", task_id, exc_info=True)
    return False


def snapshot_liveness_baseline(
    project_root: Path,
    task_id: str,
    *,
    reset: bool = False,
) -> Optional[Path]:
    """Write-once uncapped liveness snapshot for ``task_id``.

    Skips when a complete baseline already exists unless ``reset=True``. Writes
    only after a full successful scan and marks ``complete: true`` so partial
    failures never become ratchet inputs. Returns the path, or None on failure.
    Never raises.
    """
    try:
        from devcouncil.utils.json_persist import write_json

        out_dir = project_root / ".devcouncil" / "liveness_baseline"
        out_dir.mkdir(parents=True, exist_ok=True)
        out_path = out_dir / f"{task_id}.json"

        if out_path.is_file() and not reset:
            try:
                from devcouncil.utils.json_persist import read_json

                existing = read_json(out_path)
                if isinstance(existing, dict) and existing.get("complete") is True:
                    return out_path
            except Exception:
                pass
            # Incomplete/corrupt on disk — fall through and rewrite.

        snap = kernel_liveness_snapshot(project_root)
        if snap is None:
            logger.warning(
                "no liveness baseline written for %s: the kernel could not be read. "
                "An empty baseline would read as `nothing was stranded` and clear "
                "every later diff.",
                task_id,
            )
            return None
        from devcouncil.indexing.wiring import LIVENESS_SCAN_VERSION

        truncated = list(snap.get("truncated_lists") or [])
        # Three independent reasons a snapshot cannot be a ratchet input, kept
        # apart so the log says which one fired:
        #
        # * empty entry roots — every file looks unreachable, so the diff would
        #   flood or, after a fail-soft `unreachable=[]`, falsely look clean;
        # * the kernel's own `unreachable_unreliable` verdict;
        # * any capped list — see `TRUNCATED_LIST_IS_INCOMPLETE`.
        no_roots = not _as_list(snap.get("entry_roots"))
        unreliable = bool(snap.get("liveness_unreachable_unreliable")) or no_roots
        complete = not unreliable and not (truncated and TRUNCATED_LIST_IS_INCOMPLETE)
        payload: dict[str, Any] = {
            "unwired_candidates": _as_list(snap.get("unwired_candidates")),
            "unreachable_files": _as_list(snap.get("unreachable_files")),
            "dead_symbol_candidates": _as_list(snap.get("dead_symbol_candidates")),
            "entry_roots": _as_list(snap.get("entry_roots")),
            "symbol_index": _as_list(snap.get("symbol_index")),
            "liveness_unreachable_unreliable": unreliable,
            "truncated_lists": truncated,
            # The W2.1 net resolution rate, carried so the diff can ratchet it.
            # A drop here is the earliest signal that extraction regressed, and
            # it moves before any candidate list does.
            "net_resolution_permille": snap.get("net_resolution_permille"),
            "generated_head": snap.get("generated_head") or "",
            "source": "kernel_manifest",
            "engine": snap.get("engine") or "devmap_rust",
            "scan_version": LIVENESS_SCAN_VERSION,
            "schema_version": LIVENESS_SCHEMA_VERSION,
            "complete": complete,
        }
        write_json(out_path, payload)
        if not complete:
            logger.warning(
                "liveness baseline for %s is not a ratchet input (roots_empty=%s, "
                "unreliable=%s, truncated=%s)",
                task_id,
                no_roots,
                snap.get("liveness_unreachable_unreliable"),
                truncated or "none",
            )
        return out_path if complete else None
    except Exception:
        logger.debug("snapshot_liveness_baseline failed for %s", task_id, exc_info=True)
        return None
