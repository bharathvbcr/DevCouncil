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
#: Diffing two capped samples manufactures regressions from wherever the two
#: cuts fell — a file present in the baseline's 200 and absent from the current
#: 200 reads as "newly stranded" when nothing about it changed. So a truncated
#: side makes the snapshot incomplete, and an incomplete baseline makes the
#: ratchet skip.
#:
#: :func:`_graph_liveness` avoids the cap entirely by reading the graph rather
#: than the manifest, and refuses outright when the graph is itself a capped
#: export — so in practice this fires only on an empty-root corpus. It stays
#: because "the list was cut" and "the baseline is unusable" are different facts
#: and a future list may reintroduce the first.
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


def _graph_liveness(project_root: Path, head: str) -> Optional[dict]:
    """The kernel's own uncapped liveness lists, from `code_graph.json`.

    The manifest caps `unwired_candidates` and `dead_symbol_candidates` at 200
    for readability and publishes the real totals beside them. That cap is fine
    for a reader and fatal for a ratchet: diffing two capped samples
    manufactures regressions from wherever the two cuts fell. Measured on this
    repository, the manifest shows 200 of 228 unwired while the graph carries
    all 228.

    So the lists come from the graph, which the same `devmap manifest` run
    writes, and `resolution_rate` — which only the manifest carries — is joined
    to them on `generated_head`.

    Returns `None` when the graph cannot be used as a complete inventory:

    * it is a size-capped export (`compatibility_export_tier` `compact` caps the
      lists and `stub` empties them), or
    * it was generated from a different commit than the manifest beside it —
      pairing lists from one generation with a rate from another turns the
      difference between two generations into a "regression".
    """
    try:
        from devcouncil.indexing.graph.build import load_code_graph
    except ImportError:
        return None
    try:
        graph = load_code_graph(project_root)
    except Exception:
        logger.warning("code graph unreadable for the liveness snapshot", exc_info=True)
        return None
    if graph is None:
        return None

    meta = getattr(graph, "meta", None) or {}
    tier = meta.get("compatibility_export_tier")
    if tier is not None and tier != "slim":
        return None
    graph_head = str(getattr(graph, "generated_head", "") or "")
    if head and graph_head and graph_head != head:
        return None

    # `path::name` for every symbol the graph knows. The ratchet needs it to
    # tell a symbol that went dead from one this task just wrote; without it the
    # symbol half refuses to flag anything, which is the "branch that cannot
    # execute" shape this work order removes.
    symbol_index = sorted(
        {
            f"{_norm(str(node.path))}::{node.name}"
            for node in (getattr(graph, "nodes", None) or [])
            if getattr(node, "path", "") and getattr(node, "name", "")
        }
    )
    if not symbol_index:
        return None

    # The kernel writes the `path::name` form here already, which is exactly
    # the shape `_symbol_key` parses.
    dead_symbols = _as_list(meta.get("legacy_dead_symbol_candidates"))
    if not dead_symbols:
        dead_symbols = [
            str(entry.get("id"))
            for entry in (getattr(graph, "dead_code", None) or [])
            if isinstance(entry, Mapping) and entry.get("id")
        ]

    return {
        "entry_roots": _as_list(getattr(graph, "entry_roots", None)),
        "unwired_candidates": _as_list(getattr(graph, "unwired_candidates", None)),
        "unreachable_files": _as_list(getattr(graph, "unreachable_files", None)),
        "dead_symbol_candidates": dead_symbols,
        "symbol_index": symbol_index,
        "liveness_unreachable_unreliable": bool(
            meta.get("liveness_unreachable_unreliable")
        ),
        "generated_head": graph_head,
    }


def kernel_liveness_snapshot(project_root: Path) -> Optional[dict]:
    """Liveness lists from the kernel — the engine that actually ships.

    Replaces `RepoMapper.liveness_snapshot`, which ran the Python
    `_compute_liveness` and its regex token scanner. Everything else in the
    system reads the Rust kernel, so the ratchet compared a Python baseline
    against a Python current on a code path nothing else used, and both differed
    from what `dev map dead` reports. A regression in it did not mean what a
    user would see.

    Both sides of the diff come through this one function, so they cannot drift
    onto different engines. Returns `None` when the kernel cannot answer: the
    caller then declines to write a baseline rather than writing an empty one,
    because an empty baseline reads as "nothing was stranded" and would clear
    every future diff.
    """
    try:
        from devcouncil.devmap_client import DevMapClient, DevMapClientError

        client = DevMapClient(project_root)
        try:
            # Regenerates the map and the graph together, so the lists and the
            # rate come from one generation. `read_repo_map` would be cheaper
            # and wrong: the ratchet's current side has to reflect the tree
            # *after* this task's edits.
            manifest = client.manifest()
        except DevMapClientError as exc:
            logger.warning("kernel liveness snapshot unavailable: %s", exc)
            return None
        if not isinstance(manifest, Mapping):
            return None

        head = str(manifest.get("generated_head") or "")
        if not head:
            # Without it the graph beside this manifest cannot be checked for
            # being the same generation, and the whole point of reading two
            # artifacts is that they agree. An unstamped manifest is a manifest
            # nothing can be paired with.
            logger.warning(
                "the manifest carries no generated_head, so the graph beside it "
                "cannot be shown to describe the same generation"
            )
            return None
        graph = _graph_liveness(project_root, head)
        if graph is None:
            logger.warning(
                "no usable code graph beside the manifest, so the liveness lists "
                "would be the manifest's capped samples — diffing two samples "
                "manufactures regressions from wherever the cuts fell"
            )
            return None

        rate = manifest.get("resolution_rate")
        net_permille = None
        if isinstance(rate, Mapping) and _is_plain_int(rate.get("net_permille")):
            net_permille = rate["net_permille"]

        return {
            **graph,
            # The lists above are the graph's, uncapped, so nothing here is a
            # sample. `truncated_lists` stays in the payload as an explicit
            # empty rather than being dropped: a reader checking it must not
            # have to know whether the key means "nothing was cut" or "nobody
            # looked".
            "truncated_lists": [],
            "net_resolution_permille": net_permille,
            "generated_head": head or graph.get("generated_head") or "",
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
        # Two reasons a snapshot cannot be a ratchet input, kept apart so the
        # log says which one fired:
        #
        # * empty entry roots — every file looks unreachable, so the diff would
        #   flood or, after a fail-soft `unreachable=[]`, falsely look clean;
        # * any capped list — see `TRUNCATED_LIST_IS_INCOMPLETE`.
        #
        # `liveness_unreachable_unreliable` is deliberately *not* one of them.
        # It says the reachability answer cannot be trusted, and the diff
        # already suppresses exactly the unreachable half when either side
        # reports it (`_liveness_roots_unreliable`). Gating the whole baseline
        # on it as well threw away the unwired and dead-symbol halves too — and
        # since W1.1 made it a computed verdict it is true on any corpus with a
        # parse failure or an import-blind language, which is most real ones.
        # Measured on this repository: the flag is true, both other halves are
        # perfectly good, and no baseline was written at all.
        no_roots = not _as_list(snap.get("entry_roots"))
        complete = not no_roots and not (truncated and TRUNCATED_LIST_IS_INCOMPLETE)
        unreliable = bool(snap.get("liveness_unreachable_unreliable"))
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
                "liveness baseline for %s is not a ratchet input "
                "(roots_empty=%s, truncated=%s)",
                task_id,
                no_roots,
                truncated or "none",
            )
        return out_path if complete else None
    except Exception:
        logger.debug("snapshot_liveness_baseline failed for %s", task_id, exc_info=True)
        return None
