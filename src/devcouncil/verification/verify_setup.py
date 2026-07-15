"""Verify-task setup and teardown helpers extracted from Verifier."""

from __future__ import annotations

import asyncio
import logging
from dataclasses import dataclass, field
from typing import Any, List, Optional, TYPE_CHECKING

from devcouncil.app.config import load_config
from devcouncil.domain.requirement import Requirement
from devcouncil.domain.task import Task
from devcouncil.telemetry.stages import log_step
from devcouncil.utils.git_snapshot import GitWorktreeSnapshot
from devcouncil.verification.difficulty import resolve_rigor_policy

if TYPE_CHECKING:
    from devcouncil.verification.verifier import Verifier

logger = logging.getLogger(__name__)


@dataclass
class VerifyRunContext:
    """Configuration and diff state primed at the start of verify_task."""

    config: Any | None
    rigor: Any
    ac_samples: int = 1
    ac_repair_attempts: int = 1
    ac_per_criterion: bool = False
    command_timeout: int = 300
    changed_files: List[str] = field(default_factory=list)
    diff_content: str = ""
    diff_empty: bool = True


def prime_verify_memos(verifier: Verifier) -> None:
    """Batch git plumbing and prime per-call caches."""
    snapshot = GitWorktreeSnapshot.capture(verifier.project_root)
    verifier._git_fallback.git_snapshot = snapshot
    verifier._git_fallback.untracked_cache = snapshot.untracked_files


def resolve_verify_context(
    verifier: Verifier,
    task: Task,
    requirements: List[Requirement],
) -> VerifyRunContext:
    """Load config, rigor policy, and diff snapshot for a verify run."""
    ac_samples, ac_repair_attempts = 1, 1
    ac_per_criterion = False
    cfg = None
    command_timeout = 300
    try:
        from devcouncil.app.config import role_runs_on_local_provider

        cfg = load_config(verifier.project_root)
        command_timeout = cfg.execution.command_timeout
        local_monitor = role_runs_on_local_provider(cfg, "implementation_reviewer")
        ac_samples, ac_repair_attempts, ac_per_criterion = (
            cfg.verification.acceptance_checks.resolved(local_monitor)
        )
        # Explicit overrides are honored but never silently: single-shot acceptance
        # checks on a local monitor rubber-stamped real defects in calibration probes.
        # Once per process — verify runs per task, and 20 identical lines are spam.
        from devcouncil.telemetry.logging_setup import warn_once

        for warning in cfg.verification.acceptance_checks.unsafe_override_warnings(local_monitor):
            warn_once(logger, warning)
    except Exception as exc:
        # Without config we cannot detect a local monitor, so this falls back to
        # single-shot cloud defaults — the exact mode that is unsafe if the monitor
        # IS local. Say so instead of degrading silently.
        from devcouncil.telemetry.logging_setup import warn_once

        warn_once(
            logger,
            f"Verification config unavailable ({exc}); acceptance checks fall back to "
            "single-shot defaults (samples=1). If the monitor runs on a local model, "
            "fix the config load — single-shot local checks miss real defects.",
        )
        cfg = None
    rigor = resolve_rigor_policy(task, requirements, config=cfg)
    if rigor.min_acceptance_samples > 1:
        ac_samples = max(ac_samples, rigor.min_acceptance_samples)
    log_step(
        "verify/1: resolved rigor policy",
        project_root=verifier.project_root,
        task_id=task.id,
        difficulty=rigor.difficulty,
        rigor_applied=list(rigor.applied),
    )
    verifier._command_timeout_cache = command_timeout
    changed_files = verifier.get_task_changed_files(task.id)
    diff_content = verifier.get_diff()
    committed_diff = verifier._committed_task_diff(task.id)
    if committed_diff.strip():
        diff_content = committed_diff
    diff_empty = not bool(diff_content.strip())
    log_step(
        "verify/2: collected diff and changed files",
        project_root=verifier.project_root,
        task_id=task.id,
        changed_files=len(changed_files),
        diff_empty=diff_empty,
    )
    return VerifyRunContext(
        config=cfg,
        rigor=rigor,
        ac_samples=ac_samples,
        ac_repair_attempts=ac_repair_attempts,
        ac_per_criterion=ac_per_criterion,
        command_timeout=command_timeout,
        changed_files=changed_files,
        diff_content=diff_content,
        diff_empty=diff_empty,
    )


def start_verify_futures(
    verifier: Verifier,
    *,
    task: Task,
    requirements: List[Requirement],
    diff_content: str,
    ac_samples: int,
    ac_per_criterion: bool,
) -> tuple[Optional[asyncio.Task[Any]], Optional[asyncio.Task[Any]]]:
    """Launch acceptance-compiler and implementation-reviewer LLM passes."""
    compile_future: Optional[asyncio.Task[Any]] = None
    if verifier.acceptance_compiler and diff_content and task.acceptance_criterion_ids:
        if hasattr(verifier.acceptance_compiler, "compile_candidates"):
            compile_coro = verifier.acceptance_compiler.compile_candidates(
                task, requirements, diff_content, samples=ac_samples,
                per_criterion=ac_per_criterion,
            )
        else:
            compile_coro = verifier.acceptance_compiler.compile(task, requirements, diff_content)
        compile_future = asyncio.create_task(compile_coro)
    review_future: Optional[asyncio.Task[Any]] = None
    if verifier.reviewer and diff_content:
        review_future = asyncio.create_task(
            verifier.reviewer.review_changes(task, requirements, diff_content)
        )
    return compile_future, review_future


async def cleanup_verify_futures(
    verifier: Verifier,
    compile_future: Optional[asyncio.Task[Any]],
    review_future: Optional[asyncio.Task[Any]],
) -> None:
    """Drain background LLM tasks and clear per-call memos."""
    for fut in (compile_future, review_future):
        if fut is not None:
            if not fut.done():
                fut.cancel()
            try:
                await fut
            except (asyncio.CancelledError, Exception):
                pass
    verifier._git_fallback.git_snapshot = None
    verifier._git_fallback.untracked_cache = None
    verifier._command_timeout_cache = None
    verifier._project_deps_cache = None
