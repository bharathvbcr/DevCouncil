"""Incremental gate runner: the sub-second sidecar behind ``dev check --watch``.

Ties :mod:`devcouncil.verification.gate_selector` (path → relevant/narrowed gates) to
:mod:`devcouncil.verification.gate_cache` (content-hash skip of previously-green gates).
On an iterative edit only the gates whose inputs actually changed are executed; the rest
are served from cache. This is deliberately separate from the full ``verify_task`` path,
which release / ``dev go`` still run in full and which never consults this cache.

Given the working-tree changed files and the project's configured commands, it:
1. selects the gates relevant to the changed stacks (and narrows linters to the touched
   files),
2. skips any gate that passed last time with byte-identical inputs,
3. runs the remainder and records their pass/fail in the cache,
4. returns a compact, render-friendly result.

Best-effort and side-effect-light: it shells out only to run the gates themselves and
persists a single small JSON cache file.
"""

from __future__ import annotations

import logging
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable, List, Mapping, Optional, Sequence

from devcouncil.utils.json_persist import read_json
from devcouncil.verification.gate_cache import GateResultCache
from devcouncil.verification.gate_selector import GateSpec, select_gates

logger = logging.getLogger(__name__)


@dataclass
class GateOutcome:
    name: str
    kind: str
    command: str
    passed: bool
    cached: bool
    summary: str = ""
    duration_s: float = 0.0
    narrowed: bool = False


@dataclass
class IncrementalResult:
    changed_files: List[str] = field(default_factory=list)
    outcomes: List[GateOutcome] = field(default_factory=list)
    skipped: List[tuple[str, str]] = field(default_factory=list)
    duration_s: float = 0.0
    no_changes: bool = False
    no_gates: bool = False
    narrowed: bool = False

    @property
    def ran(self) -> List[GateOutcome]:
        return [o for o in self.outcomes if not o.cached]

    @property
    def cached(self) -> List[GateOutcome]:
        return [o for o in self.outcomes if o.cached]

    @property
    def passed(self) -> bool:
        return all(o.passed for o in self.outcomes)

    def to_dict(self) -> dict:
        return {
            "passed": self.passed,
            "no_changes": self.no_changes,
            "no_gates": self.no_gates,
            "narrowed": self.narrowed,
            "changed_files": self.changed_files,
            "duration_s": round(self.duration_s, 4),
            "gates": [
                {
                    "name": o.name,
                    "kind": o.kind,
                    "command": o.command,
                    "passed": o.passed,
                    "cached": o.cached,
                    "narrowed": o.narrowed,
                    "duration_s": round(o.duration_s, 4),
                }
                for o in self.outcomes
            ],
            "ran": len(self.ran),
            "cached_hits": len(self.cached),
            "skipped": [{"command": c, "reason": r} for c, r in self.skipped],
        }


def _default_commands(project_root: Path) -> dict[str, list[str]]:
    """Configured test/lint/typecheck commands, or ``{}`` on any config failure."""
    try:
        from devcouncil.app.config import load_config

        cfg = load_config(project_root)
        return {
            "test": list(cfg.commands.test),
            "lint": list(cfg.commands.lint),
            "typecheck": list(cfg.commands.typecheck),
        }
    except Exception as exc:
        logger.debug("incremental gates: config commands unavailable: %s", exc)
        return {}


def _load_repo_map(project_root: Path) -> dict | None:
    """Best-effort load of ``.devcouncil/repo_map.json`` for dependent expansion."""
    map_path = project_root / ".devcouncil" / "repo_map.json"
    if not map_path.is_file():
        return None
    try:
        data = read_json(map_path)
        return data if isinstance(data, dict) else None
    except Exception as exc:
        logger.debug("incremental gates: repo map unavailable: %s", exc)
        return None


def _default_changed_files(project_root: Path) -> list[str]:
    from devcouncil.verification.verifier import Verifier

    return Verifier(project_root).get_changed_files()


def _default_runner(project_root: Path, timeout: int) -> Callable[[str], tuple[bool, str]]:
    from devcouncil.verification.command_runner import run_verification_command

    def _run(command: str) -> tuple[bool, str]:
        result = run_verification_command(
            project_root, command, task_id="check-watch", timeout=timeout
        )
        return result.exit_code == 0, result.summary

    return _run


def run_incremental_gates(
    project_root: Path,
    *,
    commands: Optional[Mapping[str, Sequence[str]]] = None,
    extra_test_commands: Optional[Sequence[str]] = None,
    changed_files: Optional[Sequence[str]] = None,
    cache: Optional[GateResultCache] = None,
    use_cache: bool = True,
    narrow: bool = True,
    runner: Optional[Callable[[str], tuple[bool, str]]] = None,
    timeout: int = 300,
) -> IncrementalResult:
    """Run only the gates affected by ``changed_files``, skipping cached-green ones.

    ``commands`` defaults to the project's configured test/lint/typecheck commands;
    ``extra_test_commands`` (e.g. ``dev check --test``) are appended to the test kind.
    ``runner`` is a ``command -> (passed, summary)`` callable (defaults to the real
    verification command runner) — the seam that keeps this unit-testable without
    subprocesses.
    """
    started = time.perf_counter()
    root = Path(project_root)

    if changed_files is None:
        changed_files = _default_changed_files(root)
    changed = [p for p in changed_files if p and p.strip()]

    resolved_commands: dict[str, list[str]] = {
        k: list(v) for k, v in (commands or _default_commands(root)).items()
    }
    if extra_test_commands:
        resolved_commands.setdefault("test", [])
        resolved_commands["test"] = list(resolved_commands["test"]) + [
            c for c in extra_test_commands if c and c.strip()
        ]

    if not changed:
        return IncrementalResult(no_changes=True, duration_s=time.perf_counter() - started)

    repo_map = _load_repo_map(root)
    selection = select_gates(changed, resolved_commands, narrow=narrow, repo_map=repo_map)
    any_narrowed = any(g.narrowed for g in selection.gates)
    if not selection.gates:
        return IncrementalResult(
            changed_files=changed,
            skipped=selection.skipped,
            no_gates=True,
            narrowed=any_narrowed,
            duration_s=time.perf_counter() - started,
        )

    if cache is None:
        cache = GateResultCache(root)
    cache.load()
    run = runner or _default_runner(root, timeout)

    outcomes: List[GateOutcome] = []
    for gate in selection.gates:
        if use_cache and cache.is_green(gate):
            outcomes.append(GateOutcome(
                name=gate.name, kind=gate.kind, command=gate.command,
                passed=True, cached=True,
                summary=cache.cached_summary(gate) or "", narrowed=gate.narrowed,
            ))
            continue
        gate_started = time.perf_counter()
        try:
            passed, summary = run(gate.command)
        except Exception as exc:  # a gate crash must not kill the sidecar loop
            logger.debug("incremental gate %s crashed: %s", gate.name, exc)
            passed, summary = False, f"gate crashed: {exc}"
        gate_dt = time.perf_counter() - gate_started
        cache.record(gate, passed=passed, summary=summary)
        outcomes.append(GateOutcome(
            name=gate.name, kind=gate.kind, command=gate.command,
            passed=passed, cached=False, summary=summary,
            duration_s=gate_dt, narrowed=gate.narrowed,
        ))

    cache.save()
    return IncrementalResult(
        changed_files=changed,
        outcomes=outcomes,
        skipped=selection.skipped,
        narrowed=any_narrowed,
        duration_s=time.perf_counter() - started,
    )


def selected_gate_specs(
    project_root: Path,
    changed_files: Sequence[str],
    *,
    commands: Optional[Mapping[str, Sequence[str]]] = None,
    narrow: bool = True,
) -> list[GateSpec]:
    """Convenience: the gates that *would* run for these changed files (no execution)."""
    resolved = commands or _default_commands(Path(project_root))
    repo_map = _load_repo_map(Path(project_root))
    return select_gates(changed_files, resolved, narrow=narrow, repo_map=repo_map).gates
