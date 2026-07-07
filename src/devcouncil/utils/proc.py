"""Shared subprocess helpers with consistent timeouts and error handling.

Every git/tool invocation in DevCouncil should go through these helpers (or at
minimum pass an explicit ``timeout=``) so a hung child process can never hang a
verification run or the CLI. Defaults are deliberately generous: they exist to
convert "hangs forever" into "fails loudly", not to race fast commands.
"""

from __future__ import annotations

import logging
import subprocess
from pathlib import Path
from typing import List, Optional, Sequence, Union

logger = logging.getLogger(__name__)

# Default ceiling for git plumbing commands (status/diff/ls-files/rev-parse).
GIT_TIMEOUT: float = 60.0
# Default ceiling for generic tool invocations when the caller has no better bound.
DEFAULT_TIMEOUT: float = 120.0

Cmd = Sequence[str]
PathLike = Union[str, Path]


def run_git(
    args: Cmd,
    cwd: PathLike,
    *,
    timeout: float = GIT_TIMEOUT,
    check: bool = False,
) -> subprocess.CompletedProcess[str]:
    """Run a git command with a timeout, capturing decoded text output.

    Never raises ``TimeoutExpired``: a timeout is logged and surfaced as a
    ``CompletedProcess`` with returncode 124 so existing ``returncode``/stdout
    checks keep working. Raises ``CalledProcessError`` only when ``check=True``
    and the command failed (mirroring ``subprocess.run``).
    """
    cmd: List[str] = ["git", *args] if args and args[0] != "git" else list(args)
    try:
        return subprocess.run(
            cmd,
            cwd=str(cwd),
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=timeout,
            check=check,
        )
    except subprocess.TimeoutExpired:
        logger.warning("git command timed out after %.0fs: %s (cwd=%s)", timeout, " ".join(cmd), cwd)
        return subprocess.CompletedProcess(cmd, returncode=124, stdout="", stderr=f"timed out after {timeout}s")


def git_output(
    args: Cmd,
    cwd: PathLike,
    *,
    timeout: float = GIT_TIMEOUT,
    default: Optional[str] = None,
) -> str:
    """Return stdout of a git command, or ``default`` on any failure.

    When ``default`` is None a failure raises ``CalledProcessError`` /
    ``TimeoutExpired`` for the caller's existing except paths; otherwise the
    failure is logged at debug level and ``default`` is returned.
    """
    cmd: List[str] = ["git", *args] if args and args[0] != "git" else list(args)
    try:
        result = subprocess.run(
            cmd,
            cwd=str(cwd),
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=timeout,
        )
        if result.returncode != 0:
            raise subprocess.CalledProcessError(result.returncode, cmd, result.stdout, result.stderr)
        return result.stdout
    except (subprocess.CalledProcessError, subprocess.TimeoutExpired, OSError) as e:
        if default is not None:
            logger.debug("git command failed (%s): %s (cwd=%s)", e, " ".join(cmd), cwd)
            return default
        raise
