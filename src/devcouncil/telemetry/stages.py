"""Stage and step instrumentation for DevCouncil pipelines.

A DevCouncil run moves through coarse *stages* (plan, execute, verify, repair,
reconcile, report) each made of finer *steps*. When a run misbehaves, the first
question is always "how far did it get, and what was the last thing it tried?".
This module gives one consistent way to answer that:

* :func:`log_stage` — a context manager that logs ``▶ <stage>`` on entry and
  ``✔ <stage> (1.23s)`` / ``✖ <stage> failed (1.23s): ...`` on exit, with wall
  time. It also mirrors the boundary to the structured JSONL trace
  (:class:`~devcouncil.telemetry.traces.TraceLogger`) when a ``project_root`` is
  given, so the human log and the machine trace stay in lock-step.
* :func:`log_step` — a one-liner for a single step inside a stage.

Both are best-effort: instrumentation must never be the reason a run fails, so
trace-write errors are swallowed (the Python log line still goes out).
"""

from __future__ import annotations

import logging
import time
from contextlib import contextmanager
from pathlib import Path
from typing import Any, Iterator, Optional

logger = logging.getLogger("devcouncil.stage")


def _safe_trace(
    project_root: Optional[Path],
    event_type: str,
    details: dict[str, Any],
    *,
    run_id: Optional[str],
    task_id: Optional[str],
    summary: str,
) -> None:
    """Mirror a stage/step boundary to the JSONL trace, swallowing any error."""
    if project_root is None:
        return
    # Never MATERIALIZE a project root that doesn't exist: TraceLogger mkdirs
    # .devcouncil/ under the given path, so tracing under a typo'd/absent
    # --project-root would silently create directories there — and break
    # commands that first check the root's existence (observed: `lsp inspect`
    # reporting a missing root as valid because the stage trace created it).
    if not Path(project_root).exists():
        return
    try:
        from devcouncil.telemetry.traces import TraceLogger

        TraceLogger(Path(project_root)).log_event(
            event_type, details, run_id=run_id, task_id=task_id, summary=summary
        )
    except Exception:  # pragma: no cover - tracing is strictly best-effort
        logger.debug("Failed to mirror %s to trace", event_type, exc_info=True)


def _context_suffix(context: dict[str, Any]) -> str:
    if not context:
        return ""
    return " " + " ".join(f"{k}={v}" for k, v in context.items())


@contextmanager
def log_stage(
    name: str,
    *,
    project_root: Optional[Path] = None,
    run_id: Optional[str] = None,
    task_id: Optional[str] = None,
    **context: Any,
) -> Iterator[None]:
    """Log entry/exit (with timing) around a coarse pipeline stage.

    Usage::

        with log_stage("plan", project_root=root, run_id=run_id, goal=goal):
            ...

    Logs at INFO on success, ERROR on exception, and always reports elapsed time.
    Re-raises whatever the body raised so control flow is unchanged.
    """
    suffix = _context_suffix(context)
    logger.info("▶ %s%s", name, suffix)
    _safe_trace(
        project_root,
        "stage_started",
        {"stage": name, **context},
        run_id=run_id,
        task_id=task_id,
        summary=f"Stage started: {name}",
    )
    start = time.monotonic()
    try:
        yield
    except BaseException as exc:
        elapsed = time.monotonic() - start
        # Control-flow exceptions are NOT stage failures: typer.Exit(1)/SystemExit
        # is how a CLI command reports a legitimate non-zero verdict (e.g. `verify
        # --json` on a blocked task), and logging it as "✖ failed" both misleads
        # and — because the log line prints after the command's JSON — corrupts
        # machine-readable output for any consumer parsing stdout+stderr.
        control_flow = isinstance(exc, (SystemExit, KeyboardInterrupt, GeneratorExit))
        if not control_flow:
            # typer/click CLI exits, matched STRUCTURALLY rather than by isinstance:
            # typer can vendor click (typer._click.exceptions.Exit), which does not
            # subclass the top-level click.exceptions.Exit, so an import-based
            # isinstance check silently misses typer.Exit.
            cls = type(exc)
            control_flow = cls.__name__ in {"Exit", "Abort"} and "click" in (cls.__module__ or "")
        if control_flow:
            logger.debug("· %s exited via control flow (%.2fs): %r", name, elapsed, exc)
            raise
        # %r, not %s: common failures (httpx.ReadTimeout, CancelledError) stringify
        # to an EMPTY message, which logs a useless "failed (0.05s): ."
        logger.error("✖ %s failed (%.2fs): %r", name, elapsed, exc)
        _safe_trace(
            project_root,
            "stage_failed",
            {"stage": name, "error": repr(exc), "elapsed_s": round(elapsed, 3), **context},
            run_id=run_id,
            task_id=task_id,
            summary=f"Stage failed: {name}",
        )
        raise
    else:
        elapsed = time.monotonic() - start
        logger.info("✔ %s (%.2fs)", name, elapsed)
        _safe_trace(
            project_root,
            "stage_completed",
            {"stage": name, "elapsed_s": round(elapsed, 3), **context},
            run_id=run_id,
            task_id=task_id,
            summary=f"Stage completed: {name}",
        )


def log_step(
    message: str,
    *,
    project_root: Optional[Path] = None,
    run_id: Optional[str] = None,
    task_id: Optional[str] = None,
    level: int = logging.INFO,
    trace: bool = False,
    **context: Any,
) -> None:
    """Log a single step within a stage.

    By default this only writes the Python log (cheap, captured in full by the
    file handler). Set ``trace=True`` for milestone steps worth recording in the
    structured JSONL trace as well.
    """
    suffix = _context_suffix(context)
    logger.log(level, "• %s%s", message, suffix)
    if trace:
        _safe_trace(
            project_root,
            "step",
            {"message": message, **context},
            run_id=run_id,
            task_id=task_id,
            summary=message,
        )
