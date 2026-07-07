"""Central logging configuration for DevCouncil.

The codebase is sprinkled with ``logging.getLogger(__name__)`` calls across the
orchestrator, planner, executors, verifier, and LLM layers — but historically no
handler was ever installed, so every ``logger.info``/``logger.debug`` was silently
discarded and only uncaught WARNING+ records reached stderr via Python's "last
resort" handler. That made diagnosing the recurring run failures nearly
impossible: the breadcrumbs existed but went nowhere.

:func:`configure_logging` wires up two sinks, once per process:

* a **rotating file** at ``.devcouncil/logs/devcouncil.log`` that always captures
  *everything* at DEBUG — this is the durable record you grep after a bad run;
* a **console** (stderr) handler whose level is dialed by ``-v``/``-q`` flags or
  the ``DEVCOUNCIL_LOG_LEVEL`` env var, defaulting to WARNING so normal Rich CLI
  output stays clean.

It is idempotent: calling it again only adjusts the console level (e.g. when a
command re-points at a different ``--project-root``), so importing modules can
call it freely without stacking duplicate handlers.
"""

from __future__ import annotations

import logging
import os
from contextlib import contextmanager
from logging.handlers import RotatingFileHandler
from pathlib import Path
from typing import Iterator, Optional

# Sentinels so we can find (and reconfigure) our own handlers on repeat calls
# without disturbing handlers another library may have installed on the root.
_FILE_HANDLER_TAG = "devcouncil.file"
_CONSOLE_HANDLER_TAG = "devcouncil.console"

_LOG_FORMAT = "%(asctime)s %(levelname)-7s [%(name)s] %(message)s"
_DATE_FORMAT = "%Y-%m-%d %H:%M:%S"

# Last (console_level, log_path) we announced, so repeat configure_logging()
# calls in the same process don't spam identical "Logging configured" lines
# into the DEBUG file (observed: ~16% of a real log was this one line).
_last_announced_config: Optional[tuple] = None

# Messages already emitted via warn_once() in this process.
_warned_once: set = set()


def warn_once(target_logger: logging.Logger, message: str) -> None:
    """Emit ``message`` at WARNING at most once per process.

    For per-task/per-call code paths that detect a PROCESS-level condition (e.g.
    a risky config value): the first occurrence is signal, the 20th identical
    line in one run is log spam that buries real warnings.
    """
    if message in _warned_once:
        return
    _warned_once.add(message)
    target_logger.warning(message)

# Default file location (relative to a project root) for the durable run log.
LOG_RELATIVE_PATH = Path(".devcouncil") / "logs" / "devcouncil.log"


def _level_from_verbosity(verbosity: int, quiet: bool) -> int:
    """Map ``-v`` count / ``-q`` flag to a console log level.

    quiet -> ERROR; default(0) -> WARNING; -v -> INFO; -vv (or more) -> DEBUG.
    """
    if quiet:
        return logging.ERROR
    if verbosity <= 0:
        return logging.WARNING
    if verbosity == 1:
        return logging.INFO
    return logging.DEBUG


def _resolve_console_level(verbosity: int, quiet: bool, log_level: Optional[str]) -> int:
    """Console level precedence: explicit arg > env var > verbosity flags."""
    explicit = log_level or os.environ.get("DEVCOUNCIL_LOG_LEVEL")
    if explicit:
        resolved = logging.getLevelName(explicit.strip().upper())
        if isinstance(resolved, int):
            return resolved
    return _level_from_verbosity(verbosity, quiet)


def _resolve_log_path(project_root: Optional[Path]) -> Path:
    """Resolve the shared DEBUG log file location.

    ``DEVCOUNCIL_LOG_DIR`` overrides everything — used by the test suite so
    fixture noise (fake tasks, deliberate failures) never lands in a real
    project's ``.devcouncil/logs/devcouncil.log``, and available to anyone who
    wants logs outside the repo (CI, read-only checkouts).
    """
    override = os.environ.get("DEVCOUNCIL_LOG_DIR")
    if override:
        return Path(override) / "devcouncil.log"
    base = Path(project_root) if project_root is not None else Path.cwd()
    return base / LOG_RELATIVE_PATH


def _find_tagged_handler(logger: logging.Logger, tag: str) -> Optional[logging.Handler]:
    for handler in logger.handlers:
        if getattr(handler, "_devcouncil_tag", None) == tag:
            return handler
    return None


class _CurrentStderrHandler(logging.StreamHandler):
    """A stderr handler that resolves ``sys.stderr`` at EMIT time, not at creation.

    The handler is installed once per process and reused. A plain
    ``logging.StreamHandler()`` snapshots ``sys.stderr`` when constructed; in any
    context that later replaces or closes that stream — test runners, an MCP server
    re-pointing stdio, agents embedding the CLI — every subsequent record then hits a
    dead stream and the logging module prints a ``--- Logging error ---`` report to
    the *current* stderr, polluting agent-facing (e.g. ``--json``) output. Resolving
    the stream per emit keeps console logging bound to whatever stderr is now."""

    def __init__(self) -> None:
        import sys

        super().__init__(sys.stderr)

    @property
    def stream(self):  # type: ignore[override]
        import sys

        return sys.stderr

    @stream.setter
    def stream(self, value) -> None:  # noqa: ARG002 - always current sys.stderr
        # Ignore assignments (StreamHandler.__init__ / setStream): this handler is
        # permanently bound to the CURRENT sys.stderr by design.
        pass


def configure_logging(
    project_root: Optional[Path] = None,
    *,
    verbosity: int = 0,
    quiet: bool = False,
    log_level: Optional[str] = None,
) -> Optional[Path]:
    """Install DevCouncil's file + console log handlers on the root logger.

    Safe to call repeatedly. The file handler (DEBUG, rotating) is created once;
    subsequent calls only update the console handler's level so a later command
    invocation can raise/lower verbosity. Returns the resolved log file path, or
    ``None`` if the file sink could not be created (console logging still works).
    """
    root = logging.getLogger()
    # The root must pass DEBUG records through to handlers; each handler then
    # applies its own threshold (file=DEBUG, console=user-selected).
    root.setLevel(logging.DEBUG)

    formatter = logging.Formatter(_LOG_FORMAT, datefmt=_DATE_FORMAT)
    console_level = _resolve_console_level(verbosity, quiet, log_level)

    # --- Console handler (stderr) -------------------------------------------
    console = _find_tagged_handler(root, _CONSOLE_HANDLER_TAG)
    if console is None:
        console = _CurrentStderrHandler()  # emit-time stderr, keeps stdout clean
        console._devcouncil_tag = _CONSOLE_HANDLER_TAG  # type: ignore[attr-defined]
        console.setFormatter(formatter)
        root.addHandler(console)
    console.setLevel(console_level)

    # --- Rotating file handler (always DEBUG) -------------------------------
    log_path: Optional[Path] = None
    if _find_tagged_handler(root, _FILE_HANDLER_TAG) is None:
        log_path = _resolve_log_path(project_root)
        try:
            log_path.parent.mkdir(parents=True, exist_ok=True)
            file_handler = RotatingFileHandler(
                log_path,
                maxBytes=5 * 1024 * 1024,  # 5 MB per file
                backupCount=5,             # keep ~25 MB of history
                encoding="utf-8",
            )
            file_handler._devcouncil_tag = _FILE_HANDLER_TAG  # type: ignore[attr-defined]
            file_handler.setLevel(logging.DEBUG)
            file_handler.setFormatter(formatter)
            root.addHandler(file_handler)
        except OSError:
            # Read-only FS or unwritable path: degrade to console-only rather
            # than crashing the command the user actually asked for.
            log_path = None
    else:
        log_path = _resolve_log_path(project_root)

    # Quieten chatty third-party loggers on the console; the file still gets them.
    for noisy in ("httpx", "httpcore", "urllib3", "asyncio"):
        logging.getLogger(noisy).setLevel(logging.WARNING)

    _install_excepthook()

    global _last_announced_config
    config = (console_level, str(log_path))
    if config != _last_announced_config:
        _last_announced_config = config
        logging.getLogger(__name__).debug(
            "Logging configured (console=%s, file=%s)",
            logging.getLevelName(console_level),
            log_path,
        )
    return log_path


def _install_excepthook() -> None:
    """Ensure an uncaught exception is written to the log (with traceback) before exit.

    A crash otherwise only prints a traceback to the terminal — gone once the scrollback
    is. Routing it through logging means the full stack also lands in the durable DEBUG
    file (and any active per-run log), which is exactly what you need to diagnose the
    recurring failures. ``KeyboardInterrupt`` is left to the default handler so Ctrl-C
    stays clean. Installed once; idempotent across repeat ``configure_logging`` calls.
    """
    import sys

    if getattr(sys.excepthook, "_devcouncil_hook", False):
        return
    previous = sys.excepthook

    def _hook(exc_type, exc_value, exc_tb):
        if not issubclass(exc_type, KeyboardInterrupt):
            logging.getLogger("devcouncil.crash").critical(
                "Uncaught exception", exc_info=(exc_type, exc_value, exc_tb)
            )
        previous(exc_type, exc_value, exc_tb)

    _hook._devcouncil_hook = True  # type: ignore[attr-defined]
    sys.excepthook = _hook


def set_log_dir(project_root: Path) -> Optional[Path]:
    """Re-point the shared DEBUG file handler at ``project_root/.devcouncil/logs``.

    The CLI callback configures logging before any command knows its ``--project-root``,
    so the file handler initially lands under the current working directory. When a
    command operates on a *different* root (``dev go --project-root /other/repo``), its
    log should live with that project, not in cwd. Each command calls this once it has
    resolved its root; if the handler already points there (the common ``.`` case) this
    is a cheap no-op. Returns the (re)resolved log path, or ``None`` if it could not be
    created (logging then stays on the previous handler / console).
    """
    target = _resolve_log_path(project_root)
    root = logging.getLogger()
    existing = _find_tagged_handler(root, _FILE_HANDLER_TAG)
    if existing is not None:
        current = getattr(existing, "baseFilename", None)
        if current and Path(current) == target.resolve():
            return target  # already logging to this project's file — nothing to do

    try:
        target.parent.mkdir(parents=True, exist_ok=True)
        new_handler = RotatingFileHandler(
            target, maxBytes=5 * 1024 * 1024, backupCount=5, encoding="utf-8"
        )
        new_handler._devcouncil_tag = _FILE_HANDLER_TAG  # type: ignore[attr-defined]
        new_handler.setLevel(logging.DEBUG)
        new_handler.setFormatter(logging.Formatter(_LOG_FORMAT, datefmt=_DATE_FORMAT))
    except OSError:
        return None

    if existing is not None:
        root.removeHandler(existing)
        existing.close()
    root.addHandler(new_handler)
    logging.getLogger(__name__).debug("Log file re-pointed to %s", target)
    return target


@contextmanager
def run_log(log_file: Path) -> Iterator[Optional[Path]]:
    """Capture everything logged during a single invocation into its own DEBUG file.

    The shared rotating ``devcouncil.log`` interleaves every command and rotates by
    size, so isolating one run's complete trail there means grepping across rotations
    and unrelated activity. This attaches a second, run-scoped DEBUG file handler for
    the duration of the ``with`` block (e.g. ``.devcouncil/runs/<id>/run.log``) and
    detaches it on exit — giving a clean, self-contained log for exactly that run on
    top of the always-on shared log.

    Best-effort: if the file can't be opened (read-only FS, bad path) the block still
    runs with only the shared log. Yields the resolved path, or ``None`` on failure.
    """
    root = logging.getLogger()
    handler: Optional[logging.Handler] = None
    try:
        log_file.parent.mkdir(parents=True, exist_ok=True)
        handler = logging.FileHandler(log_file, encoding="utf-8")
        handler._devcouncil_tag = "devcouncil.run"  # type: ignore[attr-defined]
        handler.setLevel(logging.DEBUG)
        handler.setFormatter(logging.Formatter(_LOG_FORMAT, datefmt=_DATE_FORMAT))
        root.addHandler(handler)
    except OSError:
        if handler is not None:
            root.removeHandler(handler)
        handler = None
        log_file = None  # type: ignore[assignment]

    try:
        yield log_file
    finally:
        if handler is not None:
            handler.flush()
            handler.close()
            root.removeHandler(handler)
