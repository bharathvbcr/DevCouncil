"""Liveness ratchet: flag existing code newly stranded by this task.

Diff-scoped gates catch *new* unwired files / dead symbols. This check catches
the complementary failure: a task removes the last importer/caller of *existing*
code, leaving it stranded relative to the checkout-time baseline.

Skip when no complete baseline exists (pre-feature tasks). Never raises.
"""

from __future__ import annotations

import logging
from pathlib import Path
from typing import Any, Callable, List, Mapping, Optional, Set

from devcouncil.domain.gap import Gap
from devcouncil.domain.task import Task

logger = logging.getLogger(__name__)


def _norm(path: str) -> str:
    s = str(path).replace("\\", "/")
    while s.startswith("./"):
        s = s[2:]
    return s


def _as_str_set(values: Any) -> Set[str]:
    if not isinstance(values, list):
        return set()
    return {_norm(str(v)) for v in values if v is not None and str(v).strip()}


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


def baseline_is_complete(baseline: Mapping[str, Any] | None) -> bool:
    """True when baseline exists and carries the write-completed marker."""
    return bool(baseline and isinstance(baseline, Mapping) and baseline.get("complete") is True)


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
    """
    gaps: List[Gap] = []
    if not baseline_is_complete(baseline):
        return gaps
    if not current or not isinstance(current, Mapping):
        return gaps
    assert baseline is not None
    try:
        added = {_norm(p) for p in (task_added_files or set())}
        task_id = task.id if task is not None else "TASK"
        gap_id = next_gap_id or (lambda tid, kind: f"{tid}-{kind}-1")
        added_lines = {
            _norm(p): set(lines)
            for p, lines in (diff_added_lines or {}).items()
        }

        base_unwired = _as_str_set(baseline.get("unwired_candidates"))
        base_unreachable = _as_str_set(baseline.get("unreachable_files"))
        cur_unwired = _as_str_set(current.get("unwired_candidates"))
        cur_unreachable = _as_str_set(current.get("unreachable_files"))

        newly_unwired = (cur_unwired - base_unwired) - added
        newly_unreachable = (cur_unreachable - base_unreachable) - added
        stranded_files = sorted(newly_unwired | newly_unreachable)

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
        from devcouncil.indexing.repo_mapper import RepoMapper
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

        mapper = RepoMapper(project_root)
        snap = mapper.liveness_snapshot()
        from devcouncil.indexing.wiring import LIVENESS_SCAN_VERSION

        payload: dict[str, Any] = {
            "unwired_candidates": list(snap.get("unwired_candidates") or []),
            "unreachable_files": list(snap.get("unreachable_files") or []),
            "dead_symbol_candidates": list(snap.get("dead_symbol_candidates") or []),
            "entry_roots": list(snap.get("entry_roots") or []),
            "symbol_index": list(snap.get("symbol_index") or []),
            "generated_head": mapper._git_head(),
            "source": "fresh_scan",
            "scan_version": LIVENESS_SCAN_VERSION,
            "complete": True,
        }
        write_json(out_path, payload)
        return out_path
    except Exception:
        logger.debug("snapshot_liveness_baseline failed for %s", task_id, exc_info=True)
        return None
